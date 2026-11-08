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
    let backup = root(s).join("backup").join(mod_id.to_string());
    let mut files = Vec::new();
    copy_tree(&base, &base, &game, &backup, &mut files)?;
    let mut c = cfg(s);
    let detail = get(s, &format!("/games/{GAME}/mods/{mod_id}.json")).unwrap_or(json!({}));
    let entry = json!({"mod": mod_id, "file": file_id, "name": detail["name"], "version": detail["version"], "pic": detail["picture_url"], "time": now_ms(), "files": files});
    if !c["installed"].is_array() { c["installed"] = json!([]); }
    c["installed"].as_array_mut().unwrap().push(entry);
    save(s, &c);
    let _ = std::fs::remove_dir_all(&stage);
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
        out.push(json!({"key": key, "state": "an", "dash": true, "mod": id, "name": m["name"], "pic": m["pic"], "version": m["version"], "files": files, "size": size, "time": m["time"]}));
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
pub fn toggle(s: &Shared, key: &str, on: bool) -> Value {
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
    let entry = json!({"key": key, "dash": g["dash"], "mod": g["mod"], "name": g["name"], "pic": g["pic"], "version": g["version"], "files": moved, "size": size, "time": now_ms()});
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
pub fn remove(s: &Shared, key: &str) -> Value {
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
