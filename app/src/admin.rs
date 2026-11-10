// Admin und Straßen-Vorschläge
// - Admins sind feste Profile (Profil-Kennung = Hash des Profil-Schlüssels, der den PC nie verlässt). Nur ein Admin kann
//   die offizielle Karte bearbeiten (Bearbeiten-Sitzung starten/beitreten), veröffentlichen und Vorschläge entscheiden.
// - Zusätzlich schützt ein Admin-Passwort (PBKDF2-gesichert, nur auf diesem PC) die Rechte: erst nach der Anmeldung ist
//   das freigeschaltet – bis zum Abmelden, Neustart oder 4 Stunden ohne Aktion. Fünf falsche Versuche = 60 s Sperre.
// - Spieler mit Profil schicken Vorschläge (gezeichnete Änderungen + Beschreibung) in einen verschlüsselten Kanal:
//     …/v/<profil-id>/<vid>   der Vorschlag (vom Spieler signiert; gelöscht = signierter Grabstein)
//     …/e/<profil-id>/<vid>   die Entscheidung des Admins (mit dem Admin-Schlüssel signiert): angenommen / abgelehnt
//   Übernommen wird ein Vorschlag nie automatisch – der Admin schaut ihn an und übernimmt ihn in seine Sitzung.
use crate::kanal::Kanal;
use crate::karte::{canon_ent, check_ent, strip};
use crate::mqtt::Net;
use crate::state::Shared;
use crate::util::{log, now_ms, random_hex, read_json, take_chars, write_json};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

const ADMINS: [&str; 1] = ["42d1a19dd220567e"];   // matze
const KANAL: &str = "FDVORS001";
const ITER: u32 = 150_000;
const IDLE: u64 = 4 * 3_600_000;
const MAX_ENTS: usize = 300;
const MAX_ENTS_JSON: usize = 50_000;   // passt mit Text in eine gespeicherte Kanal-Nachricht (60 KB)
const MAX_OWN: usize = 15;

struct Adm {
    pw_file: PathBuf,
    own_file: PathBuf,
    dec_file: PathBuf,
    until: Mutex<u64>,
    fails: Mutex<(u32, u64)>,
    own: Mutex<Vec<Value>>,
    alle: Mutex<HashMap<String, Value>>,
    ent: Mutex<HashMap<String, Value>>,
    k: Mutex<Option<Kanal>>,
    #[allow(dead_code)]
    net: Arc<Net>,
}
static A: OnceLock<Arc<Adm>> = OnceLock::new();
fn a() -> Option<&'static Arc<Adm>> { A.get() }

fn clean_id(s: &str) -> String { s.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect() }
fn hex16(s: &str) -> bool { s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()) }
fn fin(x: &Value) -> Value { x.as_f64().filter(|f| f.is_finite() && f.abs() < 1e6).map(|f| json!((f * 10.0).round() / 10.0)).unwrap_or(Value::Null) }

// ---------------- Admin + Passwort ----------------
pub fn is_admin() -> bool { crate::profil::key_id().is_some_and(|id| ADMINS.contains(&id.as_str())) }
pub fn is_admin_id(id: &str) -> bool { ADMINS.contains(&id) }

// freigeschaltet? (jede Admin-Aktion verlängert die Anmeldung)
pub fn unlocked() -> bool {
    let Some(a) = a() else { return false };
    if !is_admin() { return false; }
    let mut u = a.until.lock().unwrap();
    let now = now_ms();
    if *u > now { *u = now + IDLE; true } else { false }
}

fn pbkdf2(pw: &[u8], salt: &[u8], iter: u32) -> [u8; 32] {
    let mac0 = <Hmac<Sha256> as Mac>::new_from_slice(pw).expect("hmac");
    let mut m = mac0.clone();
    m.update(salt);
    m.update(&1u32.to_be_bytes());
    let mut u: [u8; 32] = m.finalize().into_bytes().into();
    let mut out = u;
    for _ in 1..iter {
        let mut m = mac0.clone();
        m.update(&u);
        u = m.finalize().into_bytes().into();
        for i in 0..32 { out[i] ^= u[i]; }
    }
    out
}
fn has_pw(a: &Adm) -> bool { read_json(&a.pw_file).is_some_and(|v| v["hash"].is_string()) }
fn pw_ok(a: &Adm, pw: &str) -> bool {
    let Some(v) = read_json(&a.pw_file) else { return false };
    let (Ok(salt), Ok(want)) = (hex::decode(v["salt"].as_str().unwrap_or("")), hex::decode(v["hash"].as_str().unwrap_or(""))) else { return false };
    let iter = v["iter"].as_u64().unwrap_or(ITER as u64).clamp(1000, 5_000_000) as u32;
    let got = pbkdf2(pw.as_bytes(), &salt, iter);
    want.len() == 32 && got.iter().zip(want.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn status() -> Value {
    let Some(a) = a() else { return json!({"ok": false}) };
    let open = if unlocked() { *a.until.lock().unwrap() } else { 0 };
    json!({"ok": true, "admin": is_admin(), "pw": has_pw(a), "unlocked": open > 0, "until": open, "me": crate::profil::key_id()})
}

pub fn login(pw: &str) -> Value {
    let Some(a) = a() else { return json!({"ok": false}) };
    if !is_admin() { return json!({"ok": false, "why": "Dieses Profil ist kein Admin"}); }
    if !has_pw(a) { return json!({"ok": false, "why": "Noch kein Admin-Passwort festgelegt", "needPw": true}); }
    let now = now_ms();
    {
        let f = a.fails.lock().unwrap();
        if f.0 >= 5 && now < f.1 { return json!({"ok": false, "why": format!("Zu viele Versuche – in {} s erneut versuchen", (f.1 - now) / 1000 + 1)}); }
    }
    if !pw_ok(a, pw) {
        let mut f = a.fails.lock().unwrap();
        if f.0 >= 5 { f.0 = 0; }
        f.0 += 1;
        if f.0 >= 5 { f.1 = now + 60_000; }
        log("Admin-Anmeldung: falsches Passwort");
        return json!({"ok": false, "why": if f.0 >= 5 { "Falsches Passwort – 60 s gesperrt".to_string() } else { format!("Falsches Passwort ({} von 5 Versuchen)", f.0) }});
    }
    *a.fails.lock().unwrap() = (0, 0);
    *a.until.lock().unwrap() = now + IDLE;
    log("Admin angemeldet");
    status()
}

pub fn logout() -> Value {
    if let Some(a) = a() { *a.until.lock().unwrap() = 0; }
    status()
}

// Passwort festlegen (erstes Mal) oder ändern (angemeldet + altes Passwort)
pub fn set_pw(new: &str, old: &str) -> Value {
    let Some(a) = a() else { return json!({"ok": false}) };
    if !is_admin() { return json!({"ok": false, "why": "Dieses Profil ist kein Admin"}); }
    if has_pw(a) && !(unlocked() && pw_ok(a, old)) { return json!({"ok": false, "why": "Das aktuelle Passwort stimmt nicht"}); }
    if new.chars().count() < 8 { return json!({"ok": false, "why": "Das Passwort braucht mindestens 8 Zeichen"}); }
    if new.chars().count() > 128 { return json!({"ok": false, "why": "Das Passwort ist zu lang"}); }
    let mut salt = [0u8; 16];
    let _ = getrandom::fill(&mut salt);
    let hash = pbkdf2(new.as_bytes(), &salt, ITER);
    write_json(&a.pw_file, &json!({"hinweis": "Admin-Passwort (nur der Hash, nicht weitergeben)", "salt": hex::encode(salt), "hash": hex::encode(hash), "iter": ITER}));
    *a.until.lock().unwrap() = now_ms() + IDLE;
    log("Admin-Passwort gespeichert");
    status()
}

// ---------------- Vorschläge ----------------
fn clean_ents(v: &Value) -> Result<Vec<Value>, String> {
    let list = v.as_array().ok_or("Keine Änderungen")?;
    if list.is_empty() { return Err("Der Vorschlag enthält noch keine Änderung – zeichne zuerst etwas auf der Karte".into()); }
    if list.len() > MAX_ENTS { return Err(format!("Zu viele Änderungen (max. {MAX_ENTS}) – bitte in mehrere Vorschläge aufteilen")); }
    let mut out = Vec::new();
    for e in list {
        let id = e["id"].as_str().unwrap_or("");
        if id.starts_with("cfg:") { continue; }
        if let Some(err) = check_ent(e) { return Err(format!("Ungültige Änderung: {err}")); }
        out.push(canon_ent(&strip(e)));
    }
    if out.is_empty() { return Err("Der Vorschlag enthält keine Straßen- oder Ortsänderung".into()); }
    if serde_json::to_string(&out).unwrap_or_default().len() > MAX_ENTS_JSON { return Err("Der Vorschlag ist zu groß – bitte in mehrere aufteilen".into()); }
    Ok(out)
}
fn vors_view(v: &Value, with_ents: bool, dec: Option<&Value>, me: &str) -> Value {
    let mut x = json!({"key": format!("{}/{}", v["pid"].as_str().unwrap_or(""), v["id"].as_str().unwrap_or("")), "pid": v["pid"], "vid": v["id"], "u": v["n"],
        "title": v["title"], "text": v["text"], "t": v["t"], "base": v["base"], "x": v["x"], "z": v["z"], "n": v["ents"].as_array().map(|e| e.len()).unwrap_or(0),
        "st": "offen", "mine": v["pid"].as_str() == Some(me)});
    if let Some(d) = dec { x["st"] = d["st"].clone(); x["why"] = d["why"].clone(); x["dt"] = d["t"].clone(); }
    if with_ents { x["ents"] = v["ents"].clone(); }
    x
}

pub fn init(s: &Arc<Shared>) {
    let ad = Arc::new(Adm { pw_file: s.file("admin-passwort.json"), own_file: s.file("vorschlaege.json"), dec_file: s.file("vorschlag-entscheidungen.json"),
        until: Mutex::new(0), fails: Mutex::new((0, 0)), own: Mutex::new(Vec::new()), alle: Mutex::new(HashMap::new()), ent: Mutex::new(HashMap::new()),
        k: Mutex::new(None), net: s.net.clone() });
    let own: Vec<Value> = read_json(&ad.own_file).and_then(|v| v["list"].as_array().cloned()).unwrap_or_default();
    for v in &own { ad.alle.lock().unwrap().insert(format!("{}/{}", v["pid"].as_str().unwrap_or(""), v["id"].as_str().unwrap_or("")), v.clone()); }
    *ad.own.lock().unwrap() = own;
    if let Some(d) = read_json(&ad.dec_file).and_then(|v| v["list"].as_array().cloned()) {
        for v in d { ad.ent.lock().unwrap().insert(format!("{}/{}", v["pid"].as_str().unwrap_or(""), v["vid"].as_str().unwrap_or("")), v); }
    }
    if A.set(ad.clone()).is_err() { return; }
    let w = Arc::downgrade(&ad);
    let k = Kanal::new(&s.net, "vors", KANAL, Arc::new(move |o: Value, sub: &str| {
        let Some(ad) = w.upgrade() else { return };
        let mut it = sub.split('/');
        let (Some(kind), Some(pid), Some(vid), None) = (it.next(), it.next(), it.next(), it.next()) else { return };
        if !hex16(pid) || clean_id(vid) != vid || vid.is_empty() { return; }
        let key = format!("{pid}/{vid}");
        let pk = o["pk"].as_str().unwrap_or("");
        let mut body = o.clone();
        let sig = body["sig"].as_str().unwrap_or("").to_string();
        if let Some(m) = body.as_object_mut() { m.remove("sig"); }
        if !crate::profil::check_sig(pk, &body.to_string(), &sig) { return; }
        let signer = crate::profil::pk_id(pk).unwrap_or_default();
        match kind {
            "v" => {
                if signer != pid || o["y"] != "vors" || o["id"].as_str() != Some(vid) { return; }
                if o["del"] == true { ad.alle.lock().unwrap().remove(&key); return; }
                let Ok(ents) = clean_ents(&o["ents"]) else { return };
                let n: String = o["n"].as_str().unwrap_or("").to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.').take(20).collect();
                ad.alle.lock().unwrap().insert(key, json!({"pid": pid, "id": vid, "n": n, "title": take_chars(o["title"].as_str().unwrap_or("").trim(), 60),
                    "text": take_chars(o["text"].as_str().unwrap_or("").trim(), 600), "base": o["base"].as_u64().unwrap_or(0), "t": o["t"].as_u64().unwrap_or(0),
                    "x": fin(&o["x"]), "z": fin(&o["z"]), "ents": ents}));
            }
            "e" => {
                if !is_admin_id(&signer) || o["y"] != "ent" || o["pid"].as_str() != Some(pid) || o["vid"].as_str() != Some(vid) { return; }
                let st = match o["st"].as_str() { Some("ok") => "ok", Some("nein") => "nein", _ => return };
                let d = json!({"pid": pid, "vid": vid, "st": st, "why": take_chars(o["why"].as_str().unwrap_or("").trim(), 300), "t": o["t"].as_u64().unwrap_or(0)});
                let mut e = ad.ent.lock().unwrap();
                if e.get(&key).is_some_and(|x| x["t"].as_u64().unwrap_or(0) > d["t"].as_u64().unwrap_or(0)) { return; }
                e.insert(key, d);
            }
            _ => {}
        }
    }));
    *ad.k.lock().unwrap() = Some(k);
    // eigene Vorschläge und (auf dem Admin-PC) die Entscheidungen regelmäßig erneut senden, damit sie gespeichert bleiben
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(12));
        loop {
            if let Some(ad) = a() { ad.republish(); }
            std::thread::sleep(std::time::Duration::from_secs(30 * 60));
        }
    });
}

impl Adm {
    fn send_signed(&self, mut v: Value, sub: &str) -> bool {
        let Some((pk, _)) = crate::profil::sign("") else { return false };
        v["pk"] = json!(pk);
        let Some((_, sig)) = crate::profil::sign(&v.to_string()) else { return false };
        v["sig"] = json!(sig);
        self.k.lock().unwrap().as_ref().is_some_and(|k| k.send_to(&v, sub, true))
    }
    fn save_own(&self) { write_json(&self.own_file, &json!({"list": *self.own.lock().unwrap()})); }
    fn save_dec(&self) {
        let me = crate::profil::key_id().unwrap_or_default();
        if !is_admin_id(&me) { return; }
        let list: Vec<Value> = self.ent.lock().unwrap().values().cloned().collect();
        write_json(&self.dec_file, &json!({"list": list}));
    }
    fn republish(&self) {
        let own = self.own.lock().unwrap().clone();
        for v in own { self.send_vors(&v); }
        if is_admin() {
            let list: Vec<Value> = self.ent.lock().unwrap().values().cloned().collect();
            for d in list { self.send_dec(&d); }
        }
    }
    fn send_vors(&self, v: &Value) -> bool {
        let r = json!({"y": "vors", "id": v["id"], "n": v["n"], "title": v["title"], "text": v["text"], "base": v["base"], "t": v["t"], "x": v["x"], "z": v["z"], "ents": v["ents"]});
        self.send_signed(r, &format!("v/{}/{}", v["pid"].as_str().unwrap_or(""), v["id"].as_str().unwrap_or("")))
    }
    fn send_dec(&self, d: &Value) -> bool {
        let r = json!({"y": "ent", "pid": d["pid"], "vid": d["vid"], "st": d["st"], "why": d["why"], "t": d["t"]});
        self.send_signed(r, &format!("e/{}/{}", d["pid"].as_str().unwrap_or(""), d["vid"].as_str().unwrap_or("")))
    }
}

pub fn list() -> Value {
    let Some(a) = a() else { return json!({"ok": false, "list": []}) };
    let me = crate::profil::key_id().unwrap_or_default();
    let ent = a.ent.lock().unwrap();
    let mut l: Vec<Value> = a.alle.lock().unwrap().iter().map(|(k, v)| vors_view(v, false, ent.get(k), &me)).collect();
    l.sort_by(|x, y| y["t"].as_u64().unwrap_or(0).cmp(&x["t"].as_u64().unwrap_or(0)));
    json!({"ok": true, "admin": is_admin(), "unlocked": unlocked(), "profil": crate::profil::my_id().is_some(), "list": l})
}
pub fn get(key: &str) -> Value {
    let Some(a) = a() else { return json!({"ok": false}) };
    let me = crate::profil::key_id().unwrap_or_default();
    let ent = a.ent.lock().unwrap();
    match a.alle.lock().unwrap().get(key) { Some(v) => { let mut x = vors_view(v, true, ent.get(key), &me); x["ok"] = json!(true); x } None => json!({"ok": false, "why": "Vorschlag nicht gefunden"}) }
}

pub fn neu(s: &Shared, b: &Value) -> Value {
    let Some(a) = a() else { return json!({"ok": false}) };
    let (Some(pid), Some(name)) = (crate::profil::my_id(), crate::profil::my_name()) else { return json!({"ok": false, "why": "Für Vorschläge brauchst du ein Profil (Freunde → Profil)", "needProfil": true}) };
    let title = take_chars(b["title"].as_str().unwrap_or("").trim(), 60);
    if title.chars().count() < 3 { return json!({"ok": false, "why": "Bitte kurz beschreiben, was du verbessert hast (Titel)"}); }
    let ents = match clean_ents(&b["ents"]) { Ok(e) => e, Err(e) => return json!({"ok": false, "why": e}) };
    let open = a.own.lock().unwrap().iter().filter(|v| !a.ent.lock().unwrap().contains_key(&format!("{}/{}", pid, v["id"].as_str().unwrap_or("")))).count();
    if open >= MAX_OWN { return json!({"ok": false, "why": format!("Du hast schon {MAX_OWN} offene Vorschläge – warte, bis der Admin sie angeschaut hat")}); }
    let vid = random_hex(6);
    let v = json!({"pid": pid, "id": vid, "n": name, "title": title, "text": take_chars(b["text"].as_str().unwrap_or("").trim(), 600), "base": s.karten.active_version(),
        "t": now_ms(), "x": fin(&b["x"]), "z": fin(&b["z"]), "ents": ents});
    if !a.send_vors(&v) { return json!({"ok": false, "why": "Senden fehlgeschlagen – keine Verbindung zum Vermittler"}); }
    a.alle.lock().unwrap().insert(format!("{pid}/{vid}"), v.clone());
    { let mut o = a.own.lock().unwrap(); o.push(v); let n = o.len(); if n > 40 { o.drain(..n - 40); } }
    a.save_own();
    log(format!("Straßen-Vorschlag gesendet: {title}"));
    json!({"ok": true, "key": format!("{pid}/{vid}")})
}

pub fn del(vid: &str) -> Value {
    let Some(a) = a() else { return json!({"ok": false}) };
    let Some(pid) = crate::profil::key_id() else { return json!({"ok": false}) };
    let vid = clean_id(vid);
    let had = { let mut o = a.own.lock().unwrap(); let n = o.len(); o.retain(|v| v["id"].as_str() != Some(vid.as_str())); n != o.len() };
    if !had { return json!({"ok": false, "why": "Nur eigene Vorschläge können gelöscht werden"}); }
    a.save_own();
    a.alle.lock().unwrap().remove(&format!("{pid}/{vid}"));
    a.send_signed(json!({"y": "vors", "id": vid, "del": true, "t": now_ms()}), &format!("v/{pid}/{vid}"));
    json!({"ok": true})
}

pub fn decide(key: &str, st: &str, why: &str) -> Value {
    let Some(a) = a() else { return json!({"ok": false}) };
    if !unlocked() { return json!({"ok": false, "why": "Nur als angemeldeter Admin", "needAdmin": true}); }
    let Some((pid, vid)) = key.split_once('/') else { return json!({"ok": false}) };
    if !a.alle.lock().unwrap().contains_key(key) { return json!({"ok": false, "why": "Vorschlag nicht gefunden"}); }
    let st = match st { "ok" => "ok", "nein" => "nein", _ => return json!({"ok": false}) };
    let d = json!({"pid": pid, "vid": vid, "st": st, "why": take_chars(why.trim(), 300), "t": now_ms()});
    if !a.send_dec(&d) { return json!({"ok": false, "why": "Senden fehlgeschlagen – keine Verbindung zum Vermittler"}); }
    a.ent.lock().unwrap().insert(key.to_string(), d);
    a.save_dec();
    log(format!("Vorschlag {key}: {}", if st == "ok" { "angenommen" } else { "abgelehnt" }));
    json!({"ok": true})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pbkdf2_rfc_vektor() {
        // RFC 7914 / bekannte PBKDF2-HMAC-SHA256-Testwerte: "passwd", "salt", 1 Runde
        let h = pbkdf2(b"passwd", b"salt", 1);
        assert_eq!(hex::encode(&h[..16]), "55ac046e56e3089fec1691c22544b605");
    }
}
