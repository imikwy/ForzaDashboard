// Webserver: Dashboard für dieses Fenster und für Handy/Tablet im WLAN, Live-Daten per Server-Sent Events
use crate::state::Shared;
use crate::util::{fmt_code, lan_ips, now_ms, take_chars};
use crate::{dienst, karte, media, updates, veroeffentlichen, APP_VER};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::Manager;
use tokio_stream::wrappers::BroadcastStream;

type S = State<Arc<Shared>>;
type Q = Query<HashMap<String, String>>;
type Addr = ConnectInfo<SocketAddr>;

static HTML: &str = include_str!("../../src/dashboard.html");
static VORLAGEN: &[u8] = include_bytes!("../../src/fh6-vorlagen.js");
static FH6_KARTE: &[u8] = include_bytes!("../../src/fh6-karte.js");
// Tuning: FH6-Autos (Forza-Wiki, CC BY-SA) und Builds mit Tuning-Code (ForzaFire)
static TUNING_DATEN: &[u8] = include_bytes!("../../src/tuning-daten.json");
static BEISPIELE: &[u8] = include_bytes!("../../src/community-beispiele.json");   // Beispiel-Beiträge der Community (nur lokal)

pub fn html_ok() -> bool {
    HTML.contains("const APP_VER") && HTML.contains("fh6-karte.js") && VORLAGEN.len() > 100_000 && FH6_KARTE.len() > 100_000
}

pub fn router(s: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/fh6-vorlagen.js", get(|h: HeaderMap| async move { asset(&h, VORLAGEN, 0) }))
        .route("/fh6-karte.js", get(|h: HeaderMap| async move { asset(&h, FH6_KARTE, 1) }))
        .route("/tuning-daten.json", get(|h: HeaderMap| async move { asset(&h, TUNING_DATEN, 2) }))
        .route("/community-beispiele.json", get(|h: HeaderMap| async move { asset(&h, BEISPIELE, 3) }))
        .route("/events", get(events))
        .route("/info", get(info))
        .route("/media", get(media_get))
        // Navi-Ansage: Musik kurz leiser (f = Anteil der Lautstärke, ms = längstens), danach zurück
        .route("/media/duck", get(|Query(q): Q| async move {
            let f = q.get("f").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.5).clamp(0.15, 1.0);
            let ms = q.get("ms").and_then(|v| v.parse::<u64>().ok()).unwrap_or(8000).clamp(500, 20000);
            tauri::async_runtime::spawn_blocking(move || media::duck(f, ms));
            j(json!({"ok": true}))
        }))
        // Mods (Nexus, nur FH6): Stöbern für alle, Ändern nur vom PC selbst
        .route("/mods/status", get(|State(s): S| async move { j(tokio::task::spawn_blocking(move || crate::mods::status(&s)).await.unwrap_or(json!({}))) }))
        .route("/mods/list", get(|State(s): S, Query(q): Q| async move { let k = q.get("kind").cloned().unwrap_or_default(); j(tokio::task::spawn_blocking(move || crate::mods::list(&s, &k)).await.unwrap_or(json!({}))) }))
        .route("/mods/detail", get(|State(s): S, Query(q): Q| async move { let id = q.get("id").and_then(|v| v.parse().ok()).unwrap_or(0); j(tokio::task::spawn_blocking(move || crate::mods::detail(&s, id)).await.unwrap_or(json!({}))) }))
        .route("/mods/key", post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); let k = b["key"].as_str().unwrap_or("").to_string(); j(tokio::task::spawn_blocking(move || crate::mods::set_key(&s, &k)).await.unwrap_or(json!({}))) }))
        .route("/mods/pickdir", post(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(tokio::task::spawn_blocking(move || crate::mods::pick_dir(&s)).await.unwrap_or(json!({}))) }))
        .route("/mods/autodir", post(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(tokio::task::spawn_blocking(move || crate::mods::auto_dir(&s)).await.unwrap_or(json!({}))) }))
        .route("/mods/dir", post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); j(crate::mods::set_dir(&s, b["dir"].as_str().unwrap_or(""))) }))
        .route("/mods/install", get(|State(s): S, ConnectInfo(a): Addr, Query(q): Q| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let n = |k: &str| q.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0); crate::mods::install(s.clone(), n("mod"), n("file"), None); j(json!({"ok": true})) }))
        .route("/mods/uninstall", get(|State(s): S, ConnectInfo(a): Addr, Query(q): Q| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let id = q.get("mod").and_then(|v| v.parse().ok()).unwrap_or(0); j(tokio::task::spawn_blocking(move || crate::mods::uninstall(&s, id)).await.unwrap_or(json!({}))) }))
        .route("/mods/register", get(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::mods::register_nxm(&s)) }))
        .route("/mods/local", get(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(tokio::task::spawn_blocking(move || crate::mods::local(&s)).await.unwrap_or(json!({}))) }))
        .route("/mods/toggle", get(|State(s): S, ConnectInfo(a): Addr, Query(q): Q| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let k = q.get("key").cloned().unwrap_or_default(); let on = q.get("on").map(|v| v == "1").unwrap_or(false); j(tokio::task::spawn_blocking(move || crate::mods::toggle(&s, &k, on)).await.unwrap_or(json!({}))) }))
        .route("/mods/remove", get(|State(s): S, ConnectInfo(a): Addr, Query(q): Q| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let k = q.get("key").cloned().unwrap_or_default(); j(tokio::task::spawn_blocking(move || crate::mods::remove(&s, &k)).await.unwrap_or(json!({}))) }))
        .route("/game/start", get(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(tokio::task::spawn_blocking(move || crate::mods::start_game(&s)).await.unwrap_or(json!({}))) }))
        .route("/tunes", get(|State(s): S| async move { j(tokio::task::spawn_blocking(move || crate::tuning::list(&s)).await.unwrap_or(json!({}))) }))
        .route("/tunes/add", post(|State(s): S, b: Bytes| async move { let b = body(&b); let r = tokio::task::spawn_blocking(move || { let r = crate::tuning::add(&s, &b); if r["ok"] == true { crate::profil::activity("post", b["title"].as_str().unwrap_or("Beitrag")); } r }).await.unwrap_or(json!({})); j(r) }))
        .route("/tunes/del", post(|b: Bytes| async move { let b = body(&b); j(crate::tuning::del(b["id"].as_str().unwrap_or(""))) }))
        .route("/tunes/like", post(|b: Bytes| async move { let b = body(&b); j(crate::tuning::like(b["key"].as_str().unwrap_or(""), b["on"].as_bool().unwrap_or(false))) }))
        .route("/tunes/comment", post(|b: Bytes| async move { let b = body(&b); j(crate::tuning::comment(b["key"].as_str().unwrap_or(""), b["x"].as_str().unwrap_or(""))) }))
        .route("/tunes/uncomment", post(|b: Bytes| async move { let b = body(&b); j(crate::tuning::uncomment(b["id"].as_str().unwrap_or(""))) }))
        .route("/tunes/go", get(|ConnectInfo(a): Addr, Query(q): Q| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::tuning::go(q.get("key").map(String::as_str).unwrap_or(""), q.get("on").map(|v| v == "1").unwrap_or(false))) }))
        .route("/tunes/photo", get(|Query(q): Q| async move { tune_photo(q.get("k").map(String::as_str).unwrap_or("")) }))
        .route("/overlay", get(overlay_page))
        // Spieler-Profile
        // Admin (nur von diesem PC aus) und Straßen-Vorschläge
        .route("/admin", get(|| async move { j(crate::admin::status()) }))
        .route("/admin/login", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); j(tokio::task::spawn_blocking(move || crate::admin::login(b["pw"].as_str().unwrap_or(""))).await.unwrap_or(json!({}))) }))
        .route("/admin/logout", post(|ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::admin::logout()) }))
        .route("/admin/passwort", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); j(tokio::task::spawn_blocking(move || crate::admin::set_pw(b["neu"].as_str().unwrap_or(""), b["alt"].as_str().unwrap_or(""))).await.unwrap_or(json!({}))) }))
        .route("/vorschlaege", get(|| async move { j(crate::admin::list()) }))
        .route("/vorschlaege/get", get(|Query(q): Q| async move { j(crate::admin::get(q.get("k").map(String::as_str).unwrap_or(""))) }))
        .route("/vorschlaege/neu", post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); blocking(s, move |s| crate::admin::neu(&s, &b)).await }))
        .route("/vorschlaege/del", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::admin::del(body(&b)["vid"].as_str().unwrap_or(""))) }))
        .route("/vorschlaege/entscheiden", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); j(crate::admin::decide(b["k"].as_str().unwrap_or(""), b["st"].as_str().unwrap_or(""), b["why"].as_str().unwrap_or(""))) }))
        .route("/profil", get(|| async move { j(crate::profil::me()) }))
        .route("/profil/alle", get(|| async move { j(crate::profil::all()) }))
        .route("/profil/get", get(|Query(q): Q| async move { j(crate::profil::get(q.get("id").map(String::as_str).unwrap_or(""))) }))
        .route("/profil/check", get(|Query(q): Q| async move { j(crate::profil::check(q.get("u").map(String::as_str).unwrap_or(""))) }))
        .route("/profil/bild", get(|Query(q): Q| async move { prof_img(q.get("id").map(String::as_str).unwrap_or("")) }))
        .route("/profil/set", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::profil::set(&body(&b))) }))
        .route("/profil/bild/set", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::profil::set_avatar(String::from_utf8_lossy(&b).trim())) }))
        .route("/profil/drive", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::profil::drive(&body(&b))) }))
        .route("/profil/extra", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::profil::set_extra(&body(&b))) }))
        .route("/profil/hintergrund", get(|Query(q): Q| async move { prof_bg(q.get("id").map(String::as_str).unwrap_or("")) }))
        .route("/profil/hintergrund/set", post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::profil::set_background(String::from_utf8_lossy(&b).trim())) }))
        .route("/profil/del", post(|ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::profil::remove()) }))
        .route("/sys", get(|| async move { j(crate::system::last()) }))
        .route("/ov/state", get(|State(s): S| async move { let on = s.app.get().map(crate::overlay::is_on).unwrap_or(false); j(json!({"on": on})) }))
        .route("/ov/last", get(|| async move { j(crate::overlay::last()) }))
        .route("/ov/toggle", get(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let Some(app) = s.app.get().cloned() else { return j(json!({"ok": false})) }; let on = tokio::task::spawn_blocking(move || crate::overlay::toggle(&app, &s)).await.unwrap_or(false); j(json!({"ok": true, "on": on})) }))
        .route("/ov/layout", get(|State(s): S| async move { j(crate::overlay::layout(&s)) }).post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(json!({"ok": crate::overlay::set_layout(&s, body(&b))})) }))
        .route("/overlay/vorschau.jpg", get(|| async move { ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "max-age=86400")], OVERLAY_VORSCHAU).into_response() }))
        .route("/ov/map", get(|| async move { let b = crate::overlay::map(); if b.is_empty() { return (StatusCode::NOT_FOUND, "").into_response(); } ([(header::CONTENT_TYPE, "image/webp"), (header::CACHE_CONTROL, "no-store")], b).into_response() }).post(|ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(json!({"ok": crate::overlay::set_map(&b)})) }))
        // eigener Blinker (für Mitspieler sichtbar); das Dashboard meldet ihn bei jeder Änderung und alle 2 s
        .route("/blink", get(|ConnectInfo(a): Addr, Query(q): Query<std::collections::HashMap<String, String>>| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } crate::state::blink_set(q.get("m").map(|s| s.as_str()).unwrap_or("")); j(json!({"ok": true})) }))
        .route("/ov/push", post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } crate::overlay::push(&s, body(&b)); j(json!({"ok": true})) }))
        .route("/mods/import", post(|State(s): S, ConnectInfo(a): Addr, Query(q): Q, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let n = q.get("name").cloned().unwrap_or_default(); j(tokio::task::spawn_blocking(move || crate::mods::import_stage(&s, &n, &b)).await.unwrap_or(json!({"ok": false}))) }).layer(DefaultBodyLimit::max(1536 << 20)))
        .route("/mods/import/apply", post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); let (id, v, n) = (b["id"].as_str().unwrap_or("").to_string(), b["variant"].as_str().unwrap_or("").to_string(), b["name"].as_str().unwrap_or("").to_string()); j(tokio::task::spawn_blocking(move || crate::mods::import_apply(&s, &id, &v, &n)).await.unwrap_or(json!({"ok": false}))) }))
        .route("/mods/import/cancel", post(|State(s): S, b: Bytes| async move { let b = body(&b); j(crate::mods::import_cancel(&s, b["id"].as_str().unwrap_or(""))) }))
        // ReShade-Menütaste (KeyOverlay in ReShade.ini): lesen bzw. setzen
        .route("/mods/reshade/key", get(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::mods::rs_key(&s, None)) })
            .post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(crate::mods::rs_key(&s, Some(body(&b)))) }))
        .route("/mods/rename", post(|State(s): S, ConnectInfo(a): Addr, b: Bytes| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } let b = body(&b); j(crate::mods::rename(&s, b["mod"].as_u64().unwrap_or(0), b["name"].as_str().unwrap_or(""))) }))
        .route("/mods/open", get(|ConnectInfo(a): Addr, Query(q): Q| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(json!({"ok": crate::mods::open_page(q.get("url").map(String::as_str).unwrap_or(""))})) }))
        .route("/media/unduck", get(|| async { tauri::async_runtime::spawn_blocking(media::unduck); j(json!({"ok": true})) }))
        .route("/media/{cmd}", get(media_cmd))
        .route("/marks", get(|| async { json_str("[]".into()) }))
        .route("/quit", get(quit))
        .route("/window", get(window))
        .route("/app", get(app_ctl))
        // Spotify (Web-API): Anmeldung, Einstellungen, Weiterleitung der API-Anfragen
        .route("/spotify/status", get(|State(s): S| async move { j(crate::spotify::status(&s)) }))
        .route("/spotify/config", post(|State(s): S, b: Bytes| async move { let b = body(&b); j(crate::spotify::set_client(&s, b["id"].as_str().unwrap_or(""))) }))
        .route("/spotify/login", get(|State(s): S| async move { j(crate::spotify::login_url(&s)) }))
        .route("/spotify/logout", post(|State(s): S| async move { crate::spotify::logout(&s); j(json!({"ok": true})) }))
        .route("/spotify/callback", get(sp_callback))
        .route("/spotify/api/{*path}", axum::routing::any(sp_api))
        .route("/log", get(|ConnectInfo(addr): Addr, Query(q): Q| async move {
            if addr.ip().is_loopback() { if let Some(m) = q.get("m") { crate::util::log(format!("Fenster: {}", take_chars(m, 500))); } }
            json_str("{}".into())
        }))
        // Freunde und Lobbys
        .route("/friends/config", get(friends_config))
        .route("/lobby", get(|State(s): S| async move { j(json!({"liste": s.lobbys().list(), "name": s.name(), "on": s.fr_on(), "ok": s.net_ok()})) }))
        .route("/lobby/neu", post(|State(s): S, b: Bytes| async move { let b = body(&b); let r = s.lobbys().create(b["name"].as_str().unwrap_or(""), &s.name()); if r["ok"] == true { crate::profil::activity("kol", b["name"].as_str().unwrap_or("Kolonne")); } lobby_resp(&s, r) }))
        .route("/lobby/beitreten", post(|State(s): S, b: Bytes| async move { let b = body(&b); let r = s.lobbys().join(b["code"].as_str().unwrap_or("")); if r["ok"] == true { if let Some((n, _)) = s.lobbys().find(b["code"].as_str().unwrap_or("")) { crate::profil::activity("kol", &n); } } lobby_resp(&s, r) }))
        .route("/lobby/verlassen", post(|State(s): S, b: Bytes| async move { let b = body(&b); let code = b["code"].as_str().unwrap_or(""); s.sozial().einladungen_zurueck(&crate::util::norm_code(code)); let ok = s.lobbys().leave(code); lobby_resp(&s, json!({"ok": ok})) }))
        .route("/lobby/an", post(|State(s): S, b: Bytes| async move { let b = body(&b); let code = b["code"].as_str().unwrap_or(""); let an = b["an"].as_bool().unwrap_or(false); if !an { s.sozial().einladungen_zurueck(&crate::util::norm_code(code)); } let ok = s.lobbys().set_on(code, an); lobby_resp(&s, json!({"ok": ok})) }))
        // Freunde, Einladungen, Online-Modus
        .route("/sozial", get(|State(s): S| async move { j(s.sozial().state()) }))
        .route("/sozial/anfrage", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.sozial().anfrage(b["id"].as_str().unwrap_or(""), b["n"].as_str().unwrap_or("Fahrer"), &s.name()); j(json!({"ok": ok, "soz": s.sozial().state()})) }))
        .route("/sozial/annehmen", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.sozial().annehmen(b["id"].as_str().unwrap_or(""), &s.name()); j(json!({"ok": ok, "soz": s.sozial().state()})) }))
        .route("/sozial/ablehnen", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.sozial().ablehnen(b["id"].as_str().unwrap_or("")); j(json!({"ok": ok, "soz": s.sozial().state()})) }))
        .route("/sozial/zurueckziehen", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.sozial().zurueckziehen(b["id"].as_str().unwrap_or("")); j(json!({"ok": ok, "soz": s.sozial().state()})) }))
        .route("/sozial/entfernen", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.sozial().entfernen(b["id"].as_str().unwrap_or("")); j(json!({"ok": ok, "soz": s.sozial().state()})) }))
        .route("/sozial/einladen", post(|State(s): S, b: Bytes| async move {
            let b = body(&b);
            let Some((ln, code)) = s.lobbys().find(b["code"].as_str().unwrap_or("")) else { return j(json!({"ok": false, "why": "Erst eine Kolonne starten"})) };
            let ok = s.sozial().einladen(b["id"].as_str().unwrap_or(""), &code, &ln, &s.name());
            j(json!({"ok": ok, "soz": s.sozial().state()}))
        }))
        .route("/sozial/einladung", post(|State(s): S, b: Bytes| async move {
            let b = body(&b);
            let code = s.sozial().einladung(b["id"].as_str().unwrap_or(""), b["an"].as_bool().unwrap_or(false));
            let r = match code { Some(c) => s.lobbys().join(&c), None => json!({"ok": b["an"].as_bool() != Some(true), "why": "Die Einladung ist abgelaufen"}) };
            lobby_resp(&s, r)
        }))
        .route("/sozial/welt", post(|State(s): S, b: Bytes| async move {
            let on = body(&b)["on"].as_bool().unwrap_or(false);
            s.cfg.lock().unwrap().online = on; s.set_setting("online_modus", if on { "ja" } else { "nein" });
            s.sozial().set_welt(on && s.fr_on());
            j(json!({"ok": true, "soz": s.sozial().state()}))
        }))
        .route("/lobby/wegpunkt", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.lobbys().send_wp(&b, &s.name()); j(json!({"ok": ok})) }))
        .route("/lobby/chat", post(|State(s): S, b: Bytes| async move { let b = body(&b); j(s.lobbys().send_chat(&b, &s.name())) }))
        .route("/lobby/kick", post(|State(s): S, b: Bytes| async move { let b = body(&b); j(s.lobbys().vote(&b, &s.name())) }))
        .route("/lobby/wegpunkt/weg", post(|State(s): S| async move { let ok = s.lobbys().clear_wp(); j(json!({"ok": ok})) }))
        .route("/lobby/name", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.lobbys().rename(b["code"].as_str().unwrap_or(""), b["name"].as_str().unwrap_or("")); lobby_resp(&s, json!({"ok": ok})) }))
        // gelernte Straßen
        .route("/learned/version", get(|State(s): S| async move { j(json!({"v": s.lver.load(Ordering::Relaxed)})) }))
        .route("/friendlr/version", get(|State(s): S| async move { j(json!({"v": s.flr.lock().unwrap().v})) }))
        .route("/friendlr", get(friendlr))
        .route("/learned/lease", get(lease))
        .route("/learned", get(learned_get).post(learned_post))
        .route("/friendlr/import", post(friendlr_import))
        // Karte
        .route("/map", get(|State(s): S| async move { blocking(s, |s| dienst::map_get(&s)).await }).post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| dienst::map_post(&s, &b)).await }))
        .route("/map/presence", post(|State(s): S, b: Bytes| async move { dienst::presence(&s, &body(&b)); j(json!({"ok": true})) }))
        .route("/map/import", post(|State(s): S, ConnectInfo(addr): Addr, b: Bytes| async move {
            if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
            let b = body(&b);
            blocking(s, move |s| dienst::map_import(&s, &b)).await
        }))
        .route("/karte/info", get(|State(s): S| async move { let mut i = s.karten.info(); i["updates"] = updates::state(&s); j(i) }))
        .route("/karte/version", post(|State(s): S, b: Bytes| async move {
            let v = body(&b)["v"].as_u64().unwrap_or(0);
            let r = s.karten.activate(v);
            if r.is_ok() { dienst::on_map_version(&s); }
            j(json!({"ok": r.is_ok(), "why": r.err(), "info": s.karten.info()}))
        }))
        .route("/karte/export", get(karte_export))
        // Bearbeiten-Sitzung
        .route("/sitzung", get(|State(s): S| async move { blocking(s, |s| dienst::sess_info(&s)).await }))
        .route("/sitzung/neu", post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| if !crate::admin::unlocked() { merge(json!({"ok": false, "needAdmin": true, "why": "Die offizielle Karte bearbeitet nur der Admin – als Spieler kannst du Verbesserungen vorschlagen"}), dienst::sess_info(&s)) } else { merge(s.sitz().create(b["title"].as_str().unwrap_or("")), dienst::sess_info(&s)) }).await }))
        .route("/sitzung/beitreten", post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| if !crate::admin::unlocked() { merge(json!({"ok": false, "needAdmin": true, "why": "Die offizielle Karte bearbeitet nur der Admin – als Spieler kannst du Verbesserungen vorschlagen"}), dienst::sess_info(&s)) } else { merge(s.sitz().join(b["code"].as_str().unwrap_or("")), dienst::sess_info(&s)) }).await }))
        .route("/sitzung/verlassen", post(|State(s): S| async move { blocking(s, |s| { s.sitz().close(true); merge(json!({"ok": true}), dienst::sess_info(&s)) }).await }))
        .route("/sitzung/umstellen", post(|State(s): S| async move { blocking(s, |s| {
            let Some(c) = s.sitz().cur().filter(|c| c.is_host()) else { return json!({"ok": false, "why": "Nur der Host kann die Sitzung umstellen"}) };
            let v = s.karten.active_version();
            if v == 0 || v == c.base() { return json!({"ok": false, "why": "Die Sitzung ist schon auf der neuesten Version"}); }
            c.rebase_to(v);
            merge(json!({"ok": true}), dienst::sess_info(&s))
        }).await }))
        .route("/sitzung/verlauf", get(|State(s): S| async move { blocking(s, |s| match s.sitz().cur() {
            Some(c) => json!({"ok": true, "me": s.sid, "list": c.history()}),
            None => json!({"ok": false, "list": []}),
        }).await }))
        .route("/sitzung/schritt", get(|State(s): S, Query(q): Q| async move { blocking(s, move |s| {
            s.sitz().cur().and_then(|c| c.detail(q.get("g").map(String::as_str).unwrap_or(""))).map(|d| merge(json!({"ok": true}), d)).unwrap_or(json!({"ok": false}))
        }).await }))
        .route("/sitzung/rueckgaengig", post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| match s.sitz().cur() {
            Some(c) => c.undo(b["g"].as_str().unwrap_or(""), b["force"].as_bool().unwrap_or(false)),
            None => json!({"ok": false, "why": "Keine Sitzung"}),
        }).await }))
        .route("/sitzung/strgz", post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| {
            let Some(c) = s.sitz().cur() else { return json!({"ok": false, "why": "Keine Sitzung"}) };
            let redo = b["redo"].as_bool().unwrap_or(false);
            let Some(g) = c.last_own(redo) else { return json!({"ok": false, "why": if redo { "Nichts zum Wiederholen" } else { "Nichts zum Rückgängigmachen" }}) };
            merge(c.undo(&g, b["force"].as_bool().unwrap_or(false)), json!({"g": g}))
        }).await }))
        .route("/sitzung/veroeffentlichen", post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| {
            let Some(c) = s.sitz().cur().filter(|c| c.ready()) else { return json!({"ok": false, "why": "Keine Sitzung aktiv"}) };
            if !crate::admin::unlocked() { return json!({"ok": false, "needAdmin": true, "why": "Die offizielle Karte bearbeitet nur der Admin – als Spieler kannst du Verbesserungen vorschlagen"}); }
            if b["vorschau"].as_bool().unwrap_or(false) { return veroeffentlichen::preview(&c, &s.karten, &s.repo_root()); }
            let r = veroeffentlichen::publish(&c, &s.repo_root(), b["notiz"].as_str().unwrap_or(""), &s.name(), b["massenloeschung"].as_bool().unwrap_or(false));
            if r["ok"] == true { c.send(json!({"y": "pub", "n": s.name()})); }
            r
        }).await }))
        // Updates
        .route("/updates", get(|State(s): S| async move { j(updates::state(&s)) }))
        .route("/hb", post(|State(s): S, b: Bytes| async move { s.last_hb.store(crate::util::now_ms(), std::sync::atomic::Ordering::Relaxed); s.hb_vis.store(&b[..] != b"hidden", std::sync::atomic::Ordering::Relaxed); j(json!({"ok": true})) }))
        .route("/updates/jetzt", post(|State(s): S, ConnectInfo(a): Addr| async move { if !a.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); } j(tokio::task::spawn_blocking(move || updates::install_now(&s)).await.unwrap_or(json!({}))) }))
        .route("/updates/pruefen", post(|State(s): S| async move { s.upd.lock().unwrap().last = 0; j(json!({"ok": true})) }))
        .fallback(|| async { (StatusCode::NOT_FOUND, "Nicht gefunden") })
        .layer(DefaultBodyLimit::max(64 << 20))
        .with_state(s)
}

fn json_str(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

fn j(v: Value) -> Response {
    json_str(v.to_string())
}

fn body(b: &Bytes) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) { for (k, v) in b { a.entry(k).or_insert(v); } }
    a
}

fn lobby_resp(s: &Shared, mut r: Value) -> Response {
    r["liste"] = s.lobbys().list();
    j(r)
}

// Arbeit mit Dateien/Karte nicht im Webserver-Takt erledigen
async fn blocking(s: Arc<Shared>, f: impl FnOnce(Arc<Shared>) -> Value + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(move || f(s)).await {
        Ok(v) => j(v),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Interner Fehler").into_response(),
    }
}

// große Kartenbilder: einmal komprimiert, danach aus dem Zwischenspeicher des Browsers
fn asset(h: &HeaderMap, data: &'static [u8], slot: usize) -> Response {
    static GZ: [OnceLock<Vec<u8>>; 4] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let tag = format!("\"{}-{}\"", APP_VER, data.len());
    if h.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(tag.as_str()) { return StatusCode::NOT_MODIFIED.into_response(); }
    let gz = h.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("gzip"));
    let base = [(header::CONTENT_TYPE, "text/javascript; charset=utf-8".to_string()), (header::CACHE_CONTROL, "no-cache".to_string()), (header::ETAG, tag)];
    if gz {
        let b = GZ[slot].get_or_init(|| crate::util::gzip(data));
        (base, [(header::CONTENT_ENCODING, "gzip")], b.clone()).into_response()
    } else {
        (base, data).into_response()
    }
}

fn prof_bg(id: &str) -> Response {
    use base64::Engine;
    let Some(d) = crate::profil::background(id) else { return (StatusCode::NOT_FOUND, "").into_response() };
    let Some((head, b64)) = d.split_once(",") else { return (StatusCode::NOT_FOUND, "").into_response() };
    let ct = if head.contains("webp") { "image/webp" } else { "image/jpeg" };
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "max-age=60")], b).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "").into_response(),
    }
}
// Foto eines Tuning-Beitrags (liegt als data:-URL vor) als Bild ausliefern
fn prof_img(id: &str) -> Response {
    use base64::Engine;
    let Some(d) = crate::profil::avatar(id) else { return (StatusCode::NOT_FOUND, "").into_response() };
    let Some((head, b64)) = d.split_once(",") else { return (StatusCode::NOT_FOUND, "").into_response() };
    let ct = if head.contains("webp") { "image/webp" } else { "image/jpeg" };
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "max-age=60")], b).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "").into_response(),
    }
}
fn tune_photo(k: &str) -> Response {
    use base64::Engine;
    let Some(d) = crate::tuning::photo(k) else { return (StatusCode::NOT_FOUND, "").into_response() };
    let Some((head, b64)) = d.split_once(",") else { return (StatusCode::NOT_FOUND, "").into_response() };
    let ct = if head.contains("webp") { "image/webp" } else { "image/jpeg" };
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "max-age=600")], b).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "").into_response(),
    }
}
static OVERLAY_VORSCHAU: &[u8] = include_bytes!("../../src/overlay-vorschau.jpg");
static OVERLAY_HTML: &str = include_str!("../../src/overlay.html");
async fn overlay_page(State(s): S) -> Response {
    // eine overlay.html im Datenordner hat Vorrang (zum Ausprobieren ohne neu zu bauen)
    let body = std::fs::read_to_string(s.file("overlay.html")).unwrap_or_else(|_| OVERLAY_HTML.to_string());
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

async fn index(State(s): S) -> Response {
    // eine dashboard.html im Datenordner hat Vorrang (zum Ausprobieren ohne neu zu bauen)
    let body = std::fs::read_to_string(s.file("dashboard.html")).unwrap_or_else(|_| HTML.to_string());
    let body = match body.find("const APP_VER = \"") {
        Some(i) => match body[i + 17..].find('"') { Some(e) => format!("{}{}{}", &body[..i + 17], APP_VER, &body[i + 17 + e..]), None => body },
        None => body,
    };
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

async fn events(State(s): S) -> impl IntoResponse {
    let first = s.latest.lock().unwrap().clone();
    let mut hello = Event::default().retry(Duration::from_millis(1000));
    if let Some(d) = first { hello = hello.data(&*d); }
    let s2 = s.clone();
    let sess = tokio::task::spawn_blocking(move || dienst::sess_info(&s2)).await.unwrap_or_default();
    let sess_ev = Event::default().event("sess").data(sess.to_string());
    let live = BroadcastStream::new(s.tx.subscribe()).filter_map(|r| async move {
        let ev = r.ok()?;
        let mut e = Event::default().data(&*ev.data);
        if let Some(n) = ev.name { e = e.event(n); }
        Some(Ok::<_, Infallible>(e))
    });
    let st: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(stream::iter([Ok(hello), Ok(sess_ev)]).chain(live));
    let sse = Sse::new(st).keep_alive(KeepAlive::new().interval(Duration::from_secs(2)).text("ping"));
    ([(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")], sse)
}

async fn info(State(s): S) -> Response {
    j(json!({
        "udpPort": s.udp_port.load(Ordering::Relaxed), "webPort": s.web_port.load(Ordering::Relaxed),
        "packets": s.packets.load(Ordering::Relaxed), "packetLen": s.last_len.load(Ordering::Relaxed),
        "ips": lan_ips(), "markKey": "", "ver": APP_VER, "dev": s.sid, "basis": karte::base_hash(), "app": "fd4",
    }))
}

pub fn media_refresh(s: &Arc<Shared>) {
    let s = s.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let spv = s.cfg.lock().unwrap().spv_on;
        let m = media::poll(spv);
        *s.media.lock().unwrap() = m;
    });
}

async fn media_get(State(s): S) -> Response {
    let now = now_ms();
    if now.saturating_sub(s.media_wanted.swap(now, Ordering::Relaxed)) > 10000 { media_refresh(&s); }
    let m = s.media.lock().unwrap().clone();
    let mut o = serde_json::to_value(&m).unwrap_or_default();
    o["vol"] = json!(m.vol);
    o["volMode"] = json!(if m.vol.is_some() { "spotify" } else { "system" });
    j(o)
}

async fn media_cmd(State(s): S, Path(cmd): Path<String>) -> Response {
    let s2 = s.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let spv = s2.cfg.lock().unwrap().spv_on;
        if cmd == "open" {
            let _ = crate::util::no_window(std::process::Command::new("cmd").args(["/c", "start", "", "spotify:"])).spawn();
            return;
        }
        // Lautstärke: nur Spotify (Windows-Mixer), sonst die PC-Lautstärke
        if cmd == "volup" || cmd == "voldown" {
            if spv {
                if let Some(v) = media::spotify_volume(Some(if cmd == "volup" { 0.06 } else { -0.06 })) {
                    s2.media.lock().unwrap().vol = Some(v);
                    return;
                }
            }
            if let Some(k) = media::key_for(&cmd) { media::press_key(k, 2); }
            return;
        }
        if let Some(k) = media::key_for(&cmd) {
            media::press_key(k, 1);
            std::thread::sleep(Duration::from_millis(300));
            *s2.media.lock().unwrap() = media::poll(spv);
        }
    });
    j(json!({"ok": true}))
}

async fn quit(State(s): S, ConnectInfo(addr): Addr) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    crate::quit(&s);
    j(json!({"ok": true}))
}

// F11 / Esc / Vollbild-Knopf im Dashboard-Fenster (nur vom PC selbst)
// Rückkehr von der Spotify-Anmeldung (läuft im eingebetteten Fenster)
async fn sp_callback(State(s): S, Query(q): Q) -> Response {
    let (code, state, err) = (q.get("code").cloned().unwrap_or_default(), q.get("state").cloned().unwrap_or_default(), q.get("error").cloned());
    let r = if let Some(e) = err { Err(format!("Anmeldung abgebrochen ({e})")) } else { tokio::task::spawn_blocking(move || crate::spotify::callback(&s, &code, &state)).await.unwrap_or(Err("Fehler".into())) };
    let (title, text) = match &r { Ok(()) => ("Angemeldet", "Du bist mit Spotify verbunden. Das Dashboard schließt dieses Fenster gleich.".to_string()), Err(e) => ("Nicht angemeldet", e.clone()) };
    let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;");
    let page = format!("<!doctype html><meta charset=utf-8><body style=\"margin:0;height:100vh;display:grid;place-items:center;background:#121212;color:#fff;font:600 16px Segoe UI,sans-serif;text-align:center\"><div><div style=\"font-size:44px;margin-bottom:10px\">{}</div><h2 style=\"margin:0 0 8px\">{}</h2><p style=\"color:#b3b3b3;max-width:420px\">{}</p></div>", if r.is_ok() { "✓" } else { "!" }, title, esc(&text));
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
}

async fn sp_api(State(s): S, method: axum::http::Method, Path(path): Path<String>, axum::extract::RawQuery(q): axum::extract::RawQuery, b: Bytes) -> Response {
    let m = method.as_str().to_string();
    let (st, text) = tokio::task::spawn_blocking(move || crate::spotify::api(&s, &m, &path, q.as_deref().unwrap_or(""), &b)).await.unwrap_or((502, String::new()));
    (StatusCode::from_u16(st).unwrap_or(StatusCode::BAD_GATEWAY), [(header::CONTENT_TYPE, "application/json")], text).into_response()
}

// Spotify/YouTube im Programmfenster anzeigen, verstecken, bedienen (nur vom PC selbst)
async fn app_ctl(State(s): S, ConnectInfo(addr): Addr, Query(q): Q) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    let Some(app) = s.app.get().cloned() else { return j(json!({"ok": false})) };
    let id = q.get("id").cloned().unwrap_or_default();
    let r = tokio::task::spawn_blocking(move || crate::apps::steuern(&app, &id, &q)).await.unwrap_or(json!({"ok": false}));
    j(r)
}

async fn window(State(s): S, ConnectInfo(addr): Addr, Query(q): Q) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    let mut fs = false;
    let onoff = |k: &str, cur: bool| match q.get(k).map(String::as_str) { Some("toggle") => !cur, Some("on") => true, Some("off") => false, _ => cur };
    let (nf0, top0) = { let c = s.cfg.lock().unwrap(); (c.kein_fokus, c.immer_oben) };
    let (nf, top) = (onoff("nofocus", nf0), onoff("top", top0));
    if nf != nf0 { s.cfg.lock().unwrap().kein_fokus = nf; s.set_setting("spiel_fokus_behalten", if nf { "ja" } else { "nein" }); }
    if top != top0 { s.cfg.lock().unwrap().immer_oben = top; s.set_setting("immer_im_vordergrund", if top { "ja" } else { "nein" }); }
    if let Some(w) = s.app.get().and_then(|a| a.get_webview_window("main")) {
        let cur = w.is_fullscreen().unwrap_or(false);
        fs = onoff("fs", cur);
        if fs != cur { let _ = w.set_fullscreen(fs); }
        if nf != nf0 || top != top0 { crate::fenster_stil(&w, nf, top); }
    }
    j(json!({"fs": fs, "nofocus": nf, "top": top}))
}

fn clean(v: &str, max: usize) -> String {
    take_chars(v.replace(['\r', '\n', '='], "").trim(), max)
}

async fn friends_config(State(s): S, Query(q): Q) -> Response {
    let mut publish_lr = false;
    {
        let mut c = s.cfg.lock().unwrap();
        if let Some(n) = q.get("name").map(|n| clean(n, 24)).filter(|n| !n.is_empty()) {
            c.fr_name = n.clone();
            s.set_setting("name", &n);
        }
        if let Some(on) = q.get("on").map(|v| v == "1") {
            if on != c.fr_on {
                c.fr_on = on;
                s.set_setting("freunde", if on { "ja" } else { "nein" });
                publish_lr = on;
            }
        }
    }
    if publish_lr { let s2 = s.clone(); tauri::async_runtime::spawn_blocking(move || dienst::pub_roads(&s2, true)); }
    j(json!({"on": s.fr_on(), "name": s.name(), "group": s.lobbys().names()}))
}

async fn friendlr(State(s): S) -> Response {
    let on = s.fr_on();
    let f = s.flr.lock().unwrap();
    let list: Vec<Value> = if on {
        f.list.iter().map(|(id, o)| {
            let mut e = serde_json::Map::new();
            e.insert("id".into(), json!(id));
            if let Some(m) = o.as_object() { for (k, v) in m { e.insert(k.clone(), v.clone()); } }
            Value::Object(e)
        }).collect()
    } else { Vec::new() };
    j(json!({"v": f.v, "list": list}))
}

// nur ein Dashboard (Fenster/Tablet) lernt Straßen gleichzeitig
async fn lease(State(s): S, Query(q): Q) -> Response {
    let Some(id) = q.get("id").filter(|i| !i.is_empty()) else { return j(json!({"learner": false})) };
    let now = now_ms();
    let mut l = s.lease.lock().unwrap();
    if l.0.is_empty() || &l.0 == id || now.saturating_sub(l.1) > 15000 { *l = (id.clone(), now); }
    j(json!({"learner": &l.0 == id}))
}

async fn karte_export(State(s): S) -> Response {
    let s2 = s.clone();
    let (text, name) = tokio::task::spawn_blocking(move || {
        let name = match s2.sitz().cur() { Some(c) => format!("karte-sitzung-{}.json", fmt_code(&c.code())), None => format!("karte-v{}.json", s2.karten.active_version()) };
        (karte::serialize(&dienst::map_ents(&s2)), name)
    }).await.unwrap_or_default();
    ([(header::CONTENT_TYPE, "application/json".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename={name}"))], text).into_response()
}

async fn friendlr_import(State(s): S, ConnectInfo(addr): Addr, b: Bytes) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    let v = body(&b);
    let arr = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let name = v["n"].as_str().unwrap_or("Freund").to_string();
    let (roads, cands, s2) = (arr("roads"), arr("cands"), s.clone());
    let _ = tokio::task::spawn_blocking(move || dienst::flr_import(&s2, &name, roads, cands)).await;
    j(json!({"ok": true}))
}

async fn learned_get(State(s): S) -> Response {
    let data = std::fs::read_to_string(s.file("gelernte-strassen.json")).map(|t| t.trim().to_string()).unwrap_or_default();
    let data = if data.is_empty() { "null".to_string() } else { data };
    json_str(format!(r#"{{"v":{},"data":{}}}"#, s.lver.load(Ordering::Relaxed), data))
}

// Speichern nur, wenn das Dashboard auf dem aktuellen Stand war (sonst würde ein zweites Gerät Änderungen überschreiben)
async fn learned_post(State(s): S, Query(q): Q, body: Bytes) -> Response {
    let lver = s.lver.load(Ordering::Relaxed);
    let base = q.get("base").and_then(|b| b.parse::<u64>().ok());
    let text = String::from_utf8_lossy(&body);
    if base != Some(lver) || !text.trim_start().starts_with('{') || serde_json::from_str::<Value>(&text).is_err() { return j(json!({"ok": false, "v": lver})); }
    let f = s.file("gelernte-strassen.json");
    let ok = (|| -> std::io::Result<()> {
        if f.exists() { std::fs::copy(&f, s.file("gelernte-strassen.json.bak"))?; }
        crate::util::write_atomic(&f, text.as_bytes())?;
        crate::util::write_atomic(&s.file("gelernte-strassen.version"), (lver + 1).to_string().as_bytes())?;
        Ok(())
    })();
    if ok.is_err() { return j(json!({"ok": false, "v": lver})); }
    s.lver.store(lver + 1, Ordering::Relaxed);
    let s2 = s.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        let _ = tokio::task::spawn_blocking(move || dienst::pub_roads(&s2, false)).await;
    });
    j(json!({"ok": true, "v": lver + 1}))
}
