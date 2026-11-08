// Tuning-Community: Spieler zeigen ihre Autos (Beitrag mit Foto, Text, optional Tuning-Code), liken und kommentieren.
// - Jedes Dashboard legt seinen Stand als gespeicherte Nachrichten in einen gemeinsamen, verschlüsselten Kanal:
//     …/u/<Kürzel>          Übersicht: Beiträge (ohne Bild), eigene Likes, eigene Kommentare
//     …/u/<Kürzel>/p/<id>   das Foto eines Beitrags (WebP/JPEG, klein gerechnet, max. ~40 KB)
//   Wer den Kanal abonniert, bekommt vom Vermittler den letzten Stand aller Spieler – auch von denen, die offline sind.
// - Alles, was ankommt, wird streng geprüft und gekürzt. Eigene Beiträge stehen in tuning.json (+ Fotos in tuning-fotos\).
use crate::kanal::Kanal;
use crate::mqtt::Net;
use crate::state::Shared;
use crate::util::{log, now_ms, random_hex, read_json, sha_hex, take_chars, write_json};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

const BOERSE: &str = "FDTUNE001";
const MAX_POSTS: usize = 20;
const MAX_IMG: usize = 54_000;   // Kanal-Nachrichten bis 60 KB gehen in einem Stück (gespeichert) durch
const ARTEN: [&str; 8] = ["strasse", "stance", "drift", "drag", "rennen", "offroad", "rally", "touge"];

pub struct Tuning {
    net: Arc<Net>,
    me: String,
    file: PathBuf,
    fotos: PathBuf,
    mine: Mutex<Value>,
    alle: Mutex<HashMap<String, Value>>,
    bilder: Mutex<HashMap<String, String>>,
    k: Mutex<Option<Kanal>>,
    name: Mutex<String>,
}

static T: OnceLock<Arc<Tuning>> = OnceLock::new();
fn t() -> Option<&'static Arc<Tuning>> { T.get() }
fn clean_id(s: &str) -> String { s.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect() }
fn clean_code(c: &str) -> String { let d: String = c.chars().filter(|c| c.is_ascii_digit()).collect(); if d.len() == 9 { d } else { String::new() } }
fn ok_img(s: &str) -> bool {
    s.len() <= MAX_IMG && (s.starts_with("data:image/webp;base64,") || s.starts_with("data:image/jpeg;base64,"))
        && s.split_once(',').map(|(_, b)| b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')).unwrap_or(false)
}
// einen Beitrag prüfen und auf erlaubte Felder kürzen
fn clean(v: &Value) -> Option<Value> {
    let id = clean_id(v["id"].as_str().unwrap_or(""));
    let car = take_chars(v["car"].as_str().unwrap_or("").trim(), 70);
    let title = take_chars(v["title"].as_str().unwrap_or("").trim(), 60);
    if id.is_empty() || car.is_empty() { return None; }
    let art = v["art"].as_str().filter(|a| ARTEN.contains(a)).unwrap_or("strasse");
    let pi: String = v["pi"].as_str().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ').take(8).collect();
    let tags: Vec<Value> = v["tags"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| take_chars(x.trim(), 18)).filter(|x| !x.is_empty()).take(6).map(|x| json!(x)).collect()).unwrap_or_default();
    Some(json!({
        "id": id, "car": car, "ord": v["ord"].as_u64().filter(|o| *o < 100_000).unwrap_or(0), "title": title,
        "text": take_chars(v["text"].as_str().unwrap_or("").trim(), 800), "code": clean_code(v["code"].as_str().unwrap_or("")),
        "art": art, "pi": pi.trim(), "tags": tags, "t": v["t"].as_u64().unwrap_or(0), "ph": v["ph"].as_bool().unwrap_or(false),
    }))
}
fn clean_cm(v: &Value) -> Option<Value> {
    let to = v["to"].as_str().filter(|x| x.len() <= 40 && x.contains(':'))?;
    let x = take_chars(v["x"].as_str().unwrap_or("").trim(), 300);
    if x.is_empty() { return None; }
    Some(json!({"id": clean_id(v["id"].as_str().unwrap_or("")), "to": to, "x": x, "t": v["t"].as_u64().unwrap_or(0)}))
}

pub fn init(s: &Arc<Shared>) {
    let file = s.file("tuning.json");
    let fotos = s.file("tuning-fotos");
    let _ = std::fs::create_dir_all(&fotos);
    let mut mine = read_json(&file).filter(|v| v.is_object()).unwrap_or_else(|| json!({}));
    // ältere Fassung (nur Codes, ohne Fotos) nicht übernehmen – Beiträge brauchen jetzt ein Foto
    if mine.get("posts").is_none() { mine = json!({"posts": [], "likes": [], "cm": []}); }
    let tu = Arc::new(Tuning { net: s.net.clone(), me: sha_hex(format!("fd-tune:{}", s.sid).as_bytes())[..16].to_string(), file, fotos, mine: Mutex::new(mine), alle: Mutex::new(HashMap::new()), bilder: Mutex::new(HashMap::new()), k: Mutex::new(None), name: Mutex::new(s.name()) });
    if T.set(tu.clone()).is_err() { return; }
    let w = Arc::downgrade(&tu);
    let k = Kanal::new(&s.net, "tune", BOERSE, Arc::new(move |o: Value, sub: &str| {
        let Some(tu) = w.upgrade() else { return };
        let Some(rest) = sub.strip_prefix("u/") else { return };
        let (who, ph) = match rest.split_once("/p/") { Some((a, b)) => (a, Some(clean_id(b))), None => (rest, None) };
        if who.len() != 16 || !who.chars().all(|c| c.is_ascii_hexdigit()) { return; }
        if let Some(id) = ph {
            if o["y"] == "ph" { if let Some(img) = o["img"].as_str().filter(|i| ok_img(i)) { tu.bilder.lock().unwrap().insert(format!("{who}:{id}"), img.to_string()); } }
            return;
        }
        if o["y"] != "tunes" { return; }
        let posts: Vec<Value> = o["posts"].as_array().map(|a| a.iter().take(MAX_POSTS).filter_map(clean).collect()).unwrap_or_default();
        let likes: Vec<Value> = o["likes"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| x.len() <= 40).take(500).map(|x| json!(x)).collect()).unwrap_or_default();
        let cm: Vec<Value> = o["cm"].as_array().map(|a| a.iter().take(200).filter_map(clean_cm).collect()).unwrap_or_default();
        let n = take_chars(o["n"].as_str().unwrap_or("Fahrer").trim(), 24);
        tu.alle.lock().unwrap().insert(who.to_string(), json!({"n": n, "t": o["t"].as_u64().unwrap_or(0), "posts": posts, "likes": likes, "cm": cm}));
    }));
    *tu.k.lock().unwrap() = Some(k);
    // eigenen Stand ablegen: gleich, dann alle 20 Minuten (falls ein Vermittler neu gestartet ist)
    let s2 = s.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(8));
        loop {
            if let Some(tu) = t() { *tu.name.lock().unwrap() = s2.name(); tu.publish(true); }
            std::thread::sleep(std::time::Duration::from_secs(20 * 60));
        }
    });
}

impl Tuning {
    fn foto_file(&self, id: &str) -> PathBuf { self.fotos.join(format!("{}.txt", clean_id(id))) }
    fn publish(&self, fotos: bool) {
        let m = self.mine.lock().unwrap().clone();
        let posts = m["posts"].as_array().cloned().unwrap_or_default();
        let likes = m["likes"].as_array().cloned().unwrap_or_default();
        let cm = m["cm"].as_array().cloned().unwrap_or_default();
        if posts.is_empty() && likes.is_empty() && cm.is_empty() { return; }
        let n = self.name.lock().unwrap().clone();
        let msg = json!({"y": "tunes", "n": if n.is_empty() { "Fahrer".to_string() } else { n }, "t": now_ms(), "posts": posts, "likes": likes, "cm": cm});
        let k = self.k.lock().unwrap();
        let Some(k) = k.as_ref() else { return };
        k.send_to(&msg, &format!("u/{}", self.me), true);
        if fotos {
            for p in &posts {
                let id = p["id"].as_str().unwrap_or("");
                if let Ok(img) = std::fs::read_to_string(self.foto_file(id)) { if ok_img(&img) { k.send_to(&json!({"y": "ph", "id": id, "img": img}), &format!("u/{}/p/{id}", self.me), true); } }
            }
        }
    }
    fn save(&self) { write_json(&self.file, &self.mine.lock().unwrap()); }
}

// alle Beiträge (eigene zuerst bekannt) mit Likes und Kommentaren
pub fn list(s: &Shared) -> Value {
    let Some(tu) = t() else { return json!({"ok": false}) };
    *tu.name.lock().unwrap() = s.name();
    let mine = tu.mine.lock().unwrap().clone();
    let me_n = s.name();
    let mut alle = tu.alle.lock().unwrap().clone();
    alle.insert(tu.me.clone(), json!({"n": me_n, "posts": mine["posts"], "likes": mine["likes"], "cm": mine["cm"]}));
    let my_likes: Vec<String> = mine["likes"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut likes: HashMap<String, u64> = HashMap::new();
    let mut cms: HashMap<String, Vec<Value>> = HashMap::new();
    for (who, v) in &alle {
        let n = v["n"].as_str().unwrap_or("Fahrer");
        for l in v["likes"].as_array().into_iter().flatten() { if let Some(l) = l.as_str() { *likes.entry(l.to_string()).or_default() += 1; } }
        for c in v["cm"].as_array().into_iter().flatten() {
            let to = c["to"].as_str().unwrap_or("").to_string();
            cms.entry(to).or_default().push(json!({"by": n, "x": c["x"], "t": c["t"], "mine": *who == tu.me, "id": c["id"]}));
        }
    }
    let bilder = tu.bilder.lock().unwrap();
    let mut out = Vec::new();
    for (who, v) in &alle {
        let n = v["n"].as_str().unwrap_or("Fahrer");
        let own = *who == tu.me;
        for p in v["posts"].as_array().into_iter().flatten() {
            let id = p["id"].as_str().unwrap_or("");
            let key = format!("{who}:{id}");
            let has = if own { tu.foto_file(id).exists() } else { bilder.contains_key(&key) };
            let mut x = p.clone();
            x["key"] = json!(key); x["by"] = json!(n); x["mine"] = json!(own);
            x["img"] = if has { json!(format!("/tunes/photo?k={key}")) } else { Value::Null };
            x["likes"] = json!(likes.get(&key).copied().unwrap_or(0)); x["liked"] = json!(my_likes.contains(&key));
            let mut c = cms.remove(&key).unwrap_or_default();
            c.sort_by_key(|c| c["t"].as_u64().unwrap_or(0));
            x["cm"] = json!(c);
            out.push(x);
        }
    }
    json!({"ok": true, "net": tu.net.ok(), "posts": out, "players": alle.len()})
}

pub fn photo(key: &str) -> Option<String> {
    let tu = t()?;
    let (who, id) = key.split_once(':')?;
    if who == tu.me { return std::fs::read_to_string(tu.foto_file(id)).ok().filter(|i| ok_img(i)); }
    tu.bilder.lock().unwrap().get(key).cloned()
}

pub fn add(s: &Shared, v: &Value) -> Value {
    let Some(tu) = t() else { return json!({"ok": false, "why": "Nicht bereit"}) };
    let img = v["img"].as_str().unwrap_or("");
    if !ok_img(img) { return json!({"ok": false, "why": "Bitte ein Foto deines Autos hinzufügen"}); }
    let code_in = v["code"].as_str().unwrap_or("");
    if !code_in.trim().is_empty() && clean_code(code_in).is_empty() { return json!({"ok": false, "why": "Der Tuning-Code hat 9 Ziffern"}); }
    let mut v = v.clone();
    let id = random_hex(5);
    v["id"] = json!(id); v["t"] = json!(now_ms()); v["ph"] = json!(true);
    let Some(p) = clean(&v) else { return json!({"ok": false, "why": "Bitte das Auto angeben"}) };
    if p["title"].as_str().unwrap_or("").is_empty() { return json!({"ok": false, "why": "Bitte einen Titel angeben"}); }
    {
        let mut m = tu.mine.lock().unwrap();
        if !m["posts"].is_array() { m["posts"] = json!([]); }
        let a = m["posts"].as_array_mut().unwrap();
        if a.len() >= MAX_POSTS { return json!({"ok": false, "why": "Höchstens 20 eigene Beiträge – lösche erst einen alten"}); }
        a.insert(0, p);
    }
    if std::fs::write(tu.foto_file(&id), img).is_err() { return json!({"ok": false, "why": "Foto konnte nicht gespeichert werden"}); }
    *tu.name.lock().unwrap() = s.name();
    tu.save();
    if let Some(k) = tu.k.lock().unwrap().as_ref() { k.send_to(&json!({"y": "ph", "id": id, "img": img}), &format!("u/{}/p/{id}", tu.me), true); }
    tu.publish(false);
    log("Tuning-Beitrag geteilt");
    json!({"ok": true})
}

pub fn del(id: &str) -> Value {
    let Some(tu) = t() else { return json!({"ok": false}) };
    let id = clean_id(id);
    {
        let mut m = tu.mine.lock().unwrap();
        if let Some(a) = m["posts"].as_array_mut() { a.retain(|x| x["id"].as_str() != Some(id.as_str())); }
    }
    let _ = std::fs::remove_file(tu.foto_file(&id));
    // gespeichertes Foto beim Vermittler wegräumen (leere gespeicherte Nachricht)
    if let Some(k) = tu.k.lock().unwrap().as_ref() { tu.net.publish(&format!("{}/u/{}/p/{id}", k.topic(), tu.me), &[], true); }
    tu.save();
    tu.publish(false);
    json!({"ok": true})
}

pub fn like(key: &str, on: bool) -> Value {
    let Some(tu) = t() else { return json!({"ok": false}) };
    if key.len() > 40 || !key.contains(':') { return json!({"ok": false}); }
    {
        let mut m = tu.mine.lock().unwrap();
        if !m["likes"].is_array() { m["likes"] = json!([]); }
        let a = m["likes"].as_array_mut().unwrap();
        a.retain(|x| x.as_str() != Some(key));
        if on { a.push(json!(key)); if a.len() > 500 { a.remove(0); } }
    }
    tu.save();
    tu.publish(false);
    json!({"ok": true})
}

pub fn comment(key: &str, text: &str) -> Value {
    let Some(tu) = t() else { return json!({"ok": false}) };
    let Some(c) = clean_cm(&json!({"id": random_hex(4), "to": key, "x": text, "t": now_ms()})) else { return json!({"ok": false, "why": "Kommentar leer"}) };
    {
        let mut m = tu.mine.lock().unwrap();
        if !m["cm"].is_array() { m["cm"] = json!([]); }
        let a = m["cm"].as_array_mut().unwrap();
        a.push(c);
        while a.len() > 200 { a.remove(0); }
    }
    tu.save();
    tu.publish(false);
    json!({"ok": true})
}

pub fn uncomment(id: &str) -> Value {
    let Some(tu) = t() else { return json!({"ok": false}) };
    {
        let mut m = tu.mine.lock().unwrap();
        if let Some(a) = m["cm"].as_array_mut() { a.retain(|x| x["id"].as_str() != Some(id)); }
    }
    tu.save();
    tu.publish(false);
    json!({"ok": true})
}
