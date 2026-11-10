// Spieler-Profile (freiwillig): eindeutiger Nutzername, Profilbild, Tags, Level/EP und Garage.
// - Jeder PC hat einen eigenen Schlüssel (profil-schluessel.txt im Datenordner, verlässt nie den PC).
//   Profile werden damit signiert – niemand kann ein fremdes Profil verändern oder sich als jemand anderes ausgeben.
// - Alles liegt als gespeicherte Nachricht im verschlüsselten Gemeinschaftskanal:
//     …/u/<id>      das Profil (Name, Tags, Level, EP, Garage, aktuelles Auto)
//     …/u/<id>/av   das Profilbild (klein, max. ~24 KB)
//   <id> = die ersten 16 Zeichen des Hashes vom öffentlichen Schlüssel.
// - Ein Nutzername ist vergeben, sobald ihn ein anderes Profil trägt (wer zuerst kommt, behält ihn).
// - EP sammelt das Dashboard beim Fahren: 1 EP je km, saubere Rennen, Events, neue Autos.
use crate::kanal::Kanal;
use crate::mqtt::Net;
use crate::state::Shared;
use crate::util::{log, now_ms, read_json, sha_hex, write_json};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey, Signature};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

const KANAL: &str = "FDPROF001";
const MAX_AV: usize = 26_000;
// eigener Hintergrund/GIF der Profilkarte ab Level 100, die JDM-Banner ab Level 25
const MAX_BG: usize = 48_000;
pub const BG_LEVEL: u64 = 100;
pub const JDM_LEVEL: u64 = 25;
// vorgefertigte Tags – dazu darf jeder eigene schreiben (bis 15 Zeichen, höchstens 3 Tags)
pub const TAGS: [&str; 5] = ["Cruiser", "Drift", "Touge", "Car-Meets", "Nachtfahrer"];
const BANNER: [&str; 8] = ["glas", "streifen", "verlauf", "carbon", "neon", "aurora", "bild", "link"];
// GIF-Banner per Link: nur von Seiten, die das Einbetten erlauben (Giphy, Tenor, Imgur) und nur Bilddateien
fn clean_url(s: &str) -> String {
    let s = s.trim();
    if s.len() > 300 || !s.starts_with("https://") || s.chars().any(|c| c.is_whitespace() || "\"'<>()\\".contains(c)) { return String::new(); }
    let rest = &s[8..];
    let host = rest.split('/').next().unwrap_or("").to_lowercase();
    let ok_host = host == "i.giphy.com" || host == "media.giphy.com" || (host.starts_with("media") && host.ends_with(".giphy.com"))
        || host == "media.tenor.com" || host == "c.tenor.com" || host == "i.imgur.com";
    let path = rest.split('?').next().unwrap_or("").to_lowercase();
    if ok_host && [".gif", ".webp", ".png", ".jpg", ".jpeg"].iter().any(|e| path.ends_with(e)) { s.to_string() } else { String::new() }
}
// Vorauswahl-Banner „jdm1“ … „jdm40“ (die Links stehen im Dashboard)
fn preset_ok(b: &str) -> bool { b.strip_prefix("jdm").and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=40).contains(&n)) }

pub struct Profil {
    key: SigningKey,
    pub id: String,
    file: PathBuf,
    av_file: PathBuf,
    me: Mutex<Value>,
    alle: Mutex<HashMap<String, Value>>,
    bilder: Mutex<HashMap<String, String>>,
    hgr: Mutex<HashMap<String, String>>,
    bg_file: PathBuf,
    k: Mutex<Option<Kanal>>,
    last_pub: Mutex<u64>,
    #[allow(dead_code)]
    net: Arc<Net>,
}
static P: OnceLock<Arc<Profil>> = OnceLock::new();
fn p() -> Option<&'static Arc<Profil>> { P.get() }

// Level: von Level n auf n+1 braucht es 100 + 25·(n−1) EP (Level 25 ≈ 50 Std. allein mit Skill-Ketten)
pub fn level_of(ep: u64) -> (u64, u64, u64) {
    let (mut lv, mut base) = (1u64, 0u64);
    loop { let need = 100 + 25 * (lv - 1); if ep < base + need { return (lv, ep - base, need); } base += need; lv += 1; if lv > 999 { return (lv, 0, 1); } }
}
fn clean_user(u: &str) -> String { u.trim().to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.').take(20).collect() }
fn ok_av(s: &str) -> bool {
    s.len() <= MAX_AV && (s.starts_with("data:image/webp;base64,") || s.starts_with("data:image/jpeg;base64,"))
        && s.split_once(',').map(|(_, b)| b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')).unwrap_or(false)
}
fn ok_bg(s: &str) -> bool {
    s.len() <= MAX_BG && (s.starts_with("data:image/webp;base64,") || s.starts_with("data:image/jpeg;base64,"))
        && s.split_once(',').map(|(_, b)| b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')).unwrap_or(false)
}
// freier Text: keine Steuerzeichen, Leerraum zusammengefasst, höchstens n Zeichen
fn clean_txt(s: &str, n: usize) -> String {
    let t: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(n).collect()
}
fn clean_tags(v: &Value) -> Vec<Value> {
    let mut out: Vec<String> = Vec::new();
    for x in v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>()).unwrap_or_default() {
        let t = clean_txt(x, 15);
        if !t.is_empty() && !out.iter().any(|o| o.to_lowercase() == t.to_lowercase()) { out.push(t); }
        if out.len() >= 3 { break; }
    }
    out.into_iter().map(|t| json!(t)).collect()
}
// verknüpfte Konten: nur Text (Name/Link), damit sich Spieler finden – nichts wird abgefragt
fn clean_links(v: &Value) -> Value {
    let mut o = serde_json::Map::new();
    for (k, n) in [("steam", 80), ("xbox", 24), ("dc", 40)] {
        let t = clean_txt(v[k].as_str().unwrap_or(""), n);
        if !t.is_empty() { o.insert(k.into(), json!(t)); }
    }
    Value::Object(o)
}
fn key_ok(s: &str) -> bool { !s.is_empty() && s.len() <= 16 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') }
// Aussehen der Profilkarte: Banner, Farbe, Titel (ein Erfolg), bis zu 3 ausgestellte Erfolge
fn clean_style(v: &Value, lv: u64) -> Value {
    let url = clean_url(v["url"].as_str().unwrap_or(""));
    let bn = v["bn"].as_str().filter(|b| (BANNER.contains(b) || (preset_ok(b) && lv >= JDM_LEVEL)) && ((*b != "bild" && *b != "link") || lv >= BG_LEVEL) && (*b != "link" || !url.is_empty())).unwrap_or("glas");
    let col = v["col"].as_str().filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit())).unwrap_or("#0a84ff");
    let ti = v["ti"].as_str().filter(|t| key_ok(t)).unwrap_or("");
    let sh: Vec<Value> = v["sh"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| key_ok(x)).take(3).map(|x| json!(x)).collect()).unwrap_or_default();
    json!({"bn": bn, "col": col.to_lowercase(), "ti": ti, "sh": sh, "url": if bn == "link" { url } else { String::new() }})
}
fn clean_ach(v: &Value) -> Vec<Value> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| key_ok(x)).take(80).map(|x| json!(x)).collect()).unwrap_or_default()
}
// zuletzt dabei: Events, Kolonnen, eigene Beiträge
fn clean_act(v: &Value) -> Vec<Value> {
    v.as_array().map(|a| a.iter().filter_map(|x| {
        let k = x["k"].as_str().filter(|k| ["ev", "kol", "post"].contains(k))?;
        let n = clean_txt(x["n"].as_str().unwrap_or(""), 48);
        if n.is_empty() { return None; }
        Some(json!({"k": k, "n": n, "t": x["t"].as_u64().unwrap_or(0)}))
    }).take(12).collect()).unwrap_or_default()
}
fn clean_gar(v: &Value) -> Vec<Value> {
    v.as_array().map(|a| a.iter().take(400).filter_map(|g| Some(json!([g[0].as_u64()? % 100_000, (g[1].as_f64()?.max(0.0) * 10.0).round() / 10.0]))).collect()).unwrap_or_default()
}
fn id_of(pk: &[u8]) -> String { sha_hex(pk)[..16].to_string() }
fn verify(pk_hex: &str, msg: &str, sig_hex: &str) -> bool {
    let (Ok(pk), Ok(sig)) = (hex::decode(pk_hex), hex::decode(sig_hex)) else { return false };
    let (Ok(pk), Ok(sig)) = (<[u8; 32]>::try_from(pk.as_slice()), <[u8; 64]>::try_from(sig.as_slice())) else { return false };
    let Ok(vk) = VerifyingKey::from_bytes(&pk) else { return false };
    vk.verify(msg.as_bytes(), &Signature::from_bytes(&sig)).is_ok()
}

pub fn init(s: &Arc<Shared>) {
    let kf = s.file("profil-schluessel.txt");
    let seed: [u8; 32] = std::fs::read_to_string(&kf).ok().and_then(|h| hex::decode(h.trim()).ok()).and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok()).unwrap_or_else(|| {
        let mut b = [0u8; 32]; let _ = getrandom::fill(&mut b); let _ = std::fs::write(&kf, hex::encode(b)); b
    });
    let key = SigningKey::from_bytes(&seed);
    let id = id_of(key.verifying_key().as_bytes());
    let file = s.file("profil.json");
    let me = read_json(&file).filter(|v| v.is_object()).unwrap_or_else(|| json!({}));
    let pr = Arc::new(Profil { key, id, file, av_file: s.file("profil-bild.txt"), bg_file: s.file("profil-hintergrund.txt"), me: Mutex::new(me), alle: Mutex::new(HashMap::new()), bilder: Mutex::new(HashMap::new()), hgr: Mutex::new(HashMap::new()), k: Mutex::new(None), last_pub: Mutex::new(0), net: s.net.clone() });
    if P.set(pr.clone()).is_err() { return; }
    let w = Arc::downgrade(&pr);
    let k = Kanal::new(&s.net, "prof", KANAL, Arc::new(move |o: Value, sub: &str| {
        let Some(pr) = w.upgrade() else { return };
        let Some(rest) = sub.strip_prefix("u/") else { return };
        let (who, av, bg) = match rest.split_once('/') { Some((a, "av")) => (a, true, false), Some((a, "bg")) => (a, false, true), Some(_) => return, None => (rest, false, false) };
        if who.len() != 16 || !who.chars().all(|c| c.is_ascii_hexdigit()) || who == pr.id { return; }
        let pk = o["pk"].as_str().unwrap_or("");
        if hex::decode(pk).map(|b| id_of(&b) != who).unwrap_or(true) { return; }
        if bg {
            let img = o["img"].as_str().unwrap_or("");
            if o["y"] == "bg" && ok_bg(img) && verify(pk, img, o["sig"].as_str().unwrap_or("")) { pr.hgr.lock().unwrap().insert(who.to_string(), img.to_string()); }
            return;
        }
        if av {
            let img = o["img"].as_str().unwrap_or("");
            if o["y"] == "av" && ok_av(img) && verify(pk, img, o["sig"].as_str().unwrap_or("")) { pr.bilder.lock().unwrap().insert(who.to_string(), img.to_string()); }
            return;
        }
        if o["y"] != "prof" { return; }
        let mut body = o.clone(); let sig = body["sig"].as_str().unwrap_or("").to_string();
        if let Some(m) = body.as_object_mut() { m.remove("sig"); }
        if !verify(pk, &body.to_string(), &sig) { return; }
        let u = clean_user(o["u"].as_str().unwrap_or(""));
        if u.len() < 3 { return; }
        let garage: Vec<Value> = o["garage"].as_array().map(|a| a.iter().take(3).filter_map(|g| Some(json!([g[0].as_u64()? % 100_000, (g[1].as_f64()?.max(0.0) * 10.0).round() / 10.0]))).collect()).unwrap_or_default();
        let ep = o["ep"].as_u64().unwrap_or(0).min(10_000_000);
        let lv = level_of(ep).0;
        pr.alle.lock().unwrap().insert(who.to_string(), json!({"id": who, "u": u, "tags": clean_tags(&o["tags"]), "ep": ep, "lv": lv,
            "links": clean_links(&o["links"]), "st": clean_style(&o["st"], lv), "ach": clean_ach(&o["ach"]), "act": clean_act(&o["act"]), "gar": clean_gar(&o["gar"]), "bg": o["bg"].as_bool().unwrap_or(false) && lv >= BG_LEVEL,
            "km": o["km"].as_f64().unwrap_or(0.0).max(0.0).round(), "cars": o["cars"].as_u64().unwrap_or(0).min(5000), "ev": o["ev"].as_u64().unwrap_or(0).min(5000),
            "garage": garage, "car": o["car"].as_u64().unwrap_or(0) % 100_000, "since": o["since"].as_u64().unwrap_or(0), "t": o["t"].as_u64().unwrap_or(0), "av": o["av"].as_bool().unwrap_or(false)}));
    }));
    *pr.k.lock().unwrap() = Some(k);
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(10));
        loop { if let Some(pr) = p() { pr.publish(true); } std::thread::sleep(std::time::Duration::from_secs(20 * 60)); }
    });
}

impl Profil {
    fn has(&self) -> bool { self.me.lock().unwrap()["u"].as_str().map(|u| u.len() >= 3).unwrap_or(false) }
    fn save(&self) { write_json(&self.file, &self.me.lock().unwrap()); }
    fn record(&self) -> Value {
        let m = self.me.lock().unwrap().clone();
        let mut g: Vec<(u64, f64)> = m["garage"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_f64()?))).collect()).unwrap_or_default();
        g.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        json!({"y": "prof", "pk": hex::encode(self.key.verifying_key().as_bytes()), "u": m["u"], "tags": m["tags"], "ep": m["ep"].as_u64().unwrap_or(0),
            "km": (m["km"].as_f64().unwrap_or(0.0) * 10.0).round() / 10.0, "cars": g.len(), "ev": m["ev"].as_u64().unwrap_or(0),
            "garage": g.iter().take(3).map(|(o, k)| json!([o, (k * 10.0).round() / 10.0])).collect::<Vec<_>>(), "car": m["car"].as_u64().unwrap_or(0),
            "gar": g.iter().take(400).map(|(o, k)| json!([o, (k * 10.0).round() / 10.0])).collect::<Vec<_>>(),
            "links": clean_links(&m["links"]), "st": clean_style(&m["st"], level_of(m["ep"].as_u64().unwrap_or(0)).0), "ach": clean_ach(&m["ach"]), "act": clean_act(&m["act"]),
            "bg": self.bg_file.exists() && level_of(m["ep"].as_u64().unwrap_or(0)).0 >= BG_LEVEL,
            "since": m["since"].as_u64().unwrap_or(0), "t": now_ms(), "av": self.av_file.exists()})
    }
    fn publish(&self, with_av: bool) {
        if !self.has() { return; }
        *self.last_pub.lock().unwrap() = now_ms();
        let mut r = self.record();
        let sig = hex::encode(self.key.sign(r.to_string().as_bytes()).to_bytes());
        r["sig"] = json!(sig);
        let k = self.k.lock().unwrap(); let Some(k) = k.as_ref() else { return };
        k.send_to(&r, &format!("u/{}", self.id), true);
        if with_av { if let Ok(img) = std::fs::read_to_string(&self.av_file) { if ok_av(&img) {
            let sig = hex::encode(self.key.sign(img.as_bytes()).to_bytes());
            k.send_to(&json!({"y": "av", "pk": hex::encode(self.key.verifying_key().as_bytes()), "img": img, "sig": sig}), &format!("u/{}/av", self.id), true);
        } } }
        if with_av { if let Ok(img) = std::fs::read_to_string(&self.bg_file) { if ok_bg(&img) {
            let sig = hex::encode(self.key.sign(img.as_bytes()).to_bytes());
            k.send_to(&json!({"y": "bg", "pk": hex::encode(self.key.verifying_key().as_bytes()), "img": img, "sig": sig}), &format!("u/{}/bg", self.id), true);
        } } }
    }
    // nicht bei jedem Kilometer neu senden – höchstens alle 2 Minuten (Level/Auto sind trotzdem bald aktuell)
    fn publish_soon(&self) { if now_ms().saturating_sub(*self.last_pub.lock().unwrap()) > 120_000 { self.publish(false); } }
}

fn view(v: &Value, own: bool) -> Value {
    let ep = v["ep"].as_u64().unwrap_or(0);
    let (lv, cur, need) = level_of(ep);
    let mut x = v.clone();
    x["lv"] = json!(lv); x["cur"] = json!(cur); x["need"] = json!(need); x["own"] = json!(own);
    if !x["av"].is_boolean() { x["av"] = json!(false); }
    x
}

// eigenes Profil (auch ohne Namen: dann has=false, EP zählen trotzdem schon)
pub fn me() -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    let m = pr.me.lock().unwrap().clone();
    let mut g: Vec<(u64, f64)> = m["garage"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_f64()?))).collect()).unwrap_or_default();
    g.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut v = view(&json!({"id": pr.id, "u": m["u"], "tags": m["tags"], "ep": m["ep"].as_u64().unwrap_or(0), "km": m["km"].as_f64().unwrap_or(0.0).round(),
        "cars": g.len(), "ev": m["ev"].as_u64().unwrap_or(0), "garage": g.iter().take(6).map(|(o, k)| json!([o, (k * 10.0).round() / 10.0])).collect::<Vec<_>>(),
        "car": m["car"], "since": m["since"], "av": pr.av_file.exists()}), true);
    let lv = v["lv"].as_u64().unwrap_or(1);
    v["ok"] = json!(true); v["has"] = json!(pr.has()); v["today"] = m["log"][today()].clone(); v["alltags"] = json!(TAGS);
    v["links"] = clean_links(&m["links"]); v["st"] = clean_style(&m["st"], lv); v["ach"] = json!(clean_ach(&m["ach"])); v["act"] = json!(clean_act(&m["act"]));
    v["gar"] = json!(g.iter().map(|(o, k)| json!([o, (k * 10.0).round() / 10.0])).collect::<Vec<_>>()); v["tot"] = m["tot"].clone();
    v["bg"] = json!(pr.bg_file.exists() && lv >= BG_LEVEL); v["bgLevel"] = json!(BG_LEVEL); v["jdmLevel"] = json!(JDM_LEVEL);
    v
}
pub fn get(id: &str) -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    if id == pr.id { return me(); }
    match pr.alle.lock().unwrap().get(id) { Some(v) => { let mut x = view(v, false); x["ok"] = json!(true); x } None => json!({"ok": false}) }
}
pub fn all() -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    let mut list: Vec<Value> = pr.alle.lock().unwrap().values().map(|v| json!({"id": v["id"], "u": v["u"], "lv": v["lv"], "av": v["av"], "tags": v["tags"], "car": v["car"]})).collect();
    if pr.has() { let m = me(); list.push(json!({"id": m["id"], "u": m["u"], "lv": m["lv"], "av": m["av"], "tags": m["tags"], "car": m["car"], "own": true})); }
    json!({"ok": true, "me": pr.id, "list": list})
}
// Kennung des Profil-Schlüssels (auch ohne Namen) und Signieren/Prüfen für andere Module (Admin, Vorschläge)
pub fn key_id() -> Option<String> { p().map(|pr| pr.id.clone()) }
pub fn sign(msg: &str) -> Option<(String, String)> { let pr = p()?; Some((hex::encode(pr.key.verifying_key().as_bytes()), hex::encode(pr.key.sign(msg.as_bytes()).to_bytes()))) }
pub fn check_sig(pk: &str, msg: &str, sig: &str) -> bool { verify(pk, msg, sig) }
pub fn pk_id(pk: &str) -> Option<String> { hex::decode(pk).ok().filter(|b| b.len() == 32).map(|b| id_of(&b)) }
pub fn my_id() -> Option<String> { let pr = p()?; if pr.has() { Some(pr.id.clone()) } else { None } }
pub fn my_name() -> Option<String> { let pr = p()?; if pr.has() { pr.me.lock().unwrap()["u"].as_str().map(String::from) } else { None } }
pub fn background(id: &str) -> Option<String> {
    let pr = p()?;
    if id == pr.id { return std::fs::read_to_string(&pr.bg_file).ok().filter(|i| ok_bg(i)); }
    pr.hgr.lock().unwrap().get(id).cloned()
}
// eigener Hintergrund für die Profilkarte – erst ab Level 50
pub fn set_background(img: &str) -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    if img.is_empty() {
        let _ = std::fs::remove_file(&pr.bg_file);
        if let Some(k) = pr.k.lock().unwrap().as_ref() { pr.net.publish(&format!("{}/u/{}/bg", k.topic(), pr.id), &[], true); }
        pr.publish(false); return json!({"ok": true});
    }
    let lv = level_of(pr.me.lock().unwrap()["ep"].as_u64().unwrap_or(0)).0;
    if lv < BG_LEVEL { return json!({"ok": false, "why": format!("Eigene Hintergründe gibt es ab Level {BG_LEVEL}")}); }
    if !ok_bg(img) { return json!({"ok": false, "why": "Bild zu groß oder falsches Format"}); }
    if std::fs::write(&pr.bg_file, img).is_err() { return json!({"ok": false, "why": "Bild konnte nicht gespeichert werden"}); }
    { let mut m = pr.me.lock().unwrap(); m["st"]["bn"] = json!("bild"); }
    pr.save(); pr.publish(true);
    json!({"ok": true})
}
// Aussehen, Konten, Erfolge und Tags ändern (ohne Namensprüfung)
pub fn set_extra(v: &Value) -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    {
        let mut m = pr.me.lock().unwrap();
        let lv = level_of(m["ep"].as_u64().unwrap_or(0)).0;
        if v.get("st").is_some() { m["st"] = clean_style(&v["st"], lv); }
        if v.get("links").is_some() { m["links"] = clean_links(&v["links"]); }
        if v.get("ach").is_some() { m["ach"] = json!(clean_ach(&v["ach"])); }
        if v.get("tags").is_some() { m["tags"] = json!(clean_tags(&v["tags"])); }
    }
    pr.save();
    if pr.has() { pr.publish(false); }
    json!({"ok": true})
}
// „Zuletzt dabei“ ergänzen (Event, Kolonne, eigener Beitrag); gleiche Einträge rücken nach vorn
pub fn activity(k: &str, n: &str) {
    let Some(pr) = p() else { return };
    let n = clean_txt(n, 48);
    if n.is_empty() { return; }
    {
        let mut m = pr.me.lock().unwrap();
        if !m["act"].is_array() { m["act"] = json!([]); }
        let a = m["act"].as_array_mut().unwrap();
        a.retain(|x| !(x["k"] == k && x["n"] == n.as_str()));
        a.insert(0, json!({"k": k, "n": n, "t": now_ms()}));
        a.truncate(12);
    }
    pr.save();
    if pr.has() { pr.publish_soon(); }
}
pub fn avatar(id: &str) -> Option<String> {
    let pr = p()?;
    if id == pr.id { return std::fs::read_to_string(&pr.av_file).ok().filter(|i| ok_av(i)); }
    pr.bilder.lock().unwrap().get(id).cloned()
}

// Name und Tags setzen (Name: 3–20 Zeichen a–z, 0–9, _ und .; muss frei sein)
pub fn set(v: &Value) -> Value {
    let Some(pr) = p() else { return json!({"ok": false, "why": "Nicht bereit"}) };
    let raw = v["u"].as_str().unwrap_or("");
    let u = clean_user(raw);
    if u.len() < 3 { return json!({"ok": false, "why": "Der Name braucht mindestens 3 Zeichen (a–z, 0–9, _ oder .)"}); }
    if u != raw.trim().to_lowercase() { return json!({"ok": false, "why": "Erlaubt sind nur a–z, 0–9, _ und ."}); }
    if pr.alle.lock().unwrap().values().any(|x| x["u"].as_str() == Some(u.as_str())) { return json!({"ok": false, "why": "Dieser Name ist schon vergeben", "taken": true}); }
    {
        let mut m = pr.me.lock().unwrap();
        if m["u"].as_str() != Some(u.as_str()) && m["since"].as_u64().unwrap_or(0) == 0 { m["since"] = json!(now_ms()); }
        m["u"] = json!(u);
        if v.get("tags").is_some() { m["tags"] = json!(clean_tags(&v["tags"])); }
        if !m["tags"].is_array() { m["tags"] = json!([]); }
    }
    pr.save(); pr.publish(true);
    log(format!("Profil gespeichert: {u}"));
    json!({"ok": true})
}
pub fn check(u: &str) -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    let c = clean_user(u);
    let taken = pr.alle.lock().unwrap().values().any(|x| x["u"].as_str() == Some(c.as_str()));
    json!({"ok": true, "u": c, "valid": c.len() >= 3 && c == u.trim().to_lowercase(), "free": !taken})
}
pub fn set_avatar(img: &str) -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    if img.is_empty() { let _ = std::fs::remove_file(&pr.av_file); pr.publish(false); return json!({"ok": true}); }
    if !ok_av(img) { return json!({"ok": false, "why": "Bild zu groß oder falsches Format"}); }
    if std::fs::write(&pr.av_file, img).is_err() { return json!({"ok": false, "why": "Bild konnte nicht gespeichert werden"}); }
    pr.publish(true);
    json!({"ok": true})
}
// Profil löschen: Name frei, Bild weg; EP und Garage bleiben auf diesem PC (für ein späteres neues Profil)
pub fn remove() -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    if let Some(k) = pr.k.lock().unwrap().as_ref() { pr.net.publish(&format!("{}/u/{}", k.topic(), pr.id), &[], true); pr.net.publish(&format!("{}/u/{}/av", k.topic(), pr.id), &[], true); }
    let _ = std::fs::remove_file(&pr.av_file);
    { let mut m = pr.me.lock().unwrap(); m["u"] = json!(""); m["since"] = json!(0); }
    pr.save();
    json!({"ok": true})
}

fn today() -> String {
    // Tag nach Ortszeit reicht nicht ohne Zeitzonen-Bibliothek – UTC-Tag ist für „heute“ gut genug
    let d = now_ms() / 86_400_000;
    let (y, m, dd) = civil(d as i64);
    format!("{y:04}-{m:02}-{dd:02}")
}
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468; let era = z.div_euclid(146_097); let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// Fahrdaten vom Dashboard: {"km":1,"car":1234} (Statistik/Garage), {"ep":4,"why":"clean","bonus":2} (EP, Kolonnen-Anteil), {"race":1}, {"event":"<key>"}
pub fn drive(v: &Value) -> Value {
    let Some(pr) = p() else { return json!({"ok": false}) };
    let mut gained = 0u64; let mut why = Vec::new();
    {
        let mut m = pr.me.lock().unwrap();
        let day = today();
        if !m["log"].is_object() { m["log"] = json!({}); }
        // nur die letzten 14 Tage behalten
        if let Some(l) = m["log"].as_object_mut() { let mut keys: Vec<String> = l.keys().cloned().collect(); keys.sort(); while keys.len() > 14 { l.remove(&keys.remove(0)); } }
        // Tageslog (14 Tage) und Gesamtsummen (für Erfolge)
        let add = |m: &mut Value, k: &str, n: u64| { let e = &mut m["log"][day.as_str()][k]; *e = json!(e.as_u64().unwrap_or(0) + n); if n > 0 { let t = &mut m["tot"][k]; *t = json!(t.as_u64().unwrap_or(0) + 1); } };
        let car = v["car"].as_u64().filter(|c| *c > 0 && *c < 100_000);
        if let Some(c) = car { m["car"] = json!(c); }
        let km = v["km"].as_f64().unwrap_or(0.0);
        if km > 0.0 && km <= 5.0 {
            m["km"] = json!(m["km"].as_f64().unwrap_or(0.0) + km);
            if let Some(c) = car {
                if !m["garage"].is_object() { m["garage"] = json!({}); }
                let key = c.to_string(); let before = m["garage"][key.as_str()].as_f64().unwrap_or(0.0);
                m["garage"][key.as_str()] = json!(before + km);
                if before < 1.0 && before + km >= 1.0 { gained += 25; add(&mut m, "car", 25); why.push("Neues Auto"); }   // neues Auto gefahren
            }
        }
        // EP fürs saubere Fahren rechnet das Dashboard (ohne Crash, sauber abgebogen, Kolonne ×2/×5) – hier nur verbuchen
        if let Some(n) = v["ep"].as_u64().filter(|n| *n > 0 && *n <= 2000) {
            let k = match v["why"].as_str() { Some("turn") => "turn", Some("convoy") => "convoy", Some("km") => "km", _ => "clean" };
            gained += n; add(&mut m, k, n);
            if let Some(c) = v["cnt"].as_u64().filter(|c| *c > 1 && *c <= 500) { let t = &mut m["tot"][k]; *t = json!(t.as_u64().unwrap_or(0) + c - 1); }
            if let Some(extra) = v["bonus"].as_u64().filter(|b| *b > 0 && *b < n) { add(&mut m, "convoy", extra); add(&mut m, k, 0); let e = &mut m["log"][day.as_str()][k]; *e = json!(e.as_u64().unwrap_or(0).saturating_sub(extra)); }
        }
        if v["race"].as_u64() == Some(1) { gained += 30; add(&mut m, "race", 30); why.push("Sauberes Rennen"); }
        if let Some(ev) = v["event"].as_str().filter(|e| e.len() <= 40) {
            if !m["evdone"].is_array() { m["evdone"] = json!([]); }
            let done = m["evdone"].as_array_mut().unwrap();
            if !done.iter().any(|x| x.as_str() == Some(ev)) {
                done.push(json!(ev)); if done.len() > 200 { done.remove(0); } gained += 100; add(&mut m, "event", 100); m["ev"] = json!(m["ev"].as_u64().unwrap_or(0) + 1); why.push("Event mitgefahren");
                let n = clean_txt(v["evn"].as_str().unwrap_or(ev), 48);
                if !m["act"].is_array() { m["act"] = json!([]); }
                let a = m["act"].as_array_mut().unwrap(); a.insert(0, json!({"k": "ev", "n": n, "t": now_ms()})); a.truncate(12);
            }
        }
        let before = m["ep"].as_u64().unwrap_or(0);
        m["ep"] = json!(before + gained);
        let up = level_of(before + gained).0 > level_of(before).0;
        drop(m);
        pr.save();
        if up || !why.is_empty() { pr.publish(false); } else { pr.publish_soon(); }
        let ep = before + gained; let (lv, cur, need) = level_of(ep);
        return json!({"ok": true, "gained": gained, "why": why, "ep": ep, "lv": lv, "cur": cur, "need": need, "up": up});
    }
}
