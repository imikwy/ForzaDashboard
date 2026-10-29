// Veröffentlichen: der Stand einer Bearbeiten-Sitzung wird zur neuen Kartenversion (nur auf Herausgeber-PCs).
//
// Ablauf (sicher, ohne den Arbeitsordner anzufassen):
// 1. neuesten Stand von GitHub holen (origin/main:karte/karte.json) – das ist die aktuelle Wahrheit
// 2. Sitzungs-Stand damit zusammenführen (pro Objekt gewinnt die neuere Änderung -> nichts von anderen geht verloren)
// 3. prüfen (gültig? Massenlöschung?) und als Commit direkt auf main schieben
// 4. GitHub Actions prüft erneut, signiert und erstellt das Release "karte-vN" -> alle Spieler laden es automatisch
// Schreibrechte auf das Repo haben nur die Herausgeber – sonst kann niemand eine Kartenversion veröffentlichen.
use crate::karte::{diff, guard, merge, parse, serialize, validate, Karten};
use crate::sitzung::Sitzung;
use crate::util::{log, no_window, take_chars};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FILE: &str = "karte/karte.json";

fn git(root: &Path, args: &[&str], index: Option<&Path>, input: Option<&[u8]>) -> Result<String, String> {
    let mut c = Command::new("git");
    no_window(&mut c).arg("-C").arg(root).args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(i) = index { c.env("GIT_INDEX_FILE", i); }
    if input.is_some() { c.stdin(Stdio::piped()); }
    let mut ch = c.spawn().map_err(|_| "Git nicht gefunden".to_string())?;
    if let (Some(data), Some(mut si)) = (input, ch.stdin.take()) { si.write_all(data).map_err(|e| e.to_string())?; }
    let out = ch.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).trim().to_string()); }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn capability(root: &Path) -> Value {
    if !root.join(".git").exists() { return json!({"ok": false, "why": "Dieser Ordner ist keine Repo-Kopie – Veröffentlichen geht nur auf den Herausgeber-PCs"}); }
    match git(root, &["remote", "get-url", "origin"], None, None) {
        Ok(u) if !u.trim().is_empty() => json!({"ok": true, "remote": u.trim()}),
        _ => json!({"ok": false, "why": "Kein GitHub-Repo (origin) eingerichtet"}),
    }
}

pub fn preview(s: &Sitzung, karten: &Karten, root: &Path) -> Value {
    let base = karten.ents(s.base()).map(|a| a.to_vec()).unwrap_or_default();
    let ents = s.state_ents();
    let d = diff(&base, &ents);
    let v = validate(&ents);
    json!({"ok": v.ok, "errors": v.errors, "stats": v.stats, "guard": guard(&d), "diff": d, "pending": s.pending_n(), "base": s.base(), "herausgeber": capability(root)})
}

pub fn publish(s: &Sitzung, root: &Path, notiz: &str, name: &str, mass_ok: bool) -> Value {
    let c = capability(root);
    if c["ok"] != true { return json!({"ok": false, "why": c["why"]}); }
    let n = s.pending_n();
    if n > 0 { return json!({"ok": false, "why": format!("{n} Änderung(en) sind noch nicht vom Host bestätigt – kurz warten und erneut versuchen")}); }
    let mine = s.state_ents();
    for attempt in 1..=3 {
        if let Err(e) = git(root, &["fetch", "--quiet", "origin", "main"], None, None) { return json!({"ok": false, "why": format!("GitHub nicht erreichbar: {e}")}); }
        let remote = git(root, &["show", &format!("origin/main:{FILE}")], None, None).ok().and_then(|t| parse(&t).ok()).unwrap_or_default();
        let merged = merge(&remote, &mine);
        let v = validate(&merged);
        if !v.ok { return json!({"ok": false, "why": format!("Karte ungültig: {}", v.errors.iter().take(3).cloned().collect::<Vec<_>>().join("; "))}); }
        let d = diff(&remote, &merged);
        let probs = guard(&d);
        let (neu, ge, del) = (d["neu"].as_u64().unwrap_or(0), d["geaendert"].as_u64().unwrap_or(0), d["geloescht"].as_u64().unwrap_or(0));
        if neu + ge + del == 0 { return json!({"ok": false, "why": "Keine Änderungen gegenüber der veröffentlichten Karte"}); }
        if !probs.is_empty() && !mass_ok { return json!({"ok": false, "guard": probs, "diff": d, "why": format!("Schutzregel: {}", probs.join("; "))}); }
        let sum = [(neu, "neu"), (ge, "geändert"), (del, "gelöscht")].iter().filter(|(n, _)| *n > 0).map(|(n, t)| format!("{n} {t}")).collect::<Vec<_>>().join(", ");
        let mut msg = format!("Karte: {sum} (Sitzung {}, {})", s.code(), if name.is_empty() { "?" } else { name });
        if !notiz.trim().is_empty() { msg.push_str(&format!("\n\n{}", take_chars(notiz.trim(), 500))); }
        if mass_ok { msg.push_str("\n\n[massenloeschung-ok]"); }
        // Commit bauen, ohne Arbeitsordner oder aktuellen Branch zu verändern (eigener, temporärer Index)
        let idx: PathBuf = std::env::temp_dir().join(format!("fd-pub-{}.index", crate::util::random_hex(4)));
        let res = (|| -> Result<String, String> {
            let blob = git(root, &["hash-object", "-w", "--stdin"], None, Some(serialize(&merged).as_bytes()))?.trim().to_string();
            git(root, &["read-tree", "origin/main"], Some(&idx), None)?;
            git(root, &["update-index", "--add", "--cacheinfo", &format!("100644,{blob},{FILE}")], Some(&idx), None)?;
            let tree = git(root, &["write-tree"], Some(&idx), None)?.trim().to_string();
            let commit = git(root, &["commit-tree", &tree, "-p", "origin/main", "-m", &msg], None, None)?.trim().to_string();
            git(root, &["push", "origin", &format!("{commit}:refs/heads/main")], None, None)?;
            Ok(commit)
        })();
        let _ = std::fs::remove_file(&idx);
        match res {
            Ok(commit) => {
                log(format!("Karte veröffentlicht: {sum} (Commit {})", &commit[..8.min(commit.len())]));
                return json!({"ok": true, "commit": &commit[..8.min(commit.len())], "diff": d,
                    "text": format!("Veröffentlicht ({sum}). GitHub prüft und erstellt die neue Version – in 1–2 Minuten bekommen sie alle automatisch.")});
            }
            Err(e) if attempt < 3 && ["rejected", "fetch first", "non-fast-forward"].iter().any(|k| e.contains(k)) => {
                log("Veröffentlichen: jemand war schneller – neuer Versuch");
            }
            Err(e) => return json!({"ok": false, "why": format!("Veröffentlichen fehlgeschlagen: {e}")}),
        }
    }
    json!({"ok": false, "why": "Veröffentlichen fehlgeschlagen – bitte erneut versuchen"})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mqtt::TestBus;
    use crate::sitzung::{Ctx, Sitzungen};
    use std::sync::Arc;

    fn sh(dir: &Path, args: &[&str]) -> String {
        git(dir, args, None, None).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    #[test]
    fn veroeffentlichen_fuehrt_zusammen_und_schuetzt() {
        let root = std::env::temp_dir().join(format!("fd-test-pub-{}", crate::util::random_hex(4)));
        let (bare, a, b) = (root.join("origin.git"), root.join("a"), root.join("b"));
        std::fs::create_dir_all(&root).unwrap();
        sh(&root, &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
        // Ausgangsstand auf GitHub: die Startkarte
        let start = crate::karte::Karten::new(&root.join("start"));
        for d in [&a, &b] { sh(&root, &["clone", "-q", bare.to_str().unwrap(), d.to_str().unwrap()]); }
        for d in [&a, &b] { sh(d, &["config", "user.email", "t@t"]); sh(d, &["config", "user.name", "Test"]); }
        std::fs::create_dir_all(a.join("karte")).unwrap();
        std::fs::write(a.join("karte/karte.json"), serialize(&start.active().unwrap().ents)).unwrap();
        sh(&a, &["add", "-A"]);
        sh(&a, &["commit", "-q", "-m", "Start"]);
        sh(&a, &["push", "-q", "origin", "main"]);
        assert_eq!(capability(&a)["ok"], true);

        let bus = Arc::new(TestBus::default());
        let mk = |d: &Path, dev: &str| {
            let ctx = Ctx { net: bus.net(dev), karten: Arc::new(crate::karte::Karten::new(&d.with_extension("daten"))), dev: dev.into(),
                name: Arc::new(|| "Test".into()), dir: d.with_extension("daten").join("Sitzungen"), emit: Arc::new(|_| {}) };
            Sitzungen::new(ctx)
        };
        // zwei Herausgeber veröffentlichen nacheinander aus getrennten Sitzungen – keiner überschreibt den anderen
        let (sa, sb) = (mk(&a, "aaaaaaaaaa"), mk(&b, "bbbbbbbbbb"));
        sa.create("A");
        sb.create("B");
        sa.cur().unwrap().submit(&[json!({"id": "r-pub-a", "k": "r", "t": "road", "p": [0, 0, 10, 10]})], None);
        sb.cur().unwrap().submit(&[json!({"id": "r-pub-b", "k": "r", "t": "road", "p": [5, 5, 20, 20]})], None);
        let ra = publish(&sa.cur().unwrap(), &a, "Test A", "A", false);
        assert_eq!(ra["ok"], true, "{ra}");
        let rb = publish(&sb.cur().unwrap(), &b, "", "B", false);
        assert_eq!(rb["ok"], true, "{rb}");
        sh(&a, &["fetch", "-q", "origin", "main"]);
        let remote = parse(&sh(&a, &["show", "origin/main:karte/karte.json"])).unwrap();
        assert!(remote.iter().any(|e| e["id"] == "r-pub-a") && remote.iter().any(|e| e["id"] == "r-pub-b"));
        // der Arbeitsordner wurde nicht angefasst
        assert!(sh(&a, &["status", "--porcelain"]).trim().is_empty());
        // nichts Neues -> nichts zu veröffentlichen
        assert_eq!(publish(&sa.cur().unwrap(), &a, "", "A", false)["ok"], false);
        // Massenlöschung wird blockiert (mehr als 5 % der Startkarte, egal wie groß sie inzwischen ist)
        let ents = &start.active().unwrap().ents;
        let all: Vec<Value> = ents.iter().filter(|e| e["d"] != 1).take((ents.len() * 6 / 100).max(250))
            .map(|e| json!({"id": e["id"], "k": e["k"], "d": 1})).collect();
        sa.cur().unwrap().submit(&all, None);
        let r = publish(&sa.cur().unwrap(), &a, "", "A", false);
        assert_eq!(r["ok"], false);
        assert!(r["why"].as_str().unwrap().starts_with("Schutzregel"), "{r}");
        sa.close(false);
        sb.close(false);
        let _ = std::fs::remove_dir_all(root);
    }
}
