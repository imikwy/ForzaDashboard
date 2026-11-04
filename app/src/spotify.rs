// Spotify über die offizielle Web-API: Anmeldung mit dem eigenen Konto (OAuth mit PKCE, kein geheimer Schlüssel),
// danach zeigt das Dashboard Bibliothek, Suche und Wiedergabe und steuert die Spotify-App (Steuern braucht Premium).
// Der Zugangsschlüssel bleibt in der exe: das Dashboard fragt die API über /spotify/api/… an.
// Die Client-ID stammt aus einer eigenen Spotify-Entwickler-App (einstellungen.txt: spotify_client_id=…).
use crate::state::Shared;
use crate::util::{log, now_ms, random_hex, read_json, write_json};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing playlist-read-private playlist-read-collaborative user-library-read user-library-modify user-read-recently-played user-top-read user-read-private user-follow-read";

#[derive(Default)]
pub struct Sp {
    verifier: String,
    state: String,
    access: String,
    exp: u64,
    refresh: String,
    loaded: bool,
}

fn file(s: &Shared) -> PathBuf { s.file("spotify.json") }
fn client_id(s: &Shared) -> String { s.cfg.lock().unwrap().spotify_id.clone() }
pub fn redirect(s: &Shared) -> String { format!("http://127.0.0.1:{}/spotify/callback", s.web_port.load(Ordering::Relaxed)) }

fn enc(v: &str) -> String {
    v.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

fn agent() -> ureq::Agent {
    // Ein Agent für alle Anfragen (hält die Verbindung offen); HTTPS über die Windows-Verschlüsselung
    static A: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    A.get_or_init(|| {
        let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(20));
        if let Ok(tls) = native_tls::TlsConnector::new() { b = b.tls_connector(std::sync::Arc::new(tls)); }
        b.build()
    }).clone()
}

fn load(s: &Shared, sp: &mut Sp) {
    if sp.loaded { return; }
    sp.loaded = true;
    if let Some(v) = read_json(&file(s)) { sp.refresh = v["refresh"].as_str().unwrap_or("").to_string(); }
}

pub fn status(s: &Shared) -> Value {
    let mut sp = s.spotify.lock().unwrap();
    load(s, &mut sp);
    json!({"configured": !client_id(s).is_empty(), "loggedIn": !sp.refresh.is_empty(), "redirect": redirect(s)})
}

pub fn set_client(s: &Shared, id: &str) -> Value {
    let id: String = id.trim().chars().filter(|c| c.is_ascii_alphanumeric()).take(64).collect();
    s.cfg.lock().unwrap().spotify_id = id.clone();
    s.set_setting("spotify_client_id", &id);
    status(s)
}

// Anmelde-Adresse (PKCE: zufälliger Prüfwert, nur sein Hash geht an Spotify)
pub fn login_url(s: &Shared) -> Value {
    let id = client_id(s);
    if id.is_empty() { return json!({"ok": false, "why": "Erst die Client-ID eintragen"}); }
    let verifier = random_hex(48);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_hex(12);
    {
        let mut sp = s.spotify.lock().unwrap();
        sp.verifier = verifier;
        sp.state = state.clone();
    }
    let url = format!("https://accounts.spotify.com/authorize?response_type=code&client_id={}&scope={}&redirect_uri={}&code_challenge_method=S256&code_challenge={}&state={}",
        enc(&id), enc(SCOPES), enc(&redirect(s)), challenge, state);
    json!({"ok": true, "url": url})
}

fn take_tokens(s: &Shared, v: &Value) {
    let mut sp = s.spotify.lock().unwrap();
    sp.access = v["access_token"].as_str().unwrap_or("").to_string();
    sp.exp = now_ms() + v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60) * 1000;
    if let Some(r) = v["refresh_token"].as_str() { sp.refresh = r.to_string(); }
    let refresh = sp.refresh.clone();
    drop(sp);
    write_json(&file(s), &json!({"refresh": refresh}));
}

pub fn callback(s: &Shared, code: &str, state: &str) -> Result<(), String> {
    let (verifier, ok) = {
        let sp = s.spotify.lock().unwrap();
        (sp.verifier.clone(), !sp.state.is_empty() && sp.state == state)
    };
    if !ok { return Err("Anmeldung abgelaufen – bitte im Dashboard neu starten".into()); }
    let id = client_id(s);
    let r = agent().post("https://accounts.spotify.com/api/token").send_form(&[
        ("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect(s)), ("client_id", &id), ("code_verifier", &verifier),
    ]);
    match r {
        Ok(resp) => {
            let v: Value = serde_json::from_str(&resp.into_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            take_tokens(s, &v);
            log("Spotify: angemeldet");
            Ok(())
        }
        Err(ureq::Error::Status(c, resp)) => Err(format!("Spotify lehnt ab ({c}): {}", resp.into_string().unwrap_or_default())),
        Err(e) => Err(e.to_string()),
    }
}

fn token(s: &Shared, force: bool) -> Result<String, String> {
    let refresh = {
        let mut sp = s.spotify.lock().unwrap();
        load(s, &mut sp);
        if !force && !sp.access.is_empty() && now_ms() < sp.exp { return Ok(sp.access.clone()); }
        sp.refresh.clone()
    };
    if refresh.is_empty() { return Err("nicht angemeldet".into()); }
    let id = client_id(s);
    match agent().post("https://accounts.spotify.com/api/token").send_form(&[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", &id)]) {
        Ok(resp) => {
            let v: Value = serde_json::from_str(&resp.into_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            take_tokens(s, &v);
            Ok(s.spotify.lock().unwrap().access.clone())
        }
        Err(ureq::Error::Status(400, _)) => { logout(s); Err("Anmeldung abgelaufen".into()) }
        Err(e) => Err(e.to_string()),
    }
}

pub fn logout(s: &Shared) {
    *s.spotify.lock().unwrap() = Sp { loaded: true, ..Default::default() };
    let _ = std::fs::remove_file(file(s));
}

// Weiterleitung an die Web-API mit dem gespeicherten Schlüssel; liefert (Status, Antworttext)
pub fn api(s: &Shared, method: &str, path: &str, query: &str, body: &[u8]) -> (u16, String) {
    if path.contains("..") { return (400, String::new()); }
    let url = format!("https://api.spotify.com/v1/{path}{}{query}", if query.is_empty() { "" } else { "?" });
    for attempt in 0..2 {
        let tok = match token(s, attempt == 1) { Ok(t) => t, Err(e) => return (401, json!({"error": e}).to_string()) };
        let req = agent().request(method, &url).set("Authorization", &format!("Bearer {tok}"));
        let r = if method == "GET" || method == "DELETE" && body.is_empty() { req.call() } else { req.set("Content-Type", "application/json").send_bytes(body) };
        match r {
            Ok(resp) => { let st = resp.status(); return (st, resp.into_string().unwrap_or_default()); }
            Err(ureq::Error::Status(401, _)) if attempt == 0 => continue,
            Err(ureq::Error::Status(c, resp)) => return (c, resp.into_string().unwrap_or_default()),
            Err(e) => return (502, json!({"error": e.to_string()}).to_string()),
        }
    }
    (401, String::new())
}
