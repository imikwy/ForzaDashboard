// Webserver: Dashboard für dieses Fenster und für Handy/Tablet im WLAN, Live-Daten per Server-Sent Events
use crate::state::Shared;
use crate::util::{lan_ips, now_ms, take_chars};
use crate::{media, net, settings, shared, APP_VER};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;
use tokio_stream::wrappers::BroadcastStream;

type S = State<Arc<Shared>>;
type Q = Query<HashMap<String, String>>;

static HTML: &[u8] = include_bytes!("../../src/dashboard.html");

pub fn router(s: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/events", get(events))
        .route("/info", get(info))
        .route("/media", get(media_get))
        .route("/media/{cmd}", get(media_cmd))
        .route("/marks", get(|| async { json_str("[]".into()) }))
        .route("/quit", get(quit))
        .route("/window", get(window))
        .route("/log", get(|ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(q): Q| async move {
            if addr.ip().is_loopback() { if let Some(m) = q.get("m") { crate::util::log(format!("Fenster: {}", take_chars(m, 500))); } }
            json_str("{}".into())
        }))
        .route("/friends/config", get(friends_config))
        .route("/learned/version", get(|State(s): S| async move { json_str(json!({"v": s.lver.load(Ordering::Relaxed)}).to_string()) }))
        .route("/friendlr/version", get(|State(s): S| async move { json_str(json!({"v": s.flr.lock().unwrap().v}).to_string()) }))
        .route("/friendlr", get(friendlr))
        .route("/learned/lease", get(lease))
        .route("/learned", get(learned_get).post(learned_post))
        .route("/map", get(|State(s): S| async move { json_str(shared::snapshot(&s).to_string()) }).post(map_post))
        .route("/map/presence", axum::routing::post(map_presence))
        .route("/map/import", axum::routing::post(map_import))
        .route("/friendlr/import", axum::routing::post(friendlr_import))
        .fallback(|| async { (StatusCode::NOT_FOUND, "Nicht gefunden") })
        .layer(DefaultBodyLimit::max(64 << 20))
        .with_state(s)
}

fn json_str(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

async fn index(State(s): S) -> Response {
    // eine dashboard.html im Datenordner hat Vorrang (zum Ausprobieren ohne neu zu bauen)
    let body = std::fs::read(s.file("dashboard.html")).map(Bytes::from).unwrap_or_else(|_| Bytes::from_static(HTML));
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

async fn events(State(s): S) -> impl IntoResponse {
    let first = s.latest.lock().unwrap().clone();
    let mut hello = Event::default().retry(Duration::from_millis(1000));
    if let Some(d) = first { hello = hello.data(&*d); }
    let live = BroadcastStream::new(s.tx.subscribe()).filter_map(|r| async move {
        let ev = r.ok()?;
        let mut e = Event::default().data(&*ev.data);
        if let Some(n) = ev.name { e = e.event(n); }
        Some(Ok::<_, Infallible>(e))
    });
    let st: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(stream::once(async { Ok(hello) }).chain(live));
    let sse = Sse::new(st).keep_alive(KeepAlive::new().interval(Duration::from_secs(2)).text("ping"));
    ([(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")], sse)
}

async fn info(State(s): S) -> Response {
    json_str(json!({
        "udpPort": s.udp_port.load(Ordering::Relaxed), "webPort": s.web_port.load(Ordering::Relaxed),
        "packets": s.packets.load(Ordering::Relaxed), "packetLen": s.last_len.load(Ordering::Relaxed),
        "ips": lan_ips(), "markKey": "", "ver": APP_VER,
    }).to_string())
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
    json_str(o.to_string())
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
    json_str(r#"{"ok":true}"#.into())
}

async fn quit(State(s): S, ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    crate::quit(&s);
    json_str(r#"{"ok":true}"#.into())
}

// F11 / Esc / Vollbild-Knopf im Dashboard-Fenster (nur vom PC selbst)
async fn window(State(s): S, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(q): Q) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    let mut fs = false;
    if let Some(w) = s.app.get().and_then(|a| a.get_webview_window("main")) {
        let cur = w.is_fullscreen().unwrap_or(false);
        fs = match q.get("fs").map(String::as_str) { Some("toggle") => !cur, Some("on") => true, Some("off") => false, _ => cur };
        if fs != cur { let _ = w.set_fullscreen(fs); }
    }
    json_str(json!({"fs": fs}).to_string())
}

fn clean(v: &str, max: usize) -> String {
    take_chars(v.replace(['\r', '\n', '='], "").trim(), max)
}

async fn friends_config(State(s): S, Query(q): Q) -> Response {
    let mut restart = false;
    let mut publish_lr = false;
    {
        let mut c = s.cfg.lock().unwrap();
        if let Some(n) = q.get("name").map(|n| clean(n, 24)).filter(|n| !n.is_empty()) {
            c.fr_name = n.clone();
            s.set_setting("name", &n);
        }
        if let Some(g) = q.get("group") {
            let mut g = clean(g, 40);
            if g.is_empty() { g = settings::DEFAULT_GROUP.into(); }
            if g != c.fr_group {
                c.fr_group = g.clone();
                s.set_setting("gruppe", &g);
                restart = true;
            }
        }
        if let Some(on) = q.get("on").map(|v| v == "1") {
            if on != c.fr_on {
                if !on { drop(c); net::bye(&s); c = s.cfg.lock().unwrap(); }
                c.fr_on = on;
                s.set_setting("freunde", if on { "ja" } else { "nein" });
                publish_lr = on;
            }
        }
    }
    if publish_lr { let s2 = s.clone(); tauri::async_runtime::spawn_blocking(move || net::lr_publish(&s2, true)); }
    if restart { net::restart(&s); }
    let c = s.cfg.lock().unwrap();
    json_str(json!({"on": c.fr_on, "name": c.fr_name, "group": c.fr_group}).to_string())
}

async fn friendlr(State(s): S) -> Response {
    let on = s.cfg.lock().unwrap().fr_on;
    let f = s.flr.lock().unwrap();
    let list: Vec<serde_json::Value> = if on {
        f.list.iter().map(|(id, o)| {
            let mut e = serde_json::Map::new();
            e.insert("id".into(), json!(id));
            if let Some(m) = o.as_object() { for (k, v) in m { e.insert(k.clone(), v.clone()); } }
            serde_json::Value::Object(e)
        }).collect()
    } else { Vec::new() };
    json_str(json!({"v": f.v, "list": list}).to_string())
}

// nur ein Dashboard (Fenster/Tablet) lernt Straßen gleichzeitig
async fn lease(State(s): S, Query(q): Q) -> Response {
    let Some(id) = q.get("id").filter(|i| !i.is_empty()) else { return json_str(r#"{"learner":false}"#.into()) };
    let now = now_ms();
    let mut l = s.lease.lock().unwrap();
    if l.0.is_empty() || &l.0 == id || now.saturating_sub(l.1) > 15000 { *l = (id.clone(), now); }
    json_str(json!({"learner": &l.0 == id}).to_string())
}

// Gemeinsame Karte: Änderungen eines Dashboards ({ents:[...]}) – der Server stempelt und verteilt sie
async fn map_post(State(s): S, body: Bytes) -> Response {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else { return (StatusCode::BAD_REQUEST, "").into_response() };
    let ents = v.get("ents").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
    let s2 = s.clone();
    let (rev, ents) = tokio::task::spawn_blocking(move || shared::local_put(&s2, ents)).await.unwrap_or_default();
    json_str(json!({"rev": rev, "ents": ents}).to_string())
}

// Import nur vom PC selbst (Datei aufs Dashboard gezogen)
async fn map_import(State(s): S, ConnectInfo(addr): ConnectInfo<SocketAddr>, body: Bytes) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else { return (StatusCode::BAD_REQUEST, "").into_response() };
    let ents = v.get("ents").and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
    let s2 = s.clone();
    let n = tokio::task::spawn_blocking(move || shared::import(&s2, ents)).await.unwrap_or(0);
    json_str(json!({"n": n}).to_string())
}

async fn friendlr_import(State(s): S, ConnectInfo(addr): ConnectInfo<SocketAddr>, body: Bytes) -> Response {
    if !addr.ip().is_loopback() { return (StatusCode::FORBIDDEN, "").into_response(); }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else { return (StatusCode::BAD_REQUEST, "").into_response() };
    let arr = |k: &str| v.get(k).and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
    let name = v.get("n").and_then(serde_json::Value::as_str).unwrap_or("Freund").to_string();
    let (roads, cands, s2) = (arr("roads"), arr("cands"), s.clone());
    let _ = tokio::task::spawn_blocking(move || net::flr_import(&s2, &name, roads, cands)).await;
    json_str(r#"{"ok":true}"#.into())
}

async fn map_presence(State(s): S, body: Bytes) -> Response {
    if let Ok(serde_json::Value::Object(o)) = serde_json::from_slice::<serde_json::Value>(&body) { shared::presence(&s, o); }
    json_str("{}".into())
}

async fn learned_get(State(s): S) -> Response {
    let data = std::fs::read_to_string(s.file("gelernte-strassen.json")).map(|t| t.trim().to_string()).unwrap_or_default();
    let data = if data.is_empty() { "null".to_string() } else { data };
    json_str(format!(r#"{{"v":{},"data":{}}}"#, s.lver.load(Ordering::Relaxed), data))
}

async fn learned_post(State(s): S, Query(q): Q, body: Bytes) -> Response {
    let lver = s.lver.load(Ordering::Relaxed);
    let base = q.get("base").and_then(|b| b.parse::<u64>().ok());
    let text = String::from_utf8_lossy(&body);
    if base != Some(lver) || !text.trim_start().starts_with('{') { return json_str(json!({"ok": false, "v": lver}).to_string()); }
    let (f, tmp) = (s.file("gelernte-strassen.json"), s.file("gelernte-strassen.json.tmp"));
    let ok = (|| -> std::io::Result<()> {
        if f.exists() { std::fs::copy(&f, s.file("gelernte-strassen.json.bak"))?; }
        std::fs::write(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, &f)?;
        std::fs::write(s.file("gelernte-strassen.version"), (lver + 1).to_string())?;
        Ok(())
    })();
    if ok.is_err() { return json_str(json!({"ok": false, "v": lver}).to_string()); }
    s.lver.store(lver + 1, Ordering::Relaxed);
    json_str(json!({"ok": true, "v": lver + 1}).to_string())
}
