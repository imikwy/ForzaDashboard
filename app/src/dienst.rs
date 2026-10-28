// Verbindet Karte, Bearbeiten-Sitzungen und Lobbys mit dem Dashboard.
// Spielmodus: die veröffentlichte Kartenversion, nur lesen. Bearbeiten: nur in einer Sitzung (per Code).
use crate::karte::{check_ent, live, newer};
use crate::lobby::Lobbys;
use crate::sitzung::{Ctx, SEv, Sitzungen};
use crate::state::Shared;
use crate::util::{log, now_ms, sha_hex, take_chars, write_json};
use crate::{bruecke, veroeffentlichen};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

pub fn setup(s: &Arc<Shared>) {
    let w: Weak<Shared> = Arc::downgrade(s);
    let wn = w.clone();
    let ctx = Ctx {
        net: s.net.clone(),
        karten: s.karten.clone(),
        dev: s.sid.clone(),
        name: Arc::new(move || wn.upgrade().map(|s| s.name()).unwrap_or_default()),
        dir: s.file("Sitzungen"),
        emit: Arc::new(move |e: SEv| { if let Some(s) = w.upgrade() { on_session_event(&s, e); } }),
    };
    let _ = s.sitz.set(Sitzungen::new(ctx));
    let w = Arc::downgrade(s);
    let lob = Lobbys::new(&s.net, &s.sid, &s.root, Arc::new(move |d: &str, o: &Value| { if let Some(s) = w.upgrade() { flr_on_roads(&s, d, o); } }));
    let _ = s.lobbys.set(lob);
}

pub fn start(s: &Arc<Shared>) {
    s.net.start();
    s.lobbys().start();
    s.sitz().resume();
    bruecke::start(&s.net, &s.root);
    let s1 = s.clone();
    tauri::async_runtime::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_millis(500));
        let mut n = 0u64;
        loop {
            t.tick().await;
            n += 1;
            let p = *s1.my_pos.lock().unwrap();
            s1.lobbys().pub_pos(p, &s1.name(), s1.fr_on());
            friends_tick(&s1);
            if n % 30 == 3 {
                let s2 = s1.clone();
                let _ = tokio::task::spawn_blocking(move || pub_roads(&s2, false)).await;
            }
        }
    });
}

pub fn pub_roads(s: &Shared, force: bool) {
    let Some(learned) = crate::util::read_json(&s.file("gelernte-strassen.json")) else { return };
    s.lobbys().pub_roads(&learned, s.lver.load(Ordering::Relaxed), &s.name(), s.fr_on(), force);
}

fn friends_tick(s: &Shared) {
    let on = s.fr_on();
    let list = if on { s.lobbys().friend_list() } else { vec![] };
    let lr = s.flr.lock().unwrap().list.len();
    let upd = crate::updates::state(s)["text"].clone();
    s.emit(Some("fr"), json!({"on": on, "ok": s.net_ok(), "name": s.name(), "group": s.lobbys().names(), "lobbys": s.lobbys().list(),
        "list": list, "lr": lr, "upd": upd}).to_string());
}

// ---------------- Ereignisse an die Dashboards ----------------
fn on_session_event(s: &Shared, e: SEv) {
    match e {
        SEv::Change(ents) => { if !ents.is_empty() { s.emit(Some("map"), json!({"ents": ents, "auth": 1}).to_string()); } }
        SEv::Reload => s.emit(Some("mapreload"), json!({"mode": mode(s)}).to_string()),
        SEv::Status => s.emit(Some("sess"), sess_info(s).to_string()),
        SEv::Note(t) => s.emit(Some("note"), json!({"t": t}).to_string()),
        SEv::Presence(o) => {
            let c = take_chars(o["c"].as_str().unwrap_or(""), 20);
            s.emit(Some("mp"), json!({"id": format!("{}/{c}", o["d"].as_str().unwrap_or("")), "n": take_chars(o["n"].as_str().unwrap_or("Freund"), 24),
                "on": o["on"], "tool": o["tool"], "sel": o["sel"], "x": o["x"], "z": o["z"]}).to_string());
        }
    }
}

// neue Kartenversion ist aktiv (von GitHub oder aus einer Sitzung)
pub fn on_map_version(s: &Shared) {
    s.emit(Some("kv"), s.karten.info().to_string());
    match s.sitz().cur() {
        None => s.emit(Some("mapreload"), json!({"mode": mode(s)}).to_string()),
        Some(c) => s.emit(Some("note"), json!({"t": format!("Neue Kartenversion {} geladen – deine Sitzung arbeitet weiter auf Version {}", s.karten.active_version(), c.base())}).to_string()),
    }
}

pub fn capability(s: &Shared) -> Value {
    let mut c = s.pub_cap.lock().unwrap();
    if now_ms().saturating_sub(c.0) > 60_000 { *c = (now_ms(), veroeffentlichen::capability(&s.repo_root())); }
    c.1.clone()
}

pub fn sess_info(s: &Shared) -> Value {
    json!({"aktiv": s.sitz().cur().map(|c| c.status()), "gespeichert": s.sitz().saved(), "karte": s.karten.info(), "herausgeber": capability(s)})
}

pub fn mode(s: &Shared) -> &'static str {
    match s.sitz().cur() { Some(c) if c.ready() => "sitzung", Some(_) => "sitzung-wartet", None => "spiel" }
}

// ---------------- Karte für das Dashboard ----------------
pub fn map_ents(s: &Shared) -> Vec<Value> {
    let mut base: Vec<Value> = match s.sitz().cur() {
        Some(c) if c.ready() => c.view(),
        _ => s.karten.active().map(|a| a.ents.to_vec()).unwrap_or_default(),
    };
    let lokal = s.lokal.lock().unwrap();
    if lokal.is_empty() { return base; }
    let mut idx: std::collections::HashMap<String, usize> = base.iter().enumerate().map(|(i, e)| (e["id"].as_str().unwrap_or("").to_string(), i)).collect();
    for (id, e) in lokal.iter() {
        match idx.get(id) {
            Some(i) if id.starts_with("cfg:car:") || !live(&base[*i]) => base[*i] = e.clone(),
            Some(_) => {}
            None => { idx.insert(id.clone(), base.len()); base.push(e.clone()); }
        }
    }
    base
}

pub fn map_get(s: &Shared) -> Value {
    let v = s.karten.active_version();
    json!({"ents": map_ents(s), "rev": v, "me": s.sid, "mode": mode(s), "version": v})
}

fn lokal_save(s: &Shared) {
    let ents: Vec<Value> = s.lokal.lock().unwrap().values().cloned().collect();
    write_json(&s.file("karte-lokal.json"), &json!({"ents": ents}));
}

fn release_has(s: &Shared, id: &str) -> bool {
    s.karten.active().is_some_and(|a| a.get(id).is_some_and(live))
}

pub fn map_post(s: &Shared, b: &Value) -> Value {
    let Some(list) = b["ents"].as_array() else { return json!({"ok": false}) };
    let now = now_ms();
    let mut out = Vec::new();
    let mut rest = Vec::new();
    let stamp = |e: &Value, ts: u64| { let mut x = e.clone(); x["ts"] = json!(ts); x["a"] = json!(s.sid); x };
    for e in list {
        let Some(id) = e["id"].as_str() else { continue };
        if id.starts_with("cfg:car:") {
            let x = stamp(e, now);
            s.lokal.lock().unwrap().insert(id.to_string(), x.clone());
            out.push(x);
        } else { rest.push(e.clone()); }
    }
    if !out.is_empty() { lokal_save(s); }
    if !rest.is_empty() {
        let only_fit = rest.iter().all(|e| e["id"] == "cfg:mapfit");
        match s.sitz().cur() {
            Some(c) if c.ready() => {
                let sub = c.submit(&rest, b["grp"].as_str().map(String::from));
                for e in &sub.ents { out.push(stamp(e, sub.t0)); }
            }
            // eigene Ausrichtung nur, solange die Karte keine mitbringt
            _ if only_fit && !release_has(s, "cfg:mapfit") => {
                for e in &rest { let x = stamp(e, now); s.lokal.lock().unwrap().insert("cfg:mapfit".into(), x.clone()); out.push(x); }
                lokal_save(s);
            }
            _ if only_fit => return json!({"ok": true, "ents": []}),   // Ausrichtung kommt aus der Karte
            _ => {
                let cur = map_ents(s);
                let back: Vec<Value> = rest.iter().map(|e| cur.iter().find(|x| x["id"] == e["id"]).cloned().unwrap_or_else(|| json!({"id": e["id"], "k": e["k"], "d": 1, "ts": 0, "a": ""}))).collect();
                return json!({"ok": false, "needSession": true, "ents": back});
            }
        }
    }
    json!({"ok": true, "ents": out})
}

// Datei aufs Dashboard gezogen (z. B. alte karte-gemeinsam.json): nur Neueres übernehmen, nur in einer Sitzung
pub fn map_import(s: &Shared, b: &Value) -> Value {
    let Some(list) = b["ents"].as_array() else { return json!({"ok": false, "n": 0}) };
    let Some(c) = s.sitz().cur().filter(|c| c.ready()) else { return json!({"ok": false, "needSession": true, "n": 0}) };
    let cur: std::collections::HashMap<String, Value> = c.view().into_iter().map(|e| (e["id"].as_str().unwrap_or("").to_string(), e)).collect();
    let add: Vec<Value> = list.iter().filter(|e| {
        let id = e["id"].as_str().unwrap_or("");
        !id.is_empty() && check_ent(e).is_none() && !id.starts_with("cfg:car:") && cur.get(id).is_none_or(|x| newer(e, x))
    }).cloned().collect();
    if !add.is_empty() { c.submit(&add, None); }
    json!({"ok": true, "n": add.len()})
}

pub fn presence(s: &Shared, o: &Value) {
    let msg = json!({"y": "pr", "c": take_chars(o["c"].as_str().unwrap_or(""), 20), "n": s.name(), "on": if o["on"].as_bool().unwrap_or(false) || o["on"].as_u64() == Some(1) { 1 } else { 0 },
        "tool": o["tool"], "sel": o["sel"], "x": o["x"], "z": o["z"]});
    if let Some(c) = s.sitz().cur() { c.send(msg.clone()); }
    // auch an die anderen Fenster dieses PCs (z. B. Tablet)
    s.emit(Some("mp"), json!({"id": format!("{}/{}", s.sid, msg["c"].as_str().unwrap_or("")), "n": msg["n"], "on": msg["on"], "tool": msg["tool"], "sel": msg["sel"], "x": msg["x"], "z": msg["z"]}).to_string());
}

// ---------------- gelernte Straßen der Freunde ----------------
fn flr_save(s: &Shared) {
    let text = Value::Object(s.flr.lock().unwrap().list.clone()).to_string();
    if let Err(e) = crate::util::write_atomic(&s.file("freunde-strassen.json"), text.as_bytes()) { log(format!("freunde-strassen.json: {e}")); }
}

fn flr_on_roads(s: &Shared, id: &str, o: &Value) {
    let ts = o["ts"].as_f64().unwrap_or(0.0);
    {
        let mut flr = s.flr.lock().unwrap();
        if flr.list.get(id).and_then(|p| p["ts"].as_f64()).is_some_and(|prev| prev >= ts) { return; }
        let take = |k: &str, n: usize| o[k].as_array().map(|a| a.iter().take(n).cloned().collect::<Vec<_>>()).unwrap_or_default();
        flr.list.insert(id.to_string(), json!({"n": take_chars(o["n"].as_str().unwrap_or("Freund"), 24), "ts": ts, "roads": take("roads", 5000), "cands": take("cands", 8000)}));
        flr.v += 1;
    }
    flr_save(s);
}

pub fn flr_import(s: &Shared, name: &str, roads: Vec<Value>, cands: Vec<Value>) {
    let n = take_chars(name, 24);
    let id = format!("import-{}", &sha_hex(n.to_lowercase().as_bytes())[..8]);
    {
        let mut flr = s.flr.lock().unwrap();
        flr.list.insert(id, json!({"n": n, "ts": now_ms(), "roads": roads.into_iter().take(5000).collect::<Vec<_>>(), "cands": cands.into_iter().take(8000).collect::<Vec<_>>()}));
        flr.v += 1;
    }
    flr_save(s);
    log(format!("Gelernte Straßen von {n} importiert"));
}
