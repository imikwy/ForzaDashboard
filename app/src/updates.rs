// Updates über GitHub Releases – für die Karte und für das Programm, getrennt voneinander.
//   karte-v12          -> karte.manifest.json + karte.json.gz   (neue Kartenversion: wird sofort übernommen)
//   app-2026-10-29.1   -> app.manifest.json  + ForzaDashboard.exe (neues Programm: erst, wenn gerade nicht gefahren wird)
// Beides ist mit dem Herausgeber-Schlüssel signiert. Nur was die Prüfung besteht, wird benutzt.
//
// Automatisch zurück: Startet eine neue Programmversion nicht sauber (Absturz in den ersten 30 Sekunden), wird beim
// nächsten Start – oder sofort, wenn sie mit einem Fehler aussteigt – wieder die vorherige exe eingesetzt.
use crate::karte::{gunzip_bytes, verify_signed};
use crate::state::Shared;
use crate::util::{log, now_ms, read_json, sha_hex, ver_newer, write_json};
use crate::APP_VER;
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const EVERY: u64 = 20 * 60_000;
const EXE_NAME: &str = "ForzaDashboard.exe";

#[derive(Default)]
pub struct Upd {
    pub text: String,
    pub error: String,
    pub last: u64,
    pub etag: String,
    pub releases: Option<Value>,
    pub karte_neueste: u64,
    pub app_neueste: String,
    pub ready: Option<(String, PathBuf)>,
    pub busy: bool,
    pub installing: bool,
}

fn agent() -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).user_agent(&format!("ForzaDashboard/{APP_VER}"));
    if let Ok(tls) = native_tls::TlsConnector::new() { b = b.tls_connector(Arc::new(tls)); }
    b.build()
}

fn api() -> String {
    std::env::var("FD_UPDATE_API").unwrap_or_else(|_| "https://api.github.com".into())
}

pub fn repo(s: &Shared) -> String {
    let r = s.cfg.lock().unwrap().update_repo.clone();
    if r.is_empty() { crate::REPO.to_string() } else { r }
}

fn get(url: &str, max: u64) -> Result<Vec<u8>, String> {
    let r = agent().get(url).set("Accept", "application/octet-stream").call().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    r.into_reader().take(max + 1).read_to_end(&mut out).map_err(|e| e.to_string())?;
    if out.len() as u64 > max { return Err("Download zu groß".into()); }
    Ok(out)
}

fn releases(s: &Shared) -> Result<Value, String> {
    let url = format!("{}/repos/{}/releases?per_page=40", api(), repo(s));
    let etag = s.upd.lock().unwrap().etag.clone();
    let mut req = agent().get(&url).set("Accept", "application/vnd.github+json");
    if !etag.is_empty() { req = req.set("If-None-Match", &etag); }
    match req.call() {
        Ok(r) if r.status() == 304 => s.upd.lock().unwrap().releases.clone().ok_or_else(|| "keine Daten".into()),
        Ok(r) => {
            let tag = r.header("etag").unwrap_or("").to_string();
            let v: Value = serde_json::from_str(&r.into_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let mut u = s.upd.lock().unwrap();
            u.etag = tag;
            u.releases = Some(v.clone());
            Ok(v)
        }
        Err(ureq::Error::Status(304, _)) => s.upd.lock().unwrap().releases.clone().ok_or_else(|| "keine Daten".into()),
        Err(ureq::Error::Status(404, _)) => Err(format!("Repo {} nicht gefunden (oder privat)", repo(s))),
        Err(ureq::Error::Status(403, _)) => Err("GitHub-Limit erreicht – später erneut".into()),
        Err(e) => Err(format!("GitHub nicht erreichbar: {e}")),
    }
}

fn asset<'a>(rel: &'a Value, name: &str) -> Option<&'a str> {
    rel["assets"].as_array()?.iter().find(|a| a["name"] == name)?["browser_download_url"].as_str()
}

fn newest<'a>(list: &'a Value, prefix: &str, num: bool, skip: &[String]) -> Option<(String, &'a Value)> {
    let mut best: Option<(String, &Value)> = None;
    for rel in list.as_array()? {
        if rel["draft"] == true || rel["prerelease"] == true { continue; }
        let Some(v) = rel["tag_name"].as_str().and_then(|t| t.strip_prefix(prefix)) else { continue };
        if skip.iter().any(|x| x == v) { continue; }
        let better = match &best {
            None => true,
            Some((b, _)) => if num { v.parse::<u64>().unwrap_or(0) > b.parse::<u64>().unwrap_or(0) } else { ver_newer(v, b) },
        };
        if better && (!num || v.parse::<u64>().is_ok()) { best = Some((v.to_string(), rel)); }
    }
    best
}

fn install_map(s: &Arc<Shared>, v: u64, rel: &Value) -> Result<(), String> {
    let (Some(mu), Some(du)) = (asset(rel, "karte.manifest.json"), asset(rel, "karte.json.gz")) else { return Err(format!("Release karte-v{v} unvollständig")) };
    let man = String::from_utf8(get(mu, 1 << 20)?).map_err(|_| "Manifest unlesbar")?;
    verify_signed(&man).map_err(|e| format!("Karte v{v}: {e} – wird NICHT übernommen"))?;
    let buf = gunzip_bytes(&get(du, 32 << 20)?, 256 << 20).ok_or("Karte nicht entpackbar")?;
    if s.karten.install(&man, &buf, "GitHub").map_err(|e| format!("Karte v{v}: {e}"))? { crate::dienst::on_map_version(s); }
    Ok(())
}

// neueste gültige Kartenversion übernehmen; eine kaputte Version blockiert die vorherigen nicht
fn check_map(s: &Arc<Shared>, list: &Value) -> Result<(), String> {
    let mut rels: Vec<(u64, &Value)> = list.as_array().into_iter().flatten()
        .filter(|r| r["draft"] != true && r["prerelease"] != true)
        .filter_map(|r| Some((r["tag_name"].as_str()?.strip_prefix("karte-v")?.parse().ok()?, r)))
        .collect();
    rels.sort_by(|a, b| b.0.cmp(&a.0));
    let Some(newest) = rels.first().map(|r| r.0) else { return Ok(()) };
    s.upd.lock().unwrap().karte_neueste = newest;
    let mut err = None;
    for (v, rel) in rels {
        if v <= s.karten.active_version() { break; }
        if s.karten.has(v) { continue; }
        match install_map(s, v, rel) {
            Ok(()) => break,
            Err(e) => { log(format!("Update-Prüfung: {e}")); err.get_or_insert(e); }
        }
    }
    err.map_or(Ok(()), Err)
}

fn blocked() -> Vec<String> {
    let f = exe_path().with_file_name("update-gesperrt.json");
    read_json(&f).and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())).unwrap_or_default()
}

fn check_app(s: &Arc<Shared>, list: &Value) -> Result<(), String> {
    // eine Version, die hier schon einmal abgestürzt ist, wird übersprungen
    let Some((v, rel)) = newest(list, "app-", false, &blocked()) else { return Ok(()) };
    s.upd.lock().unwrap().app_neueste = v.clone();
    if !ver_newer(&v, APP_VER) || !s.cfg.lock().unwrap().updates { return Ok(()); }
    if s.upd.lock().unwrap().ready.as_ref().is_some_and(|r| r.0 == v) { return Ok(()); }
    let (Some(mu), Some(eu)) = (asset(rel, "app.manifest.json"), asset(rel, EXE_NAME)) else { return Err(format!("Release app-{v} unvollständig")) };
    let man = verify_signed(&String::from_utf8(get(mu, 1 << 20)?).map_err(|_| "Manifest unlesbar")?).map_err(|e| format!("Programm {v}: {e} – wird NICHT installiert"))?;
    if man["typ"] != "app" || man["version"] != v.as_str() { return Err(format!("Programm {v}: Manifest passt nicht")); }
    let exe = get(eu, 128 << 20)?;
    if man["sha256"].as_str() != Some(&sha_hex(&exe)) { return Err(format!("Programm {v}: Prüfsumme stimmt nicht")); }
    let f = exe_path().with_file_name(format!("{EXE_NAME}.neu"));
    std::fs::write(&f, &exe).map_err(|e| e.to_string())?;
    log(format!("Programm-Update {v} geladen und geprüft – wird installiert, sobald gerade nicht gefahren wird"));
    s.upd.lock().unwrap().ready = Some((v, f));
    Ok(())
}

pub fn check(s: &Arc<Shared>, force: bool) {
    {
        let mut u = s.upd.lock().unwrap();
        if u.busy || (!force && now_ms().saturating_sub(u.last) < EVERY - 5000) { return; }
        u.busy = true;
        u.last = now_ms();
    }
    // Karte und Programm unabhängig voneinander: ein Fehler beim einen hält das andere nicht auf
    // (Fehler bei Kartenversionen protokolliert check_map selbst)
    let res = match releases(s) {
        Ok(list) => {
            let m = check_map(s, &list);
            let a = check_app(s, &list);
            if let Err(e) = &a { log(format!("Update-Prüfung: {e}")); }
            a.and(m)
        }
        Err(e) => { log(format!("Update-Prüfung: {e}")); Err(e) }
    };
    let mut u = s.upd.lock().unwrap();
    u.busy = false;
    u.error = res.err().unwrap_or_default();
}

pub fn state(s: &Shared) -> Value {
    let u = s.upd.lock().unwrap();
    let kv = s.karten.active_version();
    let ready = u.ready.as_ref().map(|r| r.0.clone());
    let text = if let Some(r) = &ready { format!("Update {r} bereit – wird installiert, sobald du anhältst") }
        else if !u.error.is_empty() { u.error.clone() }
        else if u.karte_neueste > kv { format!("Karte v{} wird geladen …", u.karte_neueste) }
        else if u.last > 0 { "Aktuell".into() } else { "Noch nicht geprüft".into() };
    json!({"text": text, "error": u.error, "last": u.last, "repo": repo(s),
        "programm": {"lokal": APP_VER, "neueste": u.app_neueste, "ready": ready, "an": s.cfg.lock().unwrap().updates},
        "karte": {"lokal": kv, "neueste": u.karte_neueste}, "ready": ready})
}

pub fn spawn(s: Arc<Shared>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let mut n = 0u64;
        loop {
            let s2 = s.clone();
            let _ = tokio::task::spawn_blocking(move || check(&s2, n == 0)).await;
            n += 1;
            // fertig geladenes Programm-Update: erst installieren, wenn seit 60 s keine Daten vom Spiel kommen
            for _ in 0..(EVERY / 5000) {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let idle = now_ms().saturating_sub(s.last_packet.load(Ordering::Relaxed)) > 60_000;
                let ready = s.upd.lock().unwrap().ready.clone();
                if let (true, Some((v, f))) = (idle, ready) {
                    if s.upd.lock().unwrap().installing { continue; }
                    s.upd.lock().unwrap().installing = true;
                    install_and_restart(&s, &v, &f);
                }
                if s.upd.lock().unwrap().last == 0 { break; }   // "Jetzt prüfen" gedrückt
            }
        }
    });
}

pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(EXE_NAME))
}

fn marker() -> PathBuf {
    exe_path().with_file_name("update-pruefung.json")
}

// laufende exe umbenennen (das erlaubt Windows) und die neue an ihre Stelle legen
pub fn install_exe(neu: &std::path::Path) -> Result<(), String> {
    let exe = exe_path();
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or(EXE_NAME).to_string();
    let alt = exe.with_file_name(format!("{name}.alt"));
    let _ = std::fs::remove_file(&alt);
    std::fs::rename(&exe, &alt).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(neu, &exe) {
        let _ = std::fs::rename(&alt, &exe);
        return Err(e.to_string());
    }
    Ok(())
}

fn install_and_restart(s: &Arc<Shared>, ver: &str, f: &std::path::Path) {
    match install_exe(f) {
        Ok(()) => {
            write_json(&marker(), &json!({"neu": ver, "alt": APP_VER, "starts": 0, "t": now_ms()}));
            log(format!("Update {ver} installiert – startet neu"));
            s.emit(Some("upd"), json!({"ver": ver}).to_string());
            std::thread::sleep(Duration::from_millis(1500));
            if !crate::restart_self(s) {
                // neue exe startet nicht einmal -> sofort zurück
                let exe = exe_path();
                let alt = exe.with_file_name(format!("{}.alt", exe.file_name().and_then(|n| n.to_str()).unwrap_or(EXE_NAME)));
                let _ = std::fs::rename(&exe, exe.with_file_name(format!("{EXE_NAME}.kaputt")));
                let _ = std::fs::rename(&alt, &exe);
                let _ = std::fs::remove_file(marker());
                let mut b = blocked();
                b.push(ver.to_string());
                write_json(&exe.with_file_name("update-gesperrt.json"), &json!(b));
                log(format!("Update {ver} ließ sich nicht starten – diese Version läuft weiter"));
                let mut u = s.upd.lock().unwrap();
                u.installing = false;
                u.ready = None;
            }
        }
        Err(e) => {
            let mut u = s.upd.lock().unwrap();
            u.installing = false;
            u.ready = None;
            u.error = format!("Update fehlgeschlagen: {e}");
            log(&u.error);
        }
    }
}

// vorherige exe wieder einsetzen und starten; die kaputte Version wird nicht noch einmal installiert
pub fn rollback(why: &str) -> bool {
    let Some(m) = read_json(&marker()) else { return false };
    if m["neu"].as_str() != Some(APP_VER) { let _ = std::fs::remove_file(marker()); return false; }
    let exe = exe_path();
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or(EXE_NAME).to_string();
    let alt = exe.with_file_name(format!("{name}.alt"));
    if !alt.exists() { let _ = std::fs::remove_file(marker()); return false; }
    log(format!("Version {APP_VER} startet nicht sauber ({why}) – zurück zu {}", m["alt"].as_str().unwrap_or("?")));
    let mut b = blocked();
    b.push(APP_VER.to_string());
    write_json(&exe.with_file_name("update-gesperrt.json"), &json!(b));
    let kaputt = exe.with_file_name(format!("{name}.kaputt"));
    let _ = std::fs::remove_file(&kaputt);
    if std::fs::rename(&exe, &kaputt).is_err() || std::fs::rename(&alt, &exe).is_err() { return false; }
    let _ = std::fs::remove_file(marker());
    let _ = std::process::Command::new(&exe).env("FD_WAIT_PID", std::process::id().to_string()).env("FD_RESTARTED", "1").env("FD_WINDOW", "1").spawn();
    true
}

// beim Start: läuft gerade eine frisch installierte Version zum ersten Mal?
pub fn startup_probe() {
    let Some(mut m) = read_json(&marker()) else {
        let _ = std::fs::remove_file(exe_path().with_file_name(format!("{EXE_NAME}.alt")));
        return;
    };
    if m["neu"].as_str() != Some(APP_VER) { let _ = std::fs::remove_file(marker()); return; }
    let starts = m["starts"].as_u64().unwrap_or(0) + 1;
    // zweimal gestartet, ohne die 30 Sekunden zu schaffen -> sie stürzt ab
    if starts >= 3 && rollback("mehrfach abgestürzt") { std::process::exit(0); }
    m["starts"] = json!(starts);
    write_json(&marker(), &m);
    std::panic::set_hook(Box::new(|info| {
        log(format!("Absturz: {info}"));
        if rollback("Absturz") { std::process::exit(0); }
    }));
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        let _ = std::fs::remove_file(marker());
        log(format!("Version {APP_VER} läuft stabil"));
        let _ = std::panic::take_hook();
    });
}
