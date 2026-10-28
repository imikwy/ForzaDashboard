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

pub fn html_ok() -> bool {
    HTML.contains("const APP_VER") && HTML.contains("fh6-karte.js") && VORLAGEN.len() > 100_000 && FH6_KARTE.len() > 100_000
}

pub fn router(s: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/fh6-vorlagen.js", get(|h: HeaderMap| async move { asset(&h, VORLAGEN, 0) }))
        .route("/fh6-karte.js", get(|h: HeaderMap| async move { asset(&h, FH6_KARTE, 1) }))
        .route("/events", get(events))
        .route("/info", get(info))
        .route("/media", get(media_get))
        .route("/media/{cmd}", get(media_cmd))
        .route("/marks", get(|| async { json_str("[]".into()) }))
        .route("/quit", get(quit))
        .route("/window", get(window))
        .route("/log", get(|ConnectInfo(addr): Addr, Query(q): Q| async move {
            if addr.ip().is_loopback() { if let Some(m) = q.get("m") { crate::util::log(format!("Fenster: {}", take_chars(m, 500))); } }
            json_str("{}".into())
        }))
        // Freunde und Lobbys
        .route("/friends/config", get(friends_config))
        .route("/lobby", get(|State(s): S| async move { j(json!({"liste": s.lobbys().list(), "name": s.name(), "on": s.fr_on(), "ok": s.net_ok()})) }))
        .route("/lobby/neu", post(|State(s): S, b: Bytes| async move { let b = body(&b); let r = s.lobbys().create(b["name"].as_str().unwrap_or(""), &s.name()); lobby_resp(&s, r) }))
        .route("/lobby/beitreten", post(|State(s): S, b: Bytes| async move { let b = body(&b); let r = s.lobbys().join(b["code"].as_str().unwrap_or("")); lobby_resp(&s, r) }))
        .route("/lobby/verlassen", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.lobbys().leave(b["code"].as_str().unwrap_or("")); lobby_resp(&s, json!({"ok": ok})) }))
        .route("/lobby/an", post(|State(s): S, b: Bytes| async move { let b = body(&b); let ok = s.lobbys().set_on(b["code"].as_str().unwrap_or(""), b["an"].as_bool().unwrap_or(false)); lobby_resp(&s, json!({"ok": ok})) }))
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
        .route("/sitzung/neu", post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| merge(s.sitz().create(b["title"].as_str().unwrap_or("")), dienst::sess_info(&s))).await }))
        .route("/sitzung/beitreten", post(|State(s): S, b: Bytes| async move { let b = body(&b); blocking(s, move |s| merge(s.sitz().join(b["code"].as_str().unwrap_or("")), dienst::sess_info(&s))).await }))
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
            if b["vorschau"].as_bool().unwrap_or(false) { return veroeffentlichen::preview(&c, &s.karten, &s.repo_root()); }
            let r = veroeffentlichen::publish(&c, &s.repo_root(), b["notiz"].as_str().unwrap_or(""), &s.name(), b["massenloeschung"].as_bool().unwrap_or(false));
            if r["ok"] == true { c.send(json!({"y": "pub", "n": s.name()})); }
            r
        }).await }))
        // Updates
        .route("/updates", get(|State(s): S| async move { j(updates::state(&s)) }))
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
    static GZ: [OnceLock<Vec<u8>>; 2] = [OnceLock::new(), OnceLock::new()];
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
async fn window(State(s): S, ConnectInfo(addr): Addr, Query(q): Q) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    let mut fs = false;
    if let Some(w) = s.app.get().and_then(|a| a.get_webview_window("main")) {
        let cur = w.is_fullscreen().unwrap_or(false);
        fs = match q.get("fs").map(String::as_str) { Some("toggle") => !cur, Some("on") => true, Some("off") => false, _ => cur };
        if fs != cur { let _ = w.set_fullscreen(fs); }
    }
    j(json!({"fs": fs}))
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
