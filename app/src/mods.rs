// Mods von Nexus Mods (nur Forza Horizon 6): stöbern, herunterladen, entpacken und in den Spielordner kopieren.
// AUF EIGENE GEFAHR: veränderte Spieldateien können zu Sperren im Online-Modus führen.
// - Der persönliche API-Schlüssel (Nexus: Einstellungen → API-Schlüssel) bleibt in der exe (mods.json im Datenordner).
// - Premium: Download direkt. Kostenloses Konto: auf der Nexus-Seite „Mod Manager Download“ → nxm://-Link → hierher.
// - Jede überschriebene Datei wird vorher gesichert; Deinstallieren stellt den alten Stand wieder her.
use crate::state::Shared;
use crate::util::{log, now_ms, read_json, write_json};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const API: &str = "https://api.nexusmods.com/v1";
const GAME: &str = "forzahorizon6";

fn agent(timeout: u64) -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(timeout)).user_agent(&format!("ForzaDashboard/{}", crate::APP_VER));
    if let Ok(tls) = native_tls::TlsConnector::new() { b = b.tls_connector(Arc::new(tls)); }
    b.build()
}
fn cfg_file(s: &Shared) -> PathBuf { s.file("mods.json") }
fn cfg(s: &Shared) -> Value { read_json(&cfg_file(s)).unwrap_or_else(|| json!({})) }
fn save(s: &Shared, v: &Value) { write_json(&cfg_file(s), v); }
fn root(s: &Shared) -> PathBuf { s.file("mods") }

fn get(s: &Shared, path: &str) -> Result<Value, String> {
    let key = cfg(s)["key"].as_str().unwrap_or("").to_string();
    if key.is_empty() { return Err("Kein Nexus-API-Schlüssel eingetragen".into()); }
    match agent(25).get(&format!("{API}{path}")).set("apikey", &key).set("Application-Name", "ForzaDashboard").set("Application-Version", crate::APP_VER).call() {
        Ok(r) => serde_json::from_str(&r.into_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string()),
        Err(ureq::Error::Status(401, _)) => Err("Nexus lehnt den Schlüssel ab".into()),
        Err(ureq::Error::Status(403, _)) => Err("Nexus erlaubt das nur mit Premium – auf der Nexus-Seite „Mod Manager Download“ nutzen".into()),
        Err(ureq::Error::Status(404, _)) => Err("Bei Nexus nicht gefunden".into()),
        Err(ureq::Error::Status(429, _)) => Err("Nexus: zu viele Anfragen – kurz warten".into()),
        Err(ureq::Error::Status(c, _)) => Err(format!("Nexus antwortet {c}")),
        Err(e) => Err(e.to_string()),
    }
}

// Spielordner automatisch suchen (Steam und Xbox-App)
fn detect_dir() -> Option<String> {
    let mut cands: Vec<PathBuf> = Vec::new();
    for d in ["C", "D", "E", "F", "G"] {
        cands.push(PathBuf::from(format!("{d}:\\XboxGames\\Forza Horizon 6\\Content")));
        cands.push(PathBuf::from(format!("{d}:\\SteamLibrary\\steamapps\\common\\ForzaHorizon6")));
        cands.push(PathBuf::from(format!("{d}:\\Program Files (x86)\\Steam\\steamapps\\common\\ForzaHorizon6")));
    }
    cands.into_iter().find(|p| p.is_dir()).map(|p| p.to_string_lossy().to_string())
}

pub fn status(s: &Shared) -> Value {
    let c = cfg(s);
    let dir = c["dir"].as_str().map(String::from).or_else(detect_dir).unwrap_or_default();
    json!({"key": c["key"].as_str().map(|k| !k.is_empty()).unwrap_or(false), "user": c["user"], "premium": c["premium"], "dir": dir,
        "dirOk": !dir.is_empty() && Path::new(&dir).is_dir(), "installed": c["installed"].as_array().cloned().unwrap_or_default(), "nxm": c["nxm"].as_bool().unwrap_or(false)})
}

pub fn set_key(s: &Shared, key: &str) -> Value {
    let key: String = key.trim().chars().filter(|c| c.is_ascii_alphanumeric() || "-_=+/".contains(*c)).take(400).collect();
    let mut c = cfg(s);
    if key.is_empty() { c["key"] = json!(""); save(s, &c); return json!({"ok": true}); }
    c["key"] = json!(key); save(s, &c);
    match get(s, "/users/validate.json") {
        Ok(v) => { c["user"] = v["name"].clone(); c["premium"] = json!(v["is_premium"].as_bool().unwrap_or(false)); save(s, &c); json!({"ok": true, "user": v["name"], "premium": c["premium"]}) }
        Err(e) => { c["key"] = json!(""); save(s, &c); json!({"ok": false, "why": e}) }
    }
}

pub fn set_dir(s: &Shared, dir: &str) -> Value {
    let d = dir.trim().trim_matches('"');
    if !Path::new(d).is_dir() { return json!({"ok": false, "why": "Ordner gibt es nicht"}); }
    let mut c = cfg(s); c["dir"] = json!(d); save(s, &c);
    json!({"ok": true})
}

pub fn list(s: &Shared, kind: &str) -> Value {
    let p = match kind { "neu" => "latest_added", "aktuell" => "latest_updated", _ => "trending" };
    match get(s, &format!("/games/{GAME}/mods/{p}.json")) {
        Ok(v) => json!({"ok": true, "mods": v.as_array().cloned().unwrap_or_default().into_iter().filter(|m| m["available"].as_bool().unwrap_or(true) && m["status"].as_str().unwrap_or("published") == "published").map(slim).collect::<Vec<_>>()}),
        Err(e) => json!({"ok": false, "why": e}),
    }
}
fn slim(m: Value) -> Value {
    json!({"id": m["mod_id"], "name": m["name"], "summary": m["summary"], "pic": m["picture_url"], "author": m["author"], "version": m["version"],
        "up": m["endorsement_count"], "dl": m["mod_downloads"], "updated": m["updated_timestamp"], "adult": m["contains_adult_content"]})
}

pub fn detail(s: &Shared, id: u64) -> Value {
    let m = match get(s, &format!("/games/{GAME}/mods/{id}.json")) { Ok(v) => v, Err(e) => return json!({"ok": false, "why": e}) };
    let files = get(s, &format!("/games/{GAME}/mods/{id}/files.json")).map(|v| v["files"].clone()).unwrap_or(json!([]));
    let files: Vec<Value> = files.as_array().cloned().unwrap_or_default().into_iter()
        .filter(|f| !matches!(f["category_name"].as_str(), Some("ARCHIVED") | Some("DELETED") | None) || f["category_id"].as_u64() == Some(1))
        .map(|f| json!({"id": f["file_id"], "name": f["name"], "version": f["version"], "cat": f["category_name"], "kb": f["size_kb"], "file": f["file_name"], "desc": f["description"]})).collect();
    let mut o = slim(m.clone()); o["desc"] = m["description"].clone(); o["files"] = json!(files); o["ok"] = json!(true);
    o
}

// Download-Link: Premium direkt, sonst mit key/expires aus dem nxm-Link
fn link(s: &Shared, mod_id: u64, file_id: u64, nxm: Option<(&str, &str)>) -> Result<String, String> {
    let q = nxm.map(|(k, e)| format!("?key={k}&expires={e}")).unwrap_or_default();
    let v = get(s, &format!("/games/{GAME}/mods/{mod_id}/files/{file_id}/download_link.json{q}"))?;
    v.as_array().and_then(|a| a.first()).and_then(|x| x["URI"].as_str()).map(String::from).ok_or_else(|| "Kein Download-Link".into())
}

fn progress(s: &Shared, state: &str, pct: f64, msg: &str, mod_id: u64) {
    s.emit(Some("mods"), json!({"state": state, "pct": (pct * 1000.0).round() / 10.0, "msg": msg, "mod": mod_id}).to_string());
}

pub fn install(s: Arc<Shared>, mod_id: u64, file_id: u64, nxm: Option<(String, String)>) {
    std::thread::spawn(move || {
        let r = install_inner(&s, mod_id, file_id, nxm.as_ref().map(|(a, b)| (a.as_str(), b.as_str())));
        match r {
            Ok(n) => { progress(&s, "fertig", 1.0, &format!("Installiert ({n} Dateien)"), mod_id); log(format!("Mod {mod_id}/{file_id} installiert ({n} Dateien)")); }
            Err(e) => { progress(&s, "fehler", 0.0, &e, mod_id); log(format!("Mod {mod_id}/{file_id}: {e}")); }
        }
    });
}

fn install_inner(s: &Shared, mod_id: u64, file_id: u64, nxm: Option<(&str, &str)>) -> Result<usize, String> {
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    if !game.is_dir() { return Err("Erst den Spielordner festlegen".into()); }
    if cfg(s)["installed"].as_array().map(|a| a.iter().any(|x| x["mod"].as_u64() == Some(mod_id))).unwrap_or(false) { return Err("Diese Mod ist schon installiert – erst deinstallieren".into()); }
    progress(s, "laden", 0.0, "Download-Link holen …", mod_id);
    let url = link(s, mod_id, file_id, nxm)?;
    let name = url.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("mod").replace("%20", " ");
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if ext == "rar" { return Err("RAR-Archive kann das Dashboard nicht entpacken – bitte von Hand installieren".into()); }
    let dl = root(s).join("downloads"); std::fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    let arch = dl.join(format!("{mod_id}-{file_id}.{ext}"));
    // herunterladen mit Fortschritt
    let resp = agent(600).get(&url).call().map_err(|e| e.to_string())?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut rd = resp.into_reader();
    let mut out = std::fs::File::create(&arch).map_err(|e| e.to_string())?;
    let (mut got, mut buf, mut last) = (0u64, vec![0u8; 1 << 16], 0u64);
    loop {
        let n = rd.read(&mut buf).map_err(|e| e.to_string())?; if n == 0 { break; }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?; got += n as u64;
        if now_ms() - last > 300 { last = now_ms(); progress(s, "laden", if total > 0 { got as f64 / total as f64 * 0.8 } else { 0.4 }, &format!("Lädt … {} MB", got / 1_048_576), mod_id); }
    }
    drop(out);
    // entpacken
    progress(s, "entpacken", 0.82, "Entpacken …", mod_id);
    let stage = root(s).join("staging").join(format!("{mod_id}-{file_id}"));
    let _ = std::fs::remove_dir_all(&stage); std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    match ext.as_str() {
        "zip" => unzip(&arch, &stage)?,
        "7z" => sevenz_rust::decompress_file(&arch, &stage).map_err(|e| format!("7z: {e}"))?,
        _ => { let f = stage.join(&name); std::fs::copy(&arch, &f).map_err(|e| e.to_string())?; }
    }
    // gemeinsamen Oberordner abschneiden (z. B. „MeineMod/media/…“ → „media/…“)
    let mut base = stage.clone();
    for _ in 0..2 {
        let items: Vec<_> = std::fs::read_dir(&base).map_err(|e| e.to_string())?.flatten().collect();
        if items.len() == 1 && items[0].path().is_dir() && !matches!(items[0].file_name().to_string_lossy().to_ascii_lowercase().as_str(), "media" | "mods" | "content") { base = items[0].path(); } else { break; }
    }
    // kopieren mit Sicherung
    progress(s, "installieren", 0.9, "In den Spielordner kopieren …", mod_id);
    // ReShade-Presets in reshade-presets\ einsortieren
    { let mut all = Vec::new(); walk(&base, &base, &mut all);
      for (rel, _) in all { let lc = rel.to_ascii_lowercase(); if lc.ends_with(".ini") && !lc.ends_with("reshade.ini") && !lc.starts_with("reshade-presets/") && is_preset_file(&base.join(&rel)) {
          let to = base.join("reshade-presets").join(rel.rsplit('/').next().unwrap_or(&rel)); let _ = std::fs::create_dir_all(to.parent().unwrap()); let _ = move_file(&base.join(&rel), &to); } } }
    let backup = root(s).join("backup").join(mod_id.to_string());
    let mut files = Vec::new();
    copy_tree(&base, &base, &game, &backup, &mut files)?;
    let _ = rs_after(&game, &stage, &mut files);
    let mut c = cfg(s);
    let detail = get(s, &format!("/games/{GAME}/mods/{mod_id}.json")).unwrap_or(json!({}));
    let entry = json!({"mod": mod_id, "file": file_id, "name": detail["name"], "version": detail["version"], "pic": detail["picture_url"], "time": now_ms(), "files": files});
    if !c["installed"].is_array() { c["installed"] = json!([]); }
    c["installed"].as_array_mut().unwrap().push(entry);
    save(s, &c);
    let _ = std::fs::remove_dir_all(&stage);
    rs_sync(s);
    Ok(files.len())
}

fn safe_rel(rel: &Path) -> bool { rel.components().all(|c| matches!(c, Component::Normal(_))) }

fn unzip(arch: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(arch).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("ZIP: {e}"))?;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = e.enclosed_name() else { continue };   // schützt vor „../“-Pfaden
        let p = dest.join(rel);
        if e.is_dir() { std::fs::create_dir_all(&p).map_err(|e| e.to_string())?; continue; }
        if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
        let mut o = std::fs::File::create(&p).map_err(|e| e.to_string())?;
        std::io::copy(&mut e, &mut o).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn copy_tree(base: &Path, dir: &Path, game: &Path, backup: &Path, files: &mut Vec<Value>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        if p.is_dir() { copy_tree(base, &p, game, backup, files)?; continue; }
        let rel = p.strip_prefix(base).map_err(|e| e.to_string())?.to_path_buf();
        if !safe_rel(&rel) { continue; }
        let to = game.join(&rel);
        let existed = to.exists();
        if existed { let b = backup.join(&rel); if let Some(d) = b.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; } if !b.exists() { std::fs::copy(&to, &b).map_err(|e| format!("Sicherung: {e}"))?; } }
        if let Some(d) = to.parent() { std::fs::create_dir_all(d).map_err(|e| format!("Spielordner: {e}"))?; }
        std::fs::copy(&p, &to).map_err(|e| format!("Kopieren nach {}: {e}", to.display()))?;
        files.push(json!({"rel": rel.to_string_lossy(), "new": !existed}));
    }
    Ok(())
}

pub fn uninstall(s: &Shared, mod_id: u64) -> Value {
    let mut c = cfg(s);
    let game = PathBuf::from(status(s)["dir"].as_str().unwrap_or(""));
    let Some(list) = c["installed"].as_array_mut() else { return json!({"ok": false, "why": "nicht installiert"}) };
    let Some(i) = list.iter().position(|x| x["mod"].as_u64() == Some(mod_id)) else { return json!({"ok": false, "why": "nicht installiert"}) };
    let entry = list.remove(i);
    let backup = root(s).join("backup").join(mod_id.to_string());
    let mut bad = 0;
    for f in entry["files"].as_array().cloned().unwrap_or_default() {
        let rel = PathBuf::from(f["rel"].as_str().unwrap_or(""));
        if !safe_rel(&rel) || rel.as_os_str().is_empty() { continue; }
        let to = game.join(&rel);
        if f["new"].as_bool().unwrap_or(false) { if to.exists() && std::fs::remove_file(&to).is_err() { bad += 1; } }
        else if std::fs::copy(backup.join(&rel), &to).is_err() { bad += 1; }
    }
    save(s, &c);
    if bad == 0 { let _ = std::fs::remove_dir_all(&backup); }
    json!({"ok": bad == 0, "why": if bad > 0 { format!("{bad} Dateien konnten nicht zurückgesetzt werden") } else { String::new() }})
}

// nxm://forzahorizon6/mods/123/files/456?key=…&expires=…&user_id=… (von „Mod Manager Download“ auf der Nexus-Seite)
pub fn handle_nxm(s: Arc<Shared>, url: &str) {
    let u = url.trim().trim_matches('"');
    let Some(rest) = u.strip_prefix("nxm://") else { return };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 5 || !parts[0].eq_ignore_ascii_case(GAME) || parts[1] != "mods" || parts[3] != "files" { progress(&s, "fehler", 0.0, "Dieser Link ist nicht für Forza Horizon 6", 0); return; }
    let (Ok(m), Ok(f)) = (parts[2].parse::<u64>(), parts[4].parse::<u64>()) else { return };
    let q = |k: &str| query.split('&').find_map(|p| p.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    let (key, exp) = (q("key"), q("expires"));
    if key.is_empty() || exp.is_empty() { return; }
    log(format!("Nexus-Link empfangen: Mod {m}, Datei {f}"));
    install(s, m, f, Some((key, exp)));
}

// nxm-Links unter Windows an diese exe binden (nur für den aktuellen Benutzer). Ersetzt z. B. Vortex als Empfänger.
pub fn register_nxm(s: &Shared) -> Value {
    let Ok(exe) = std::env::current_exe() else { return json!({"ok": false}) };
    let cmd = format!("\"{}\" \"%1\"", exe.display());
    let run = |args: &[&str]| crate::util::no_window(std::process::Command::new("reg").args(args)).status().map(|x| x.success()).unwrap_or(false);
    let ok = run(&["add", r"HKCU\Software\Classes\nxm", "/ve", "/d", "URL:NXM Protocol", "/f"])
        && run(&["add", r"HKCU\Software\Classes\nxm", "/v", "URL Protocol", "/d", "", "/f"])
        && run(&["add", r"HKCU\Software\Classes\nxm\shell\open\command", "/ve", "/d", &cmd, "/f"]);
    if ok { let mut c = cfg(s); c["nxm"] = json!(true); save(s, &c); }
    json!({"ok": ok})
}

// Nexus- bzw. ForzaFire-Seite im normalen Browser öffnen (nur diese Seiten)
pub fn open_page(url: &str) -> bool {
    if !(url.starts_with("https://www.nexusmods.com/") || url.starts_with("https://next.nexusmods.com/") || url.starts_with("https://users.nexusmods.com/") || url.starts_with("https://www.forzafire.com/")) || url.contains('"') { return false; }
    crate::util::no_window(std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url])).spawn().is_ok()
}

// ---------------------------------------------------------------------------------------------
// Mod-Manager: auch von Hand installierte Mods im Spielordner finden, pausieren, aktivieren, löschen.
// Originaldateien werden NIE angefasst: Die Xbox-/MS-Store-Version listet in layout_*.xml jede Originaldatei –
// alles andere ist eine Mod. Ohne diese Liste (Steam) zählen nur mediapc\… und typische Mod-Dateien im Hauptordner.
// Pausieren = Dateien in den Datenordner verschieben (mods\aus\…), Aktivieren = zurückschieben.
// Löschen = in den Papierkorb (lässt sich dort wiederherstellen).
// ---------------------------------------------------------------------------------------------
use std::collections::{BTreeMap, HashMap, HashSet};

fn rel_str(p: &Path) -> String { p.components().filter_map(|c| match c { Component::Normal(x) => Some(x.to_string_lossy().to_string()), _ => None }).collect::<Vec<_>>().join("/") }

// Originaldateien laut layout_*.xml (klein geschrieben, mit „/“) und ihre Ordner. None = keine Liste vorhanden.
fn originals(game: &Path) -> Option<(HashSet<String>, HashSet<String>)> {
    let mut files = HashSet::new();
    let mut found = false;
    for e in std::fs::read_dir(game).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        if !(n.starts_with("layout_") && n.ends_with(".xml")) { continue; }
        let Ok(x) = std::fs::read_to_string(e.path()) else { continue };
        found = true;
        for line in x.lines() {
            let attr = |a: &str| line.split_once(&format!("{a}=\"")).and_then(|(_, r)| r.split_once('"')).map(|(v, _)| v.to_string());
            if let (Some(d), Some(i)) = (attr("DestinationPath"), attr("Include")) {
                let p = format!("{}\\{}", d, i);
                let r: Vec<&str> = p.split('\\').filter(|x| !x.is_empty() && *x != ".").collect();
                files.insert(r.join("/").to_ascii_lowercase());
            }
        }
    }
    if !found || files.is_empty() { return None; }
    let mut dirs = HashSet::new();
    for f in &files { let mut p = f.as_str(); while let Some((a, _)) = p.rsplit_once('/') { dirs.insert(a.to_string()); p = a; } }
    Some((files, dirs))
}

// Dateien, die das Spiel bzw. die Xbox-App selbst mitbringen, aber nicht in der Liste stehen
fn ignored(rel_lc: &str) -> bool {
    matches!(rel_lc, "appxmanifest.xml" | "gamelaunchhelper.exe" | "smalllogo.ico" | "hash.manifest") || (rel_lc.starts_with("layout_") && rel_lc.ends_with(".xml") && !rel_lc.contains('/'))
}
// Steam-Fallback: typische Mod-Dateien
fn modlike(rel_lc: &str) -> bool {
    if rel_lc.starts_with("mediapc/") { return true; }
    let top = rel_lc.split('/').next().unwrap_or("");
    if rel_lc.contains('/') { return matches!(top, "reshade-shaders" | "reshade-presets" | "plugins" | "scripts" | "mods"); }
    matches!(top, "dinput8.dll" | "version.dll" | "winmm.dll" | "dxgi.dll" | "d3d11.dll" | "xinput1_3.dll" | "xinput1_4.dll" | "dsound.dll" | "winhttp.dll") || top.ends_with(".asi") || top.starts_with("reshade")
}
// Gruppe einer Mod-Datei: mediapc\Ordner, ReShade, sonst erster Pfadteil
fn group_of(rel: &str) -> String {
    let lc = rel.to_ascii_lowercase();
    let parts: Vec<&str> = rel.split('/').collect();
    if lc.starts_with("mediapc/") && parts.len() >= 2 { return format!("mediapc/{}", parts[1]); }
    if lc.starts_with("reshade") || lc.ends_with(".addon") || lc.ends_with(".addon64") || lc == "dxgi.dll" { return "ReShade".into(); }
    parts[0].to_string()
}
fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() { continue; }
        if ft.is_dir() { walk(&p, base, out); } else if let Ok(r) = p.strip_prefix(base) { out.push((rel_str(r), e.metadata().map(|m| m.len()).unwrap_or(0))); }
    }
}
fn aus_dir(s: &Shared, key: &str) -> PathBuf {
    let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    root(s).join("aus").join(safe)
}

// Alles, was gerade als Mod im Spielordner liegt (und was pausiert ist)
pub fn local(s: &Shared) -> Value {
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    if !game.is_dir() { return json!({"ok": false, "why": "Spielordner nicht gefunden"}); }
    let orig = originals(&game);
    let mut all = Vec::new(); walk(&game, &game, &mut all);
    let c = cfg(s);
    // Dateien, die das Dashboard selbst installiert hat → gehören zu dieser Nexus-Mod
    let mut owner: HashMap<String, u64> = HashMap::new();
    for m in c["installed"].as_array().cloned().unwrap_or_default() {
        for f in m["files"].as_array().cloned().unwrap_or_default() { owner.insert(f["rel"].as_str().unwrap_or("").replace('\\', "/").to_ascii_lowercase(), m["mod"].as_u64().unwrap_or(0)); }
    }
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (rel, size) in all {
        let lc = rel.to_ascii_lowercase();
        if ignored(&lc) { continue; }
        let is_mod = match &orig { Some((files, _)) => !files.contains(&lc), None => modlike(&lc) };
        if !is_mod { continue; }
        let key = match owner.get(&lc) { Some(id) => format!("nx:{id}"), None => group_of(&rel) };
        groups.entry(key).or_default().push(json!({"rel": rel, "size": size}));
    }
    let disabled = c["disabled"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<Value> = Vec::new();
    for m in c["installed"].as_array().cloned().unwrap_or_default() {   // Dashboard-Mods (auch wenn sie nur Originale ersetzen)
        let id = m["mod"].as_u64().unwrap_or(0); let key = format!("nx:{id}");
        let files = groups.remove(&key).unwrap_or_default();
        if disabled.iter().any(|d| d["key"] == json!(key)) { continue; }
        let size: u64 = files.iter().map(|f| f["size"].as_u64().unwrap_or(0)).sum();
        out.push(json!({"key": key, "state": "an", "dash": true, "imp": m["imp"], "src": m["src"], "mod": id, "name": m["name"], "pic": m["pic"], "version": m["version"], "files": files, "size": size, "time": m["time"]}));
    }
    for (key, files) in groups {
        let size: u64 = files.iter().map(|f| f["size"].as_u64().unwrap_or(0)).sum();
        out.push(json!({"key": key, "state": "an", "dash": false, "files": files, "size": size}));
    }
    for d in disabled { let mut d = d; d["state"] = json!("aus"); out.push(d); }
    json!({"ok": true, "layout": orig.is_some(), "dir": game.to_string_lossy(), "mods": out})
}

fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(d) = to.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    if std::fs::rename(from, to).is_ok() { return Ok(()); }
    std::fs::copy(from, to).map_err(|e| e.to_string())?;
    std::fs::remove_file(from).map_err(|e| { let _ = std::fs::remove_file(to); e.to_string() })
}
// leere Mod-Ordner aufräumen (nie Originalordner, nie mediapc selbst, nie den Spielordner)
fn prune(game: &Path, rel: &str, orig_dirs: Option<&HashSet<String>>) {
    let mut p = rel.to_string();
    while let Some((a, _)) = p.rsplit_once('/') {
        let lc = a.to_ascii_lowercase();
        let protected = match orig_dirs { Some(d) => d.contains(&lc), None => !lc.starts_with("mediapc/") };
        if lc == "mediapc" || protected { break; }
        if std::fs::remove_dir(game.join(a)).is_err() { break; }
        p = a.to_string();
    }
}
fn busy_msg(e: &str) -> String {
    let l = e.to_ascii_lowercase();
    if l.contains("os error 32") || l.contains("used by another") || l.contains("verwendet") { "Datei ist gesperrt – erst Forza schließen".into() } else { e.to_string() }
}

// Mod pausieren (on=false) oder wieder aktivieren (on=true)
pub fn toggle(s: &Shared, key: &str, on: bool) -> Value { let r = toggle_inner(s, key, on); rs_sync(s); r }
fn toggle_inner(s: &Shared, key: &str, on: bool) -> Value {
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    if !game.is_dir() { return json!({"ok": false, "why": "Spielordner nicht gefunden"}); }
    let orig = originals(&game);
    let is_orig = |lc: &str| orig.as_ref().map(|(f, _)| f.contains(lc)).unwrap_or(false);
    let mut c = cfg(s);
    let store = aus_dir(s, key);
    if on {
        let Some(i) = c["disabled"].as_array().and_then(|l| l.iter().position(|d| d["key"] == json!(key))) else { return json!({"ok": false, "why": "nicht pausiert"}) };
        let entry = c["disabled"][i].clone();
        for f in entry["files"].as_array().cloned().unwrap_or_default() {
            let rel = f["rel"].as_str().unwrap_or("").to_string();
            if rel.is_empty() || !safe_rel(Path::new(&rel)) { continue; }
            let from = store.join(&rel);
            if !from.is_file() { continue; }
            let to = game.join(&rel);
            if f["over"].as_bool().unwrap_or(false) {
                // ersetztes Original: Mod-Version wieder drüber (Original bleibt in der Sicherung)
                if let Err(e) = std::fs::copy(&from, &to) { return json!({"ok": false, "why": busy_msg(&e.to_string())}); }
                let _ = std::fs::remove_file(&from);
                continue;
            }
            if to.exists() { continue; }   // inzwischen liegt dort etwas anderes – nicht überschreiben
            if let Err(e) = move_file(&from, &to) { return json!({"ok": false, "why": busy_msg(&e)}); }
        }
        c["disabled"].as_array_mut().unwrap().remove(i);
        save(s, &c);
        let _ = std::fs::remove_dir_all(&store);
        log(format!("Mod aktiviert: {key}"));
        return json!({"ok": true});
    }
    // pausieren: aktuelle Dateien dieser Gruppe bestimmen
    if c["disabled"].as_array().map(|l| l.iter().any(|d| d["key"] == json!(key))).unwrap_or(false) { return json!({"ok": false, "why": "Davon gibt es schon eine pausierte Version – erst diese aktivieren oder löschen"}); }
    let cur = local(s);
    let Some(g) = cur["mods"].as_array().and_then(|a| a.iter().find(|m| m["key"] == json!(key) && m["state"] == "an")).cloned() else { return json!({"ok": false, "why": "Mod nicht gefunden"}) };
    let backup = g["mod"].as_u64().map(|id| root(s).join("backup").join(id.to_string()));
    let mut files: Vec<Value> = g["files"].as_array().cloned().unwrap_or_default();
    // Dashboard-Mods: auch ersetzte Originale (das Original steht in der Sicherung)
    if g["dash"].as_bool().unwrap_or(false) {
        if let Some(m) = c["installed"].as_array().and_then(|a| a.iter().find(|m| m["mod"] == g["mod"])) {
            for f in m["files"].as_array().cloned().unwrap_or_default() {
                if !f["new"].as_bool().unwrap_or(true) { files.push(json!({"rel": f["rel"].as_str().unwrap_or("").replace('\\', "/"), "over": true})); }
            }
        }
    }
    let mut moved: Vec<Value> = Vec::new();
    for f in &files {
        let rel = f["rel"].as_str().unwrap_or("").to_string();
        if rel.is_empty() || !safe_rel(Path::new(&rel)) { continue; }
        let from = game.join(&rel);
        if !from.is_file() { continue; }
        if f["over"].as_bool().unwrap_or(false) {
            let Some(b) = backup.as_ref().map(|b| b.join(&rel)).filter(|b| b.is_file()) else { continue };
            let to = store.join(&rel);
            if let Some(d) = to.parent() { let _ = std::fs::create_dir_all(d); }
            if let Err(e) = std::fs::copy(&from, &to) { return json!({"ok": false, "why": e.to_string()}); }
            if let Err(e) = std::fs::copy(&b, &from) { return json!({"ok": false, "why": busy_msg(&e.to_string())}); }
            moved.push(json!({"rel": rel, "over": true, "size": f["size"]}));
            continue;
        }
        if is_orig(&rel.to_ascii_lowercase()) { continue; }   // Sicherheitsnetz: nie Originaldateien
        if let Err(e) = move_file(&from, &store.join(&rel)) {
            for m in &moved { let r = m["rel"].as_str().unwrap_or(""); if !m["over"].as_bool().unwrap_or(false) { let _ = move_file(&store.join(r), &game.join(r)); } }
            return json!({"ok": false, "why": busy_msg(&e)});
        }
        prune(&game, &rel, orig.as_ref().map(|(_, d)| d));
        moved.push(json!({"rel": rel, "size": f["size"]}));
    }
    if moved.is_empty() { return json!({"ok": false, "why": "Keine Dateien zum Pausieren"}); }
    let size: u64 = moved.iter().map(|f| f["size"].as_u64().unwrap_or(0)).sum();
    let entry = json!({"key": key, "dash": g["dash"], "imp": g["imp"], "mod": g["mod"], "name": g["name"], "pic": g["pic"], "version": g["version"], "files": moved, "size": size, "time": now_ms()});
    if !c["disabled"].is_array() { c["disabled"] = json!([]); }
    c["disabled"].as_array_mut().unwrap().push(entry);
    save(s, &c);
    log(format!("Mod pausiert: {key}"));
    json!({"ok": true})
}

// in den Windows-Papierkorb
fn recycle(paths: &[PathBuf]) -> bool {
    if paths.is_empty() { return true; }
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Shell::{SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT};
        let mut buf: Vec<u16> = Vec::new();
        for p in paths { buf.extend(p.as_os_str().to_string_lossy().encode_utf16()); buf.push(0); }
        buf.push(0);
        let mut op = SHFILEOPSTRUCTW::default();
        op.wFunc = FO_DELETE;
        op.pFrom = windows::core::PCWSTR(buf.as_ptr());
        op.fFlags = (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_NOERRORUI.0 | FOF_SILENT.0) as u16;
        return SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool();
    }
    #[allow(unreachable_code)]
    { paths.iter().all(|p| std::fs::remove_file(p).is_ok()) }
}

// Mod löschen: Dashboard-Mods normal deinstallieren (Originale zurück), eigene Mods in den Papierkorb
pub fn remove(s: &Shared, key: &str) -> Value { let r = remove_inner(s, key); rs_sync(s); r }
fn remove_inner(s: &Shared, key: &str) -> Value {
    let mut c = cfg(s);
    // pausierte Mod: liegt im Datenordner
    if let Some(i) = c["disabled"].as_array().and_then(|a| a.iter().position(|d| d["key"] == json!(key))) {
        let d = c["disabled"].as_array_mut().unwrap().remove(i);
        let store = aus_dir(s, key);
        let ok = !store.exists() || recycle(&[store.clone()]);
        if !ok { return json!({"ok": false, "why": "Konnte nicht gelöscht werden"}); }
        if let Some(id) = d["mod"].as_u64().filter(|_| d["dash"].as_bool().unwrap_or(false)) {
            // Dashboard-Mod: Originale liegen schon wieder im Spiel → Eintrag und Sicherung weg
            if let Some(a) = c["installed"].as_array_mut() { a.retain(|m| m["mod"].as_u64() != Some(id)); }
            let _ = std::fs::remove_dir_all(root(s).join("backup").join(id.to_string()));
        }
        save(s, &c);
        log(format!("Pausierte Mod gelöscht: {key}"));
        return json!({"ok": true});
    }
    if let Some(id) = key.strip_prefix("nx:").and_then(|x| x.parse::<u64>().ok()) { return uninstall(s, id); }
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    if !game.is_dir() { return json!({"ok": false, "why": "Spielordner nicht gefunden"}); }
    let orig = originals(&game);
    let cur = local(s);
    let Some(g) = cur["mods"].as_array().and_then(|a| a.iter().find(|m| m["key"] == json!(key) && m["state"] == "an")).cloned() else { return json!({"ok": false, "why": "Mod nicht gefunden"}) };
    let mut paths = Vec::new();
    let mut rels = Vec::new();
    for f in g["files"].as_array().cloned().unwrap_or_default() {
        let rel = f["rel"].as_str().unwrap_or("").to_string();
        if rel.is_empty() || !safe_rel(Path::new(&rel)) { continue; }
        if orig.as_ref().map(|(o, _)| o.contains(&rel.to_ascii_lowercase())).unwrap_or(false) { continue; }
        paths.push(game.join(&rel)); rels.push(rel);
    }
    if !recycle(&paths) { return json!({"ok": false, "why": "Löschen fehlgeschlagen – erst Forza schließen"}); }
    for r in &rels { prune(&game, r, orig.as_ref().map(|(_, d)| d)); }
    log(format!("Mod gelöscht (Papierkorb): {key}"));
    json!({"ok": true})
}

// ---------------------------------------------------------------------------------------------
// Forza starten (Knopf in der Leiste, solange keine Daten kommen)
// Xbox-App/MS Store: über die App-ID (shell:AppsFolder\Paket!App), Steam: steam://rungameid/…, sonst die exe.
// ---------------------------------------------------------------------------------------------
fn publisher_id(publisher: &str) -> String {
    use sha2::{Digest, Sha256};
    let utf16: Vec<u8> = publisher.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let h = Sha256::digest(&utf16);
    let mut bits = String::new();
    for b in &h[..8] { bits.push_str(&format!("{b:08b}")); }
    bits.push('0');
    const A: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    bits.as_bytes().chunks(5).map(|c| A[usize::from_str_radix(std::str::from_utf8(c).unwrap_or("0"), 2).unwrap_or(0)] as char).collect()
}
fn xml_attr(x: &str, tag: &str, attr: &str) -> Option<String> {
    let i = x.find(&format!("<{tag} "))?;
    let t = &x[i..i + x[i..].find('>')?];
    t.split_once(&format!(" {attr}=\"")).and_then(|(_, r)| r.split_once('"')).map(|(v, _)| v.to_string())
}
pub fn start_game(s: &Shared) -> Value {
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    if !game.is_dir() { return json!({"ok": false, "why": "Spielordner nicht gefunden – in Mods → Einrichtung festlegen"}); }
    // Xbox-App / Microsoft Store
    if let Ok(x) = std::fs::read_to_string(game.join("appxmanifest.xml")) {
        if let (Some(name), Some(publ), Some(app)) = (xml_attr(&x, "Identity", "Name"), xml_attr(&x, "Identity", "Publisher"), xml_attr(&x, "Application", "Id")) {
            let aumid = format!("{}_{}!{}", name, publisher_id(&publ), app);
            let ok = std::process::Command::new("explorer.exe").arg(format!("shell:AppsFolder\\{aumid}")).spawn().is_ok();
            log(format!("Forza starten: {aumid} ({})", if ok { "ok" } else { "Fehler" }));
            return json!({"ok": ok, "how": "xbox"});
        }
    }
    // Steam: App-ID aus appmanifest_*.acf mit passendem installdir
    let folder = game.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    if let Some(apps) = game.parent().and_then(|p| p.parent()).filter(|p| p.file_name().map(|f| f.eq_ignore_ascii_case("steamapps")).unwrap_or(false)) {
        for e in std::fs::read_dir(apps).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) { continue; }
            let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
            let val = |k: &str| t.lines().find_map(|l| { let l = l.trim(); l.strip_prefix(&format!("\"{k}\"")).map(|r| r.trim().trim_matches('"').to_string()) });
            if val("installdir").map(|d| d.eq_ignore_ascii_case(&folder)).unwrap_or(false) {
                if let Some(id) = val("appid").filter(|i| i.chars().all(|c| c.is_ascii_digit())) {
                    let ok = crate::util::no_window(std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &format!("steam://rungameid/{id}")])).spawn().is_ok();
                    log(format!("Forza starten über Steam {id}"));
                    return json!({"ok": ok, "how": "steam"});
                }
            }
        }
    }
    // sonst direkt die exe
    for n in ["ForzaHorizon6.exe", "forzahorizon6.exe"] {
        let p = game.join(n);
        if p.is_file() { let ok = std::process::Command::new(&p).current_dir(&game).spawn().is_ok(); return json!({"ok": ok, "how": "exe"}); }
    }
    json!({"ok": false, "why": "Spiel nicht gefunden"})
}

// ---------------------------------------------------------------------------------------------
// Mod aus einer heruntergeladenen Datei installieren (ZIP, 7z, RAR): Das Dashboard findet selbst heraus, wohin die
// Dateien im Spiel gehören, zeigt eine Vorschau, installiert mit Sicherung und führt die Mod im Mod-Manager.
// Erkennung je Datei:
//   1. Pfad enthält „mediapc“ → ab dort nach mediapc\…        2. Pfad enthält „media“ → ab dort nach media\…
//   3. Ordnername wie ein Bereich des Spiels (Camera, UI, Sky, audio, cars …) → mediapc\<Bereich>\…
//   4. lose Datei, deren Name in einem Spielarchiv vorkommt (z. B. CameraPhysics.xml in Camera.zip) → mediapc\Camera\…
//   5. Programmdateien (DLL, ASI, ReShade) → Spielordner
//   Alles andere (Liesmich, Bilder) bleibt draußen. Mehrere gleichwertige Ordner = Varianten zum Auswählen.
// ---------------------------------------------------------------------------------------------
static MEDIA_IX: std::sync::Mutex<Option<(String, Arc<MediaIx>)>> = std::sync::Mutex::new(None);
struct MediaIx {
    areas: HashMap<String, String>,          // klein → richtige Schreibweise (Camera, UI, audio …)
    names: HashMap<String, Vec<String>>,     // Dateiname klein → ["Camera/CameraPhysics.xml", …]
}
fn media_ix(game: &Path) -> Arc<MediaIx> {
    let key = game.to_string_lossy().to_string();
    if let Some((k, ix)) = MEDIA_IX.lock().unwrap().as_ref() { if *k == key { return ix.clone(); } }
    let mut areas = HashMap::new();
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    let media = game.join("media");
    for e in std::fs::read_dir(&media).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        let p = e.path();
        if p.is_dir() { areas.insert(n.to_ascii_lowercase(), n); continue; }
        let Some(base) = n.strip_suffix(".zip").or_else(|| n.strip_suffix(".ZIP")) else { continue };
        areas.insert(base.to_ascii_lowercase(), base.to_string());
        let Ok(f) = std::fs::File::open(&p) else { continue };
        let Ok(z) = zip::ZipArchive::new(f) else { continue };
        for inner in z.file_names() {
            if inner.ends_with('/') { continue; }
            let fname = inner.rsplit('/').next().unwrap_or(inner).to_ascii_lowercase();
            names.entry(fname).or_default().push(format!("{base}/{inner}"));
        }
    }
    let ix = Arc::new(MediaIx { areas, names });
    *MEDIA_IX.lock().unwrap() = Some((key, ix.clone()));
    ix
}

fn is_junk(rel_lc: &str) -> bool {
    rel_lc.split('/').any(|p| p == "__macosx" || p == ".ds_store" || p == "thumbs.db" || p == "desktop.ini")
}
fn is_root_file(name_lc: &str) -> bool {
    name_lc.ends_with(".dll") || name_lc.ends_with(".asi") || name_lc.ends_with(".addon") || name_lc.ends_with(".addon64") || name_lc.starts_with("reshade")
}
// Ziel einer Datei im Spiel: (Varianten-Wurzel im Archiv, Zielpfad im Spielordner) oder None = gehört nicht ins Spiel
fn place(rel: &str, ix: &MediaIx) -> Option<(String, String)> {
    let parts: Vec<&str> = rel.split('/').collect();
    let lc: Vec<String> = parts.iter().map(|p| p.to_ascii_lowercase()).collect();
    let join = |a: &[&str]| a.join("/");
    let root_of = |i: usize| if i == 0 { String::new() } else { join(&parts[..i]) + "/" };
    // 1./2. ausdrücklich mediapc bzw. media
    if let Some(i) = lc.iter().position(|p| p == "mediapc") { if i + 1 < parts.len() { return Some((root_of(i), format!("mediapc/{}", join(&parts[i + 1..])))); } }
    if let Some(i) = lc.iter().position(|p| p == "media") { if i + 1 < parts.len() { return Some((root_of(i), format!("media/{}", join(&parts[i + 1..])))); } }
    let name = lc.last().cloned().unwrap_or_default();
    // 5. Programmdateien und ReShade
    if let Some(i) = lc.iter().position(|p| p == "reshade-shaders" || p == "reshade-presets") { return Some((root_of(i), join(&parts[i..]))); }
    if is_root_file(&name) { return Some((root_of(parts.len() - 1), parts.last().unwrap().to_string())); }
    // 3. Ordner heißt wie ein Bereich des Spiels (letzter Treffer zählt, z. B. „Variante A/Camera/…“)
    if let Some(i) = (0..parts.len() - 1).rev().find(|&i| ix.areas.contains_key(&lc[i])) {
        let area = &ix.areas[&lc[i]];
        return Some((root_of(i), format!("mediapc/{area}/{}", join(&parts[i + 1..]))));
    }
    // 4. lose Datei: Name in einem Spielarchiv suchen; passt der Ordnerpfad dahinter, umso besser
    if let Some(hits) = ix.names.get(&name) {
        let best = hits.iter().max_by_key(|h| {
            let hp: Vec<String> = h.to_ascii_lowercase().split('/').map(String::from).collect();
            lc.iter().rev().zip(hp.iter().rev()).take_while(|(a, b)| a == b).count()
        })?;
        let hp: Vec<&str> = best.split('/').collect();
        let m = lc.iter().rev().zip(hp.iter().rev()).take_while(|(a, b)| **a == b.to_ascii_lowercase()).count().max(1);
        return Some((root_of(parts.len() - m), format!("mediapc/{best}")));
    }
    None
}

fn import_dir(s: &Shared, id: &str) -> PathBuf { root(s).join("import").join(id.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>()) }

fn unpack_any(arch: &Path, ext: &str, dest: &Path) -> Result<(), String> {
    match ext {
        "zip" => unzip(arch, dest),
        "7z" => sevenz_rust::decompress_file(arch, dest).map_err(|e| format!("7z: {e}")),
        // RAR und anderes: Windows bringt dafür tar mit (Windows 11)
        _ => {
            let ok = crate::util::no_window(std::process::Command::new("tar").arg("-xf").arg(arch).arg("-C").arg(dest)).status().map(|s| s.success()).unwrap_or(false);
            if ok { Ok(()) } else { Err("Dieses Archiv lässt sich nicht entpacken – bitte als ZIP oder 7z versuchen".into()) }
        }
    }
}

// Schritt 1: Datei annehmen, entpacken, untersuchen → Vorschau mit Varianten
pub fn import_stage(s: &Shared, filename: &str, data: &[u8]) -> Value {
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    if !game.is_dir() { return json!({"ok": false, "why": "Erst den Spielordner festlegen (Mods → Einrichtung)"}); }
    if data.is_empty() { return json!({"ok": false, "why": "Leere Datei"}); }
    let fname: String = filename.chars().filter(|c| !"\\/:*?\"<>|".contains(*c)).take(120).collect();
    let ext = fname.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let is_rs = ext == "exe" && fname.to_ascii_lowercase().contains("reshade");
    if !matches!(ext.as_str(), "zip" | "7z" | "rar") && !is_rs { return json!({"ok": false, "why": "Bitte eine ZIP-, 7z- oder RAR-Datei wählen (oder das ReShade-Setup von reshade.me)"}); }
    let id = crate::util::random_hex(6);
    let dir = import_dir(s, &id);
    let x = dir.join("x");
    if std::fs::create_dir_all(&x).is_err() { return json!({"ok": false, "why": "Datenordner nicht beschreibbar"}); }
    let arch = dir.join(format!("archiv.{ext}"));
    if let Err(e) = std::fs::write(&arch, data) { return json!({"ok": false, "why": e.to_string()}); }
    if is_rs { return reshade_stage(&game, &dir, &x, &arch, &fname, &id); }
    if let Err(e) = unpack_any(&arch, &ext, &x) { let _ = std::fs::remove_dir_all(&dir); return json!({"ok": false, "why": e}); }
    let mut all = Vec::new(); walk(&x, &x, &mut all);
    let ix = media_ix(&game);
    // Auto-Mods: der Inhalt eines Autos (…\<Auto>.carbin) gehört als <Auto>.zip nach mediapc\cars – nicht entpackt
    // (so beschreiben es die Autoren: „ZIP nicht entpacken, nach mediapc\cars, Name wie das Auto“)
    let cars_dir = game.join("media").join("cars");
    let car_names: Vec<String> = std::fs::read_dir(&cars_dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    let mut car_roots: Vec<(String, String)> = Vec::new();
    for (rel, _) in &all {
        let lc = rel.to_ascii_lowercase();
        if is_junk(&lc) || !lc.ends_with(".carbin") || lc.split('/').any(|p| p == "mediapc" || p == "media") { continue; }
        let (d, fnm) = rel.rsplit_once('/').map(|(d, f)| (format!("{d}/"), f.to_string())).unwrap_or((String::new(), rel.clone()));
        let stem = fnm[..fnm.len() - 7].to_string();
        if let Some(real) = car_names.iter().find(|n| n.eq_ignore_ascii_case(&format!("{stem}.zip"))) {
            if !car_roots.iter().any(|(r, _)| *r == d) { car_roots.push((d, real[..real.len() - 4].to_string())); }
        }
    }
    let mut car_size: BTreeMap<String, u64> = BTreeMap::new();
    // Pakete mit eigenem Programmteil (DLL/ASI): alles daneben gehört unverändert in den Spielordner (z. B. „version.dll“ + „spotify-radio…“)
    let mut plug_roots: Vec<String> = all.iter().filter_map(|(rel, _)| {
        let lc = rel.to_ascii_lowercase();
        if is_junk(&lc) || !(lc.ends_with(".dll") || lc.ends_with(".asi")) || lc.split('/').any(|p| p == "mediapc" || p == "media") { return None; }
        Some(rel.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default())
    }).collect();
    plug_roots.sort(); plug_roots.dedup();
    let doc = |n: &str| [".txt", ".md", ".pdf", ".url", ".html", ".htm", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".nfo"].iter().any(|e| n.ends_with(e));
    let mut variants: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut skipped = Vec::new();
    for (rel, size) in &all {
        let lc = rel.to_ascii_lowercase();
        if is_junk(&lc) { continue; }
        if let Some((r, _)) = car_roots.iter().filter(|(r, _)| lc.starts_with(&r.to_ascii_lowercase())).max_by_key(|(r, _)| r.len()) { *car_size.entry(r.clone()).or_default() += size; continue; }
        let explicit = lc.split('/').any(|p| p == "mediapc" || p == "media");
        let plug = if explicit { None } else { plug_roots.iter().filter(|r| lc.starts_with(&r.to_ascii_lowercase())).max_by_key(|r| r.len()).cloned() };
        // ReShade-Preset (eine .ini mit „Techniques=“) → reshade-presets\
        let preset = !explicit && lc.ends_with(".ini") && !lc.ends_with("reshade.ini") && is_preset_file(&x.join(rel));
        let placed = if preset { Some((rel.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default(), format!("reshade-presets/{}", rel.rsplit('/').next().unwrap_or(rel)))) } else { match plug {
            Some(r) => { let rest = &rel[r.len()..]; if !rest.contains('/') && doc(&rest.to_ascii_lowercase()) { None } else { Some((r.clone(), rest.to_string())) } }
            None => place(rel, &ix),
        } };
        match placed {
            Some((root, to)) if safe_rel(Path::new(&to)) => variants.entry(root).or_default().push(json!({"from": rel, "to": to, "size": size, "exists": game.join(&to).exists()})),
            _ => skipped.push(json!(rel)),
        }
    }
    for (r, car) in &car_roots {
        let to = format!("mediapc/cars/{car}.zip");
        let parent = r.trim_end_matches('/').rsplit_once('/').map(|(p, _)| format!("{p}/")).unwrap_or_default();
        variants.entry(parent).or_default().push(json!({"from": r, "to": to, "size": car_size.get(r).copied().unwrap_or(0), "exists": game.join(&to).exists(), "pack": true, "car": car}));
    }
    if variants.is_empty() { let _ = std::fs::remove_dir_all(&dir); return json!({"ok": false, "why": "In dieser Datei wurde nichts gefunden, das ins Spiel gehört", "skipped": skipped}); }
    // Wurzeln, die nur Unterordner einer anderen sind, zur übergeordneten zählen (ein Paket mit Unterordnern = eine Variante)
    let roots: Vec<String> = variants.keys().cloned().collect();
    for r in &roots { if let Some(p) = roots.iter().filter(|p| p.as_str() != r.as_str() && r.starts_with(p.as_str()) && !p.is_empty()).max_by_key(|p| p.len()) { if variants.contains_key(p) { if let Some(v) = variants.remove(r) { if let Some(t) = variants.get_mut(p) { t.extend(v); } } } } }
    if variants.len() > 1 {
        let sets: Vec<HashSet<String>> = variants.values().map(|f| f.iter().filter_map(|x| x["to"].as_str().map(|t| t.to_ascii_lowercase())).collect()).collect();
        let overlap = (0..sets.len()).any(|i| (i + 1..sets.len()).any(|j| !sets[i].is_disjoint(&sets[j])));
        if !overlap { let merged: Vec<Value> = variants.values().flatten().cloned().collect(); variants.clear(); variants.insert(String::new(), merged); }
    }
    // Name: Nexus hängt „<Nummer> <Version> <Datum>T… <Kennung>“ an den Dateinamen – das abschneiden
    let stem = fname.rsplit_once('.').map(|(a, _)| a).unwrap_or(&fname).to_string();
    let toks: Vec<&str> = stem.split(' ').collect();
    let cut = (1..toks.len()).find(|&i| toks[i].chars().all(|c| c.is_ascii_digit()) && toks[i..].iter().any(|t| t.len() > 10 && t.as_bytes()[..4].iter().all(|b| b.is_ascii_digit()) && t.contains('T'))).unwrap_or(toks.len());
    let name = if car_roots.len() == 1 { format!("Auto: {}", car_roots[0].1.replace('_', " ")) } else { toks[..cut].join(" ").replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ") };
    let list: Vec<Value> = variants.iter().map(|(r, f)| {
        let label = r.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
        json!({"root": r, "name": if label.is_empty() { "Standard".to_string() } else { label }, "files": f, "n": f.len(), "size": f.iter().map(|x| x["size"].as_u64().unwrap_or(0)).sum::<u64>()})
    }).collect();
    let plan = json!({"id": id, "name": name, "file": fname, "variants": list, "skipped": skipped, "t": now_ms()});
    write_json(&dir.join("plan.json"), &plan);
    let mut out = plan.clone(); out["ok"] = json!(true);
    out
}

// Schritt 2: gewählte Variante installieren (mit Sicherung) und im Mod-Manager eintragen
pub fn import_apply(s: &Shared, id: &str, variant: &str, name: &str) -> Value {
    let dir = import_dir(s, id);
    let Some(plan) = read_json(&dir.join("plan.json")) else { return json!({"ok": false, "why": "Import abgelaufen – Datei bitte nochmal wählen"}) };
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    if !game.is_dir() { return json!({"ok": false, "why": "Spielordner nicht gefunden"}); }
    let Some(v) = plan["variants"].as_array().and_then(|a| a.iter().find(|v| v["root"].as_str() == Some(variant)).or_else(|| if a.len() == 1 { a.first() } else { None })) else { return json!({"ok": false, "why": "Bitte eine Variante wählen"}) };
    let mod_id = 9_000_000_000_000u64 + now_ms() % 1_000_000_000_000;
    let backup = root(s).join("backup").join(mod_id.to_string());
    let mut files = Vec::new();
    for f in v["files"].as_array().cloned().unwrap_or_default() {
        let (Some(from), Some(to)) = (f["from"].as_str(), f["to"].as_str()) else { continue };
        let pack = f["pack"].as_bool().unwrap_or(false);
        if !safe_rel(Path::new(to)) || (!from.is_empty() && !safe_rel(Path::new(from.trim_end_matches('/')))) { continue; }
        let src = dir.join("x").join(from);
        let dst = game.join(to);
        let existed = dst.exists();
        if pack {
            if existed { let b = backup.join(to); if let Some(d) = b.parent() { let _ = std::fs::create_dir_all(d); } if !b.exists() { let _ = std::fs::copy(&dst, &b); } }
            if let Some(d) = dst.parent() { if let Err(e) = std::fs::create_dir_all(d) { return json!({"ok": false, "why": format!("Spielordner: {e}")}); } }
            // war die Datei schon eine passende ZIP (Auto direkt drin), unverändert übernehmen – sonst neu packen
            let orig = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("archiv.zip")).unwrap_or(false));
            let r = match orig { Some(o) if from.is_empty() => std::fs::copy(&o, &dst).map(|_| ()).map_err(|e| e.to_string()), _ => zip_dir(&src, &dst) };
            if let Err(e) = r { return json!({"ok": false, "why": if e.contains("os error 32") { "Datei ist gesperrt – erst Forza schließen".to_string() } else { format!("Auto-Paket: {e}") }}); }
            files.push(json!({"rel": to, "new": !existed}));
            continue;
        }
        if existed && f["keep"].as_bool().unwrap_or(false) { continue; }
        if existed { let b = backup.join(to); if let Some(d) = b.parent() { let _ = std::fs::create_dir_all(d); } if !b.exists() { if let Err(e) = std::fs::copy(&dst, &b) { return json!({"ok": false, "why": format!("Sicherung: {e}")}); } } }
        if let Some(d) = dst.parent() { if let Err(e) = std::fs::create_dir_all(d) { return json!({"ok": false, "why": format!("Spielordner: {e}")}); } }
        if let Err(e) = std::fs::copy(&src, &dst) { return json!({"ok": false, "why": if e.raw_os_error() == Some(32) { "Datei ist gesperrt – erst Forza schließen".to_string() } else { format!("Kopieren: {e}") }}); }
        files.push(json!({"rel": to, "new": !existed}));
    }
    let fx_note = rs_after(&game, &dir, &mut files);
    let nm = take_name(if name.trim().is_empty() { plan["name"].as_str().unwrap_or("Mod") } else { name });
    let vname = if plan["variants"].as_array().map(|a| a.len()).unwrap_or(0) > 1 { v["name"].as_str().unwrap_or("").to_string() } else { String::new() };
    let mut c = cfg(s);
    if !c["installed"].is_array() { c["installed"] = json!([]); }
    c["installed"].as_array_mut().unwrap().push(json!({"mod": mod_id, "file": 0, "imp": true, "name": nm, "version": vname, "src": plan["file"], "pic": null, "time": now_ms(), "files": files}));
    save(s, &c);
    let _ = std::fs::remove_dir_all(&dir);
    rs_sync(s);
    log(format!("Mod importiert: {nm} ({} Dateien)", files.len()));
    json!({"ok": true, "n": files.len(), "key": format!("nx:{mod_id}"), "note": fx_note})
}
fn take_name(s: &str) -> String { s.trim().chars().take(80).collect() }

pub fn import_cancel(s: &Shared, id: &str) -> Value { let _ = std::fs::remove_dir_all(import_dir(s, id)); json!({"ok": true}) }

// installierte Mod umbenennen (vor allem für importierte)
pub fn rename(s: &Shared, mod_id: u64, name: &str) -> Value {
    let mut c = cfg(s);
    let Some(m) = c["installed"].as_array_mut().and_then(|a| a.iter_mut().find(|m| m["mod"].as_u64() == Some(mod_id))) else { return json!({"ok": false}) };
    m["name"] = json!(take_name(name));
    save(s, &c);
    json!({"ok": true})
}

// Ordner als ZIP packen (für Auto-Mods: mediapc\cars\<Auto>.zip)
fn zip_dir(src: &Path, dst: &Path) -> Result<(), String> {
    use std::io::Write as _;
    let tmp = dst.with_extension("zip.neu");
    let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(f);
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut all = Vec::new(); walk(src, src, &mut all);
    for (rel, _) in all {
        if is_junk(&rel.to_ascii_lowercase()) { continue; }
        z.start_file(rel.as_str(), opt).map_err(|e| e.to_string())?;
        let data = std::fs::read(src.join(&rel)).map_err(|e| e.to_string())?;
        z.write_all(&data).map_err(|e| e.to_string())?;
    }
    z.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dst).or_else(|_| { std::fs::copy(&tmp, dst).map(|_| ()).and_then(|_| std::fs::remove_file(&tmp)) }).map_err(|e| e.to_string())
}

// ---- ReShade: das offizielle Setup (reshade.me) einlesen und für Forza einrichten ----
// Das Setup wird NICHT ausgeführt: ReShade hängt seine Programmdateien als ZIP ans Setup an – nur die werden gelesen.
// Forza Horizon 6 nutzt DirectX 12 → ReShade64.dll kommt als dxgi.dll neben forzahorizon6.exe. Dazu eine ReShade.ini
// (Effekt-Ordner, Menü-Taste Pos1) und die Standard-Effekte von GitHub (crosire/reshade-shaders, wie das Setup selbst).
const RS_INI: &str = "[GENERAL]\r\nEffectSearchPaths=.\\reshade-shaders\\Shaders\\**\r\nTextureSearchPaths=.\\reshade-shaders\\Textures\\**\r\nPresetPath=.\\ReShadePreset.ini\r\nPerformanceMode=0\r\n\r\n[INPUT]\r\nKeyOverlay=36,0,0,0\r\n\r\n[OVERLAY]\r\nTutorialProgress=4\r\n";
const RS_FX: &str = "https://github.com/crosire/reshade-shaders/archive/refs/heads/slim.zip";

fn reshade_stage(game: &Path, dir: &Path, x: &Path, arch: &Path, fname: &str, id: &str) -> Value {
    let fail = |why: &str| { let _ = std::fs::remove_dir_all(dir); json!({"ok": false, "why": why}) };
    let pay = x.join("setup");
    let _ = std::fs::create_dir_all(&pay);
    if unzip(arch, &pay).is_err() {
        let ok = crate::util::no_window(std::process::Command::new("tar").arg("-xf").arg(arch).arg("-C").arg(&pay)).status().map(|s| s.success()).unwrap_or(false);
        if !ok { return fail("Das ReShade-Setup ließ sich nicht lesen – bitte die aktuelle Version von reshade.me nehmen"); }
    }
    let mut all = Vec::new(); walk(x, x, &mut all);
    let Some((dll, dsize)) = all.iter().find(|(r, _)| r.to_ascii_lowercase().ends_with("reshade64.dll")).cloned() else { return fail("In dieser Datei steckt kein ReShade (64 Bit) – bitte das Setup von reshade.me nehmen"); };
    let mut files = vec![json!({"from": dll, "to": "dxgi.dll", "size": dsize, "exists": game.join("dxgi.dll").exists()})];
    // Einstellungen: nur anlegen, wenn noch keine da ist
    let ini = x.join("fd").join("ReShade.ini");
    let _ = std::fs::create_dir_all(ini.parent().unwrap());
    if std::fs::write(&ini, RS_INI).is_ok() { files.push(json!({"from": "fd/ReShade.ini", "to": "ReShade.ini", "size": RS_INI.len(), "exists": game.join("ReShade.ini").exists(), "keep": true})); }
    // Standard-Effekte
    let mut note = String::new();
    let fx = x.join("fx");
    let got = (|| -> Result<(), String> {
        let resp = agent(60).get(RS_FX).call().map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        resp.into_reader().take(60 << 20).read_to_end(&mut data).map_err(|e| e.to_string())?;
        let z = dir.join("fx.zip"); std::fs::write(&z, &data).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&fx).map_err(|e| e.to_string())?;
        unzip(&z, &fx)
    })();
    match got {
        Ok(()) => {
            let mut fxs = Vec::new(); walk(&fx, x, &mut fxs);
            for (rel, size) in fxs {
                let parts: Vec<&str> = rel.split('/').collect();
                let Some(i) = parts.iter().position(|p| p.eq_ignore_ascii_case("Shaders") || p.eq_ignore_ascii_case("Textures")) else { continue };
                let to = format!("reshade-shaders/{}", parts[i..].join("/"));
                if safe_rel(Path::new(&to)) { files.push(json!({"from": rel, "to": to, "size": size, "exists": game.join(&to).exists()})); }
            }
        }
        Err(e) => { log(format!("ReShade-Effekte nicht geladen: {e}")); note = "Die Standard-Effekte konnten nicht geladen werden (keine Verbindung zu GitHub). ReShade läuft trotzdem – Effekte lassen sich später nachinstallieren.".into(); }
    }
    // Name mit Version aus dem Dateinamen („ReShade_Setup_6.3.3_Addon.exe“)
    let ver: String = fname.split(|c: char| c == '_' || c == ' ').find(|t| t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && t.contains('.')).map(|t| t.trim_end_matches(".exe").to_string()).unwrap_or_default();
    let name = if ver.is_empty() { "ReShade".to_string() } else { format!("ReShade {ver}") };
    let n = files.len(); let size: u64 = files.iter().map(|f| f["size"].as_u64().unwrap_or(0)).sum();
    let plan = json!({"id": id, "name": name, "file": fname, "rs": true, "note": note, "skipped": [], "t": now_ms(),
        "variants": [{"root": "", "name": "Standard", "files": files, "n": n, "size": size}]});
    write_json(&dir.join("plan.json"), &plan);
    let mut out = plan.clone(); out["ok"] = json!(true);
    out
}

// ---- ReShade-Presets: erkennen, Effekte nachladen, aktivieren ----
const RS_PKGS: &str = "https://raw.githubusercontent.com/crosire/reshade-shaders/list/EffectPackages.ini";
fn is_preset_file(p: &Path) -> bool {
    std::fs::read(p).map(|b| b.len() < 2 << 20 && String::from_utf8_lossy(&b).lines().any(|l| l.trim_start().starts_with("Techniques="))).unwrap_or(false)
}
fn ini_get(txt: &str, key: &str) -> Option<String> {
    txt.lines().find_map(|l| l.trim().strip_prefix(&format!("{key}=")).map(|v| v.trim().to_string()))
}
// Effekt-Dateien, die ein Preset nutzt („Techniques=Name@Datei.fx,…“), die im Spielordner noch fehlen
fn rs_missing(game: &Path, preset: &Path) -> Vec<String> {
    let txt = std::fs::read(preset).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let mut need: Vec<String> = ini_get(&txt, "Techniques").unwrap_or_default().split(',').filter_map(|t| t.split_once('@').map(|(_, f)| f.trim().to_ascii_lowercase())).filter(|f| !f.is_empty()).collect();
    need.sort(); need.dedup();
    let mut have = Vec::new(); walk(&game.join("reshade-shaders"), &game.join("reshade-shaders"), &mut have);
    let have: HashSet<String> = have.into_iter().map(|(r, _)| r.rsplit('/').next().unwrap_or("").to_ascii_lowercase()).collect();
    need.into_iter().filter(|f| !have.contains(f)).collect()
}
// fehlende Effekte aus der offiziellen ReShade-Paketliste nachladen (wie das ReShade-Setup); neue Dateien kommen zur Mod dazu
fn rs_after(game: &Path, tmp: &Path, files: &mut Vec<Value>) -> String {
    let presets: Vec<PathBuf> = files.iter().filter_map(|f| f["rel"].as_str()).filter(|r| { let l = r.to_ascii_lowercase(); l.starts_with("reshade-presets/") && l.ends_with(".ini") }).map(|r| game.join(r)).collect();
    if presets.is_empty() { return String::new(); }
    let mut missing: Vec<String> = presets.iter().flat_map(|p| rs_missing(game, p)).collect();
    missing.sort(); missing.dedup();
    if missing.is_empty() { return String::new(); }
    let list = match agent(30).get(RS_PKGS).call().ok().and_then(|r| r.into_string().ok()) { Some(t) => t, None => return format!("Für das Preset fehlen Effekte ({}), die Paketliste war nicht erreichbar.", missing.join(", ")) };
    // Abschnitte [00], [01] … lesen
    let mut pkgs: Vec<std::collections::HashMap<String, String>> = Vec::new();
    for l in list.lines() { let l = l.trim(); if l.starts_with('[') { pkgs.push(Default::default()); } else if let (Some(p), Some((k, v))) = (pkgs.last_mut(), l.split_once('=')) { p.insert(k.trim().to_string(), v.trim().to_string()); } }
    let rel_of = |p: &str| p.trim_start_matches(".\\").replace('\\', "/");
    let mut n = 0;
    for p in &pkgs {
        let fx: HashSet<String> = p.get("EffectFiles").map(|v| v.split(',').map(|x| x.trim().to_ascii_lowercase()).collect()).unwrap_or_default();
        if !missing.iter().any(|m| fx.contains(m)) { continue; }
        let (Some(url), Some(ip)) = (p.get("DownloadUrl"), p.get("InstallPath")) else { continue };
        let tp = p.get("TextureInstallPath").cloned().unwrap_or_else(|| ".\\reshade-shaders\\Textures".into());
        let deny: HashSet<String> = p.get("DenyEffectFiles").map(|v| v.split(',').map(|x| x.trim().to_ascii_lowercase()).collect()).unwrap_or_default();
        let z = tmp.join(format!("pkg{n}.zip")); let d = tmp.join(format!("pkg{n}")); n += 1;
        let ok = (|| -> Result<(), String> {
            let r = agent(120).get(url).call().map_err(|e| e.to_string())?;
            let mut data = Vec::new(); r.into_reader().take(200 << 20).read_to_end(&mut data).map_err(|e| e.to_string())?;
            std::fs::write(&z, &data).map_err(|e| e.to_string())?; std::fs::create_dir_all(&d).map_err(|e| e.to_string())?; unzip(&z, &d)
        })();
        if let Err(e) = ok { log(format!("ReShade-Effektpaket {} nicht geladen: {e}", p.get("PackageName").cloned().unwrap_or_default())); continue; }
        let mut all = Vec::new(); walk(&d, &d, &mut all);
        for (rel, size) in all {
            let parts: Vec<&str> = rel.split('/').collect();
            let Some(i) = parts.iter().position(|x| x.eq_ignore_ascii_case("shaders") || x.eq_ignore_ascii_case("textures")) else { continue };
            let sub = parts[i + 1..].join("/"); if sub.is_empty() { continue; }
            let is_sh = parts[i].eq_ignore_ascii_case("shaders");
            if is_sh && deny.contains(&parts.last().unwrap_or(&"").to_ascii_lowercase()) { continue; }
            let to = format!("{}/{}", rel_of(if is_sh { ip } else { &tp }), sub);
            if !safe_rel(Path::new(&to)) { continue; }
            let dst = game.join(&to);
            if dst.exists() { continue; }
            if let Some(pd) = dst.parent() { let _ = std::fs::create_dir_all(pd); }
            if std::fs::copy(d.join(&rel), &dst).is_ok() { files.push(json!({"rel": to, "new": true, "size": size})); }
        }
        log(format!("ReShade-Effektpaket nachgeladen: {}", p.get("PackageName").cloned().unwrap_or_default()));
    }
    let still: Vec<String> = presets.iter().flat_map(|p| rs_missing(game, p)).collect();
    if still.is_empty() { String::new() } else { format!("Diese Effekte gibt es in keinem offiziellen Paket: {} – das Preset läuft ohne sie.", still.join(", ")) }
}
// welches Preset ReShade lädt: fehlt das eingestellte (pausiert/gelöscht), das neueste vorhandene nehmen – sonst den Standard
pub fn rs_sync(s: &Shared) {
    let st = status(s);
    let game = PathBuf::from(st["dir"].as_str().unwrap_or(""));
    let ini = game.join("ReShade.ini");
    let Ok(txt) = std::fs::read_to_string(&ini) else { return };
    const DEF: &str = ".\\ReShadePreset.ini";
    let cur = ini_get(&txt, "PresetPath").unwrap_or_else(|| DEF.to_string());
    let cur_ok = game.join(cur.trim_start_matches(".\\").replace('\\', "/")).is_file();
    let is_def = cur.eq_ignore_ascii_case(DEF);
    if cur_ok { return; }   // eingestelltes Preset ist da (auch ein eigenes ReShadePreset.ini) – nichts ändern
    let newest = std::fs::read_dir(game.join("reshade-presets")).into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".ini") && is_preset_file(&e.path()))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| format!(".\\reshade-presets\\{}", e.file_name().to_string_lossy()));
    let want = match newest { Some(p) => p, None if !is_def => DEF.to_string(), None => return };
    if want == cur { return; }
    let mut done = false;
    let mut out: Vec<String> = txt.lines().map(|l| if !done && l.trim_start().starts_with("PresetPath=") { done = true; format!("PresetPath={want}") } else { l.to_string() }).collect();
    if !done { if let Some(i) = out.iter().position(|l| l.trim() == "[GENERAL]") { out.insert(i + 1, format!("PresetPath={want}")); } else { out.insert(0, format!("[GENERAL]\r\nPresetPath={want}")); } }
    if std::fs::write(&ini, out.join("\r\n") + "\r\n").is_ok() { log(format!("ReShade-Preset: {want}")); }
}

// ReShade-Menütaste: „KeyOverlay=<Tastencode>,<Strg>,<Umschalt>,<Alt>“ im Abschnitt [INPUT] der ReShade.ini
pub fn rs_key(s: &Shared, set: Option<Value>) -> Value {
    let st = status(s);
    let ini = PathBuf::from(st["dir"].as_str().unwrap_or("")).join("ReShade.ini");
    let Ok(txt) = std::fs::read_to_string(&ini) else { return json!({"ok": false, "why": "ReShade ist nicht installiert"}) };
    if let Some(v) = set {
        let vk = v["vk"].as_u64().unwrap_or(0);
        if !(1..=254).contains(&vk) || matches!(vk, 16 | 17 | 18 | 91 | 92) { return json!({"ok": false, "why": "Diese Taste geht nicht"}); }
        let b = |k: &str| if v[k].as_bool().unwrap_or(false) { 1 } else { 0 };
        let line = format!("KeyOverlay={vk},{},{},{}", b("ctrl"), b("shift"), b("alt"));
        let mut lines: Vec<String> = txt.lines().map(String::from).collect();
        if let Some(l) = lines.iter_mut().find(|l| l.trim_start().starts_with("KeyOverlay=")) { *l = line; }
        else if let Some(i) = lines.iter().position(|l| l.trim() == "[INPUT]") { lines.insert(i + 1, line); }
        else { lines.push(String::new()); lines.push("[INPUT]".into()); lines.push(line); }
        if let Err(e) = std::fs::write(&ini, lines.join("\r\n") + "\r\n") { return json!({"ok": false, "why": busy_msg(&e.to_string())}); }
        log(format!("ReShade-Menütaste: {vk}"));
    }
    let txt = std::fs::read_to_string(&ini).unwrap_or_default();
    let p: Vec<u64> = ini_get(&txt, "KeyOverlay").unwrap_or_else(|| "36,0,0,0".into()).split(',').map(|x| x.trim().parse().unwrap_or(0)).collect();
    json!({"ok": true, "vk": p.first().copied().unwrap_or(36), "ctrl": p.get(1) == Some(&1), "shift": p.get(2) == Some(&1), "alt": p.get(3) == Some(&1)})
}
