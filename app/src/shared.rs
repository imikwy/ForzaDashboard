// Gemeinsame Karte: von Hand gezeichnete oder korrigierte Straßen, Orte und Radierungen – für die ganze Gruppe gleich.
// Jedes Objekt trägt einen Zeitstempel (ts) und seinen Autor (a). Kommen zwei Stände desselben Objekts zusammen,
// gewinnt der neuere (bei Gleichstand der größere Autor). So landen alle PCs beim selben Ergebnis, egal in welcher
// Reihenfolge Änderungen ankommen – auch wenn jemand zwischendurch offline war.
// Jeder PC legt seinen Gesamtstand beim Vermittler ab ("retained"): ein frisch gestarteter PC bekommt sofort die
// neueste Karte, auch wenn die anderen gerade nicht laufen. Änderungen gehen zusätzlich sofort als kleine Pakete raus.
// Objektarten (k): r = Straße, o = Ort, e = Radier-Kreis, x = ausgeblendete Straße (gelernt oder Forza-Karte),
// c = Einstellung für alle (z. B. "cfg:mapfit" = Ausrichtung der Forza-Karte: T, fp, q). "p" ist immer eine Punktliste.
use crate::net::{gunzip, gzip, publish};
use crate::state::Shared;
use crate::util::{log, now_ms, sha_hex};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

const FILE: &str = "karte-gemeinsam.json";
const CHUNK: usize = 120_000;
// Sicherungen im Datenordner: stündlich (die neuesten BACKUP_KEEP bleiben) und sofort vor jeder Massenlöschung
const BACKUP_DIR: &str = "Sicherungen";
const BACKUP_KEEP: usize = 72;
const MASS_DEL: usize = 20;

#[derive(Default)]
pub struct MapDoc {
    pub ents: BTreeMap<String, Value>,
    pub rev: u64,
    clock: u64,
    file_dirty: bool,
    snap_due: u64,
    snap_t: u64,
    snap_n: usize,
    rx: HashMap<String, SnapRx>,
    backup_t: u64,
    mass_del: usize,
}

#[derive(Default)]
struct SnapRx {
    h: String,
    n: usize,
    done: String,
    parts: HashMap<usize, Vec<u8>>,
}

fn ts_of(v: &Value) -> u64 {
    v.get("ts").and_then(Value::as_u64).unwrap_or(0)
}

fn author(v: &Value) -> &str {
    v.get("a").and_then(Value::as_str).unwrap_or("")
}

// gewinnt b gegen a?
fn newer(b: &Value, a: &Value) -> bool {
    (ts_of(b), author(b)) > (ts_of(a), author(a))
}

// nur sinnvolle, nicht zu große Objekte annehmen
fn valid(v: &Value) -> bool {
    let Some(o) = v.as_object() else { return false };
    let id_ok = o.get("id").and_then(Value::as_str).is_some_and(|i| !i.is_empty() && i.len() <= 80);
    let k_ok = matches!(o.get("k").and_then(Value::as_str), Some("r" | "o" | "e" | "x" | "c"));
    let p_ok = o.get("p").map_or(true, |p| p.is_null() || p.as_array().is_some_and(|a| a.len() <= 40000));
    id_ok && k_ok && p_ok && v.to_string().len() <= 400_000
}

impl MapDoc {
    pub fn load(s_root: &std::path::Path) -> MapDoc {
        let mut d = MapDoc::default();
        if let Ok(Value::Object(o)) = std::fs::read_to_string(s_root.join(FILE)).map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)) {
            for e in o.get("ents").and_then(Value::as_array).cloned().unwrap_or_default() {
                if let Some(id) = e.get("id").and_then(Value::as_str) {
                    d.clock = d.clock.max(ts_of(&e));
                    d.ents.insert(id.to_string(), e);
                }
            }
        }
        d.rev = 1;
        d
    }

    fn stamp(&mut self) -> u64 {
        self.clock = (self.clock + 1).max(now_ms());
        self.clock
    }

    // fremden Stand einmischen; liefert die tatsächlich geänderten Objekte
    fn merge(&mut self, ents: Vec<Value>) -> Vec<Value> {
        let mut changed = Vec::new();
        let mut deleted = 0;
        let dead = |v: &Value| v.get("d").and_then(Value::as_u64) == Some(1);
        for e in ents {
            if !valid(&e) || ts_of(&e) == 0 { continue; }
            let id = e["id"].as_str().unwrap_or_default().to_string();
            // Uhren der PCs gehen nie ganz gleich: eigene Uhr nachziehen, damit spätere eigene Änderungen gewinnen
            self.clock = self.clock.max(ts_of(&e));
            if self.ents.get(&id).is_some_and(|cur| !newer(&e, cur)) { continue; }
            if dead(&e) && self.ents.get(&id).is_some_and(|cur| !dead(cur)) { deleted += 1; }
            self.ents.insert(id, e.clone());
            changed.push(e);
        }
        if !changed.is_empty() { self.rev += 1; self.file_dirty = true; }
        // viel auf einmal gelöscht (z. B. Import einer alten Datei): save() sichert vorher den alten Stand
        if deleted >= MASS_DEL { self.mass_del += deleted; }
        changed
    }

    pub fn all(&self) -> Vec<Value> {
        self.ents.values().cloned().collect()
    }
}

fn emit(s: &Shared, rev: u64, ents: &[Value]) {
    s.emit(Some("map"), json!({"rev": rev, "ents": ents}).to_string());
}

fn grp(s: &Shared) -> String {
    s.mq_topic.lock().unwrap().clone()
}

fn sync_on(s: &Shared) -> bool {
    s.mq_ok() && s.cfg.lock().unwrap().fr_on
}

// Änderung von einem Dashboard dieses PCs: stempeln, übernehmen, an alle verteilen
pub fn local_put(s: &Arc<Shared>, ents: Vec<Value>) -> (u64, Vec<Value>) {
    let mut out = Vec::new();
    let rev = {
        let mut d = s.map.lock().unwrap();
        for e in ents {
            let Value::Object(mut o) = e else { continue };
            o.remove("ts");
            o.remove("a");
            let ts = d.stamp();
            o.insert("ts".into(), json!(ts));
            o.insert("a".into(), json!(s.sid));
            let v = Value::Object(o);
            if valid(&v) { out.push(v); }
        }
        let changed = d.merge(out.clone());
        if !changed.is_empty() { d.snap_due = now_ms() + 8000; }
        d.rev
    };
    if !out.is_empty() {
        emit(s, rev, &out);
        if sync_on(s) {
            publish(s, &format!("{}/map/d/{}", grp(s), s.sid), &gzip(json!({"ents": out}).to_string().as_bytes()), false);
        }
    }
    (rev, out)
}

// gemeinsame Karte aus einer Datei (karte-gemeinsam.json eines anderen PCs): Stempel bleiben, Neueres gewinnt
pub fn import(s: &Shared, ents: Vec<Value>) -> usize {
    let n = merge_remote(s, ents);
    if n > 0 { s.map.lock().unwrap().snap_due = now_ms() + 3000; }
    n
}

fn merge_remote(s: &Shared, ents: Vec<Value>) -> usize {
    let (rev, changed) = {
        let mut d = s.map.lock().unwrap();
        let changed = d.merge(ents);
        // unseren Gesamtstand beim Vermittler bald auffrischen, damit er das Neue auch enthält
        if !changed.is_empty() { let due = now_ms() + 10000; d.snap_due = if d.snap_due == 0 { due } else { d.snap_due.min(due) }; }
        (d.rev, changed)
    };
    if !changed.is_empty() {
        log(format!("Gemeinsame Karte: {} Änderung(en) empfangen", changed.len()));
        emit(s, rev, &changed);
    }
    changed.len()
}

fn parse_ents(payload: &[u8]) -> Vec<Value> {
    let Some(text) = gunzip(payload) else { return Vec::new() };
    serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("ents").and_then(Value::as_array).cloned()).unwrap_or_default()
}

// Nachrichten unter <gruppe>/map/...
pub fn on_mqtt(s: &Arc<Shared>, sub: &str, payload: &[u8]) {
    let mut it = sub.splitn(3, '/');
    let (kind, from, rest) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next());
    if from.is_empty() || from.len() > 20 { return; }
    match (kind, rest) {
        ("d", None) => { if from != s.sid && !payload.is_empty() { merge_remote(s, parse_ents(payload)); } }
        ("p", None) => {
            if from == s.sid || payload.is_empty() || !s.cfg.lock().unwrap().fr_on { return; }
            let Ok(Value::Object(mut o)) = serde_json::from_slice::<Value>(payload) else { return };
            let c = o.get("c").and_then(Value::as_str).unwrap_or("").chars().take(16).collect::<String>();
            o.insert("id".into(), json!(format!("{from}/{c}")));
            s.emit(Some("mp"), Value::Object(o).to_string());
        }
        ("s", None) => {
            // Gesamtstand-Kopf: {h, n}
            let Ok(m) = serde_json::from_slice::<Value>(payload) else { return };
            let (Some(h), Some(n)) = (m.get("h").and_then(Value::as_str), m.get("n").and_then(Value::as_u64)) else { return };
            if n == 0 || n > 200 { return; }
            {
                let mut d = s.map.lock().unwrap();
                let rx = d.rx.entry(from.to_string()).or_default();
                if rx.done == h { return; }
                if rx.h != h { rx.parts.retain(|i, _| (*i as u64) < n); }
                rx.h = h.to_string();
                rx.n = n as usize;
            }
            snap_try(s, from);
        }
        ("s", Some(i)) => {
            let Ok(i) = i.parse::<usize>() else { return };
            if i >= 200 || payload.is_empty() { return; }
            s.map.lock().unwrap().rx.entry(from.to_string()).or_default().parts.insert(i, payload.to_vec());
            snap_try(s, from);
        }
        _ => {}
    }
}

fn snap_try(s: &Shared, from: &str) {
    let blob = {
        let mut d = s.map.lock().unwrap();
        let Some(rx) = d.rx.get_mut(from) else { return };
        if rx.n == 0 || rx.h.is_empty() || rx.done == rx.h { return; }
        let mut blob = Vec::new();
        for i in 0..rx.n {
            let Some(p) = rx.parts.get(&i) else { return };
            blob.extend_from_slice(p);
        }
        if sha_hex(&blob)[..16] != rx.h { return; }
        rx.done = rx.h.clone();
        rx.parts.clear();
        blob
    };
    merge_remote(s, parse_ents(&blob));
}

// eigenen Gesamtstand beim Vermittler ablegen (in Teilen, falls groß)
fn snap_publish(s: &Shared) {
    if !sync_on(s) { return; }
    let (body, old_n) = {
        let mut d = s.map.lock().unwrap();
        d.snap_due = 0;
        d.snap_t = now_ms();
        (json!({"ents": d.all()}).to_string(), d.snap_n)
    };
    let gz = gzip(body.as_bytes());
    let h = sha_hex(&gz)[..16].to_string();
    let base = format!("{}/map/s/{}", grp(s), s.sid);
    let parts: Vec<&[u8]> = gz.chunks(CHUNK).collect();
    for (i, p) in parts.iter().enumerate() { publish(s, &format!("{base}/{i}"), p, true); }
    for i in parts.len()..old_n { publish(s, &format!("{base}/{i}"), &[], true); }
    publish(s, &base, json!({"h": h, "n": parts.len(), "ts": now_ms()}).to_string().as_bytes(), true);
    let mut d = s.map.lock().unwrap();
    d.snap_n = parts.len();
    // den eigenen Stand nicht noch einmal einmischen, wenn er zurückkommt
    d.rx.entry(s.sid.clone()).or_default().done = h;
}

// beim Beenden: Datei schreiben und einen noch ausstehenden Gesamtstand sofort ablegen
pub fn flush(s: &Shared) {
    save(s);
    if s.map.lock().unwrap().snap_due != 0 { snap_publish(s); }
}

pub fn on_connect(s: &Shared) {
    let mut d = s.map.lock().unwrap();
    d.snap_due = now_ms() + 6000;
    d.rx.clear();
}

fn save(s: &Shared) {
    let (body, mass, hourly) = {
        let mut d = s.map.lock().unwrap();
        if !d.file_dirty { return; }
        d.file_dirty = false;
        let now = now_ms();
        let hourly = now.saturating_sub(d.backup_t) >= 3_600_000;
        if hourly { d.backup_t = now; }
        (json!({"v": 1, "ents": d.all()}).to_string(), std::mem::take(&mut d.mass_del), hourly)
    };
    // die Datei auf der Platte ist noch der Stand vor dieser Änderung
    if mass > 0 {
        log(format!("Gemeinsame Karte: {mass} Objekte auf einmal gelöscht – alter Stand wird gesichert"));
        backup(s, &format!("vor-massenloeschung-{mass}"));
    }
    let (f, tmp) = (s.file(FILE), s.file(&format!("{FILE}.tmp")));
    if std::fs::write(&tmp, body).is_ok() { let _ = std::fs::rename(&tmp, &f); }
    if hourly { backup(s, "stuendlich"); }
}

// Kopie von karte-gemeinsam.json nach Sicherungen\karte-gemeinsam-JJJJ-MM-TT_hh-mm-ss-<art>.json
fn backup(s: &Shared, kind: &str) {
    let src = s.file(FILE);
    if !src.exists() { return; }
    let dir = s.file(BACKUP_DIR);
    let _ = std::fs::create_dir_all(&dir);
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let name = format!("karte-gemeinsam-{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-{kind}.json", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    if let Err(e) = std::fs::copy(&src, dir.join(&name)) { log(format!("Sicherung fehlgeschlagen: {e}")); return; }
    // alte stündliche Sicherungen aufräumen (die Namen sortieren sich nach Zeit); Notfall-Sicherungen bleiben
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let mut old: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("karte-gemeinsam-") && n.ends_with("-stuendlich.json")))
        .collect();
    old.sort();
    let extra = old.len().saturating_sub(BACKUP_KEEP);
    for p in &old[..extra] { let _ = std::fs::remove_file(p); }
}

pub fn spawn_timers(s: Arc<Shared>) {
    tauri::async_runtime::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(1));
        let mut n = 0u64;
        loop {
            t.tick().await;
            n += 1;
            let s2 = s.clone();
            let _ = tokio::task::spawn_blocking(move || {
                save(&s2);
                let now = now_ms();
                let (due, last) = { let d = s2.map.lock().unwrap(); (d.snap_due, d.snap_t) };
                // fällig, frühestens alle 5 s, und spätestens alle 15 min zur Sicherheit
                if (due != 0 && now >= due && now.saturating_sub(last) > 5000) || (n % 900 == 0) { snap_publish(&s2); }
            }).await;
        }
    });
}

// Anwesenheit beim Bearbeiten (Mauszeiger, ausgewählte Straße) an die Gruppe – nicht gespeichert
pub fn presence(s: &Shared, mut o: Map<String, Value>) {
    let name = s.cfg.lock().unwrap().fr_name.clone();
    o.insert("n".into(), json!(name));
    o.retain(|k, _| matches!(k.as_str(), "n" | "c" | "x" | "z" | "sel" | "tool" | "on"));
    let text = Value::Object(o.clone()).to_string();
    if text.len() > 2000 { return; }
    // auch an die anderen Dashboards dieses PCs (z. B. Tablet)
    let c = o.get("c").and_then(Value::as_str).unwrap_or("").to_string();
    o.insert("id".into(), json!(format!("{}/{c}", s.sid)));
    s.emit(Some("mp"), Value::Object(o).to_string());
    if sync_on(s) { publish(s, &format!("{}/map/p/{}", grp(s), s.sid), text.as_bytes(), false); }
}

pub fn snapshot(s: &Shared) -> Value {
    let d = s.map.lock().unwrap();
    json!({"rev": d.rev, "me": s.sid, "ents": d.all()})
}

pub fn ok_count(s: &Shared) -> usize {
    s.map.lock().unwrap().ents.values().filter(|e| e.get("d").and_then(Value::as_u64) != Some(1)).count()
}
