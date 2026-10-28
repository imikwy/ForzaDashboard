// ForzaDashboard – Telemetrie-Dashboard für Forza Horizon 6 als eigenständige Windows-App.
// Empfängt Forza "Data Out" per UDP, zeigt das Dashboard in einem eigenen Fenster und stellt es
// zusätzlich im WLAN für Handy/Tablet bereit. Ein Symbol unten rechts in der Taskleiste hält es am Laufen.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bruecke;
mod dienst;
mod kanal;
mod karte;
mod lobby;
mod media;
mod mqtt;
mod settings;
mod sitzung;
mod state;
mod telemetry;
mod updates;
mod util;
mod veroeffentlichen;
mod web;

use state::{MyPos, Shared};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::{sleep, timeout};
use util::{log, now_ms};

// Format JJJJ-MM-TT.N. BAU_VER zählt ForzaDashboard-bauen.bat hoch; Releases auf GitHub setzen FD_VERSION beim Bauen.
const BAU_VER: &str = "2026-10-28.12";
pub const APP_VER: &str = match option_env!("FD_VERSION") { Some(v) => v, None => BAU_VER };
// hier kommen Karten- und Programm-Updates her (in einstellungen.txt mit update_repo=… änderbar)
pub const REPO: &str = "imikwy/ForzaDashboard";

// F11 / Esc und der Vollbild-Knopf des Dashboards schalten das Fenster selbst in den Vollbildmodus
const FULLSCREEN_JS: &str = r#"(function(){
  if (!/^(localhost|127\.0\.0\.1)$/.test(location.hostname)) return;
  // Fehler im Dashboard-Fenster landen in ForzaDashboard-log.txt
  const _fetch = window.fetch.bind(window);
  const log = m => { try { _fetch("/log?m=" + encodeURIComponent(String(m).slice(0, 500)), {cache:"no-store"}).catch(() => {}); } catch (e) {} };
  addEventListener("error", e => log("JS-Fehler: " + e.message + " @" + e.lineno));
  addEventListener("unhandledrejection", e => log("JS-Fehler: " + (e.reason && e.reason.message || e.reason)));
  window.fetch = (u, o) => _fetch(u, o).catch(err => { log("fetch " + u + ": " + err); throw err; });
  let fs = false;
  const call = a => fetch("/window?fs=" + a, {cache:"no-store"}).then(r => r.json()).then(o => { fs = !!o.fs; }).catch(() => {});
  call("get");
  try {
    Object.defineProperty(Document.prototype, "fullscreenElement", {configurable:true, get(){ return fs ? document.documentElement : null; }});
    Element.prototype.requestFullscreen = function(){ fs = true; return call("on"); };
    Document.prototype.exitFullscreen = function(){ fs = false; return call("off"); };
  } catch (e) {}
  // Esc beendet das Vollbild nur, wenn das Dashboard die Taste nicht selbst benutzt (z. B. im Karten-Editor)
  addEventListener("keydown", e => {
    if (e.key === "F11") { e.preventDefault(); call("toggle"); }
    else if (e.key === "Escape" && fs && !e.defaultPrevented) call("off");
  });
})();"#;

fn state_flags() -> StateFlags {
    StateFlags::all() & !StateFlags::VISIBLE
}

fn main() {
    if std::env::args().any(|a| a == "--selbsttest") { std::process::exit(selbsttest()); }
    if let Some(pid) = std::env::var("FD_WAIT_PID").ok().and_then(|p| p.parse().ok()) { util::wait_pid(pid); }
    let root = util::find_root();
    util::log_init(&root);
    log(format!("ForzaDashboard {APP_VER} startet, Datenordner {}", root.display()));
    // frisch installiertes Update? Dann aufpassen, ob es sauber läuft (sonst automatisch zurück)
    updates::startup_probe();
    // nur für den Test "automatisch zurück" (wird nie in Releases gesetzt)
    if option_env!("FD_TEST_ABSTURZ").is_some() { panic!("Test-Absturz"); }
    let shared = Arc::new(Shared::new(root));
    log(format!("Gerät {} · Karte Version {}", shared.sid, shared.karten.active_version()));
    dienst::setup(&shared);
    let restarted = std::env::var("FD_RESTARTED").is_ok();
    let want_window = if restarted { std::env::var("FD_WINDOW").as_deref() == Ok("1") } else { shared.cfg.lock().unwrap().open_on_start };

    let s_single = shared.clone();
    let s_setup = shared.clone();
    let mut builder = tauri::Builder::default();
    // zum Testen mehrerer "PCs" auf einem Rechner (eigener Datenordner per FD_ROOT): kein Einzel-Start-Schutz
    if std::env::var("FD_ZWEITE_INSTANZ").is_err() {
        builder = builder.plugin(tauri_plugin_single_instance::init(move |app, _args, _cwd| open_window(app, &s_single)));
    }
    let app = builder
        .plugin(tauri_plugin_window_state::Builder::default().with_state_flags(state_flags()).build())
        .setup(move |app| {
            let handle = app.handle().clone();
            let _ = s_setup.app.set(handle.clone());
            if let Err(e) = tauri::async_runtime::block_on(start(s_setup.clone())) {
                log(&e);
                if updates::rollback("Start fehlgeschlagen") { std::process::exit(0); }
                util::msgbox(&e);
                std::process::exit(1);
            }
            build_tray(&handle, &s_setup)?;
            if want_window { open_window(&handle, &s_setup); }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("ForzaDashboard konnte nicht gestartet werden");

    app.run(|_app, ev| {
        // nicht schon beim Schließen des Fensters hart beenden: quit() meldet sich erst bei den Freunden ab,
        // speichert die gemeinsame Karte und beendet dann selbst (oder die App läuft ohne Fenster: beim_start_oeffnen=nein)
        if let RunEvent::ExitRequested { code: None, api, .. } = ev { api.prevent_exit(); }
    });
}

// Wird vor jedem Programm-Release auf GitHub ausgeführt: startet die exe überhaupt, ist die eingebaute Karte gültig?
fn selbsttest() -> i32 {
    let dir = std::env::temp_dir().join(format!("fd-selbsttest-{}", util::random_hex(4)));
    let k = karte::Karten::new(&dir);
    let ok = k.active_version() > 0 && k.info()["basisOk"] == true && web::html_ok();
    let _ = std::fs::remove_dir_all(&dir);
    println!("ForzaDashboard {APP_VER}: Selbsttest {}", if ok { "OK" } else { "FEHLGESCHLAGEN" });
    if ok { 0 } else { 1 }
}

// ---------------- Start: alte Version ablösen, Ports öffnen ----------------
async fn http_get(port: u16, path: &str) -> Option<String> {
    let mut c = timeout(Duration::from_millis(800), TcpStream::connect(("127.0.0.1", port))).await.ok()?.ok()?;
    c.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes()).await.ok()?;
    let mut out = Vec::new();
    timeout(Duration::from_millis(1500), c.read_to_end(&mut out)).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out).to_string();
    text.split_once("\r\n\r\n").map(|(_, b)| b.to_string())
}

async fn old_running(port: u16) -> bool {
    http_get(port, "/info").await.and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok()).is_some_and(|j| j["udpPort"].is_number())
}

// Läuft noch die Node-Version (oder eine alte exe)? Höflich beenden, notfalls den Prozess auf dem Port.
async fn take_over_old(port: u16) {
    if !old_running(port).await { return; }
    log("Ältere ForzaDashboard-Version läuft noch – wird beendet");
    let _ = http_get(port, "/quit").await;
    sleep(Duration::from_millis(1500)).await;
    if !old_running(port).await { return; }
    let out = util::no_window(std::process::Command::new("netstat").args(["-ano", "-p", "tcp"])).output();
    let me = std::process::id().to_string();
    if let Ok(out) = out {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let c: Vec<&str> = line.split_whitespace().collect();
            if c.len() >= 5 && c[1].ends_with(&format!(":{port}")) && c[2].ends_with(":0") && c[4] != me && c[4] != "0" {
                let _ = util::no_window(std::process::Command::new("taskkill").args(["/f", "/pid", c[4]])).status();
            }
        }
    }
    sleep(Duration::from_millis(1500)).await;
}

async fn start(s: Arc<Shared>) -> Result<(), String> {
    let (wanted_udp, mut web_port) = { let c = s.cfg.lock().unwrap(); (c.udp_port, c.web_port) };
    take_over_old(web_port).await;

    // UDP-Port öffnen; ist er belegt, automatisch den nächsten freien nehmen (5200–5300 nutzt das Spiel selbst)
    let mut port = wanted_udp;
    let mut udp = None;
    for _ in 0..=40 {
        match UdpSocket::bind(("0.0.0.0", port)).await {
            Ok(u) => { udp = Some(u); break; }
            Err(_) => { port = port.wrapping_add(1); if (5200..=5300).contains(&port) { port = 5301; } }
        }
    }
    let udp = udp.ok_or_else(|| format!(
        "Es konnte kein UDP-Port geöffnet werden.\n\nBitte in einstellungen.txt bei udp_port eine andere Zahl eintragen (z. B. 9999) und denselben Port im Spiel unter Data Out.\n\nOrdner: {}", s.root.display()))?;
    s.udp_port.store(port, Ordering::Relaxed);
    if port != wanted_udp {
        s.port_changed.store(true, Ordering::Relaxed);
        s.set_setting("udp_port", &port.to_string());
        log(format!("UDP-Port {wanted_udp} war belegt – benutze {port}"));
    }

    let mut listener = None;
    for _ in 0..=10 {
        match TcpListener::bind(("0.0.0.0", web_port)).await {
            Ok(l) => { listener = Some(l); break; }
            Err(_) => web_port += 1,
        }
    }
    let listener = listener.ok_or("Kein freier Port für das Dashboard gefunden (web_port in einstellungen.txt).")?;
    s.web_port.store(web_port, Ordering::Relaxed);
    // Webserver und Empfang sofort starten, alles andere danach
    let app = web::router(s.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tauri::async_runtime::spawn(async move { let _ = axum::serve(listener, app).await; });
    tauri::async_runtime::spawn(udp_loop(s.clone(), udp));
    log(format!("Im Spiel: 127.0.0.1 Port {port} · Dashboard: http://localhost:{web_port}"));

    dienst::start(&s);
    updates::spawn(s.clone());
    let sm = s.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            sleep(Duration::from_secs(3)).await;
            if now_ms().saturating_sub(sm.media_wanted.load(Ordering::Relaxed)) < 10000 { web::media_refresh(&sm); }
        }
    });
    Ok(())
}

async fn udp_loop(s: Arc<Shared>, sock: UdpSocket) {
    let mut buf = vec![0u8; 2048];
    loop {
        let Ok((n, _)) = sock.recv_from(&mut buf).await else { continue };
        if n < 232 { continue; }
        let p = telemetry::parse(&buf[..n]);
        let data: Arc<str> = p.json.to_string().into();
        *s.latest.lock().unwrap() = Some(data.clone());
        s.packets.fetch_add(1, Ordering::Relaxed);
        s.last_packet.store(now_ms(), Ordering::Relaxed);
        s.last_len.store(n, Ordering::Relaxed);
        // sofort an die Dashboards schicken (kein Takt-Raster -> kein Ruckeln)
        let _ = s.tx.send(state::Ev { name: None, data });
        if p.on && p.dash && (p.px != 0.0 || p.pz != 0.0) {
            *s.my_pos.lock().unwrap() = Some(MyPos { x: p.px, z: p.pz, h: p.yaw, v: p.spd, c: p.car, t: now_ms() });
        }
    }
}

// ---------------- Fenster ----------------
fn webview_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    base.join("ForzaDashboard").join("webview")
}

// Einstellungen, Kartenausrichtung usw. aus dem bisherigen Edge/Chrome-Fenster übernehmen (einmalig)
fn migrate_browser_storage(dir: &std::path::Path) {
    let dst = dir.join("EBWebView").join("Default").join("Local Storage");
    let Some(base) = dir.parent() else { return };
    let src = base.join("browser").join("Default").join("Local Storage");
    if dst.exists() || !src.exists() { return; }
    fn copy(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            if e.file_name() == "LOCK" { continue; }
            if e.file_type()?.is_dir() { copy(&e.path(), &dst.join(e.file_name()))?; } else { std::fs::copy(e.path(), dst.join(e.file_name()))?; }
        }
        Ok(())
    }
    match copy(&src, &dst) {
        Ok(()) => log("Dashboard-Einstellungen aus dem bisherigen Browserfenster übernommen"),
        Err(e) => log(format!("Übernahme der Browser-Einstellungen fehlgeschlagen: {e}")),
    }
}

pub fn open_window(app: &AppHandle, s: &Arc<Shared>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let dir = webview_dir();
    migrate_browser_storage(&dir);
    let first_run = !app.path().app_config_dir().map(|d| d.join(".window-state.json").exists()).unwrap_or(false);
    let url = format!("http://localhost:{}/", s.web_port.load(Ordering::Relaxed));
    let built = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse().expect("URL")))
        .title("ForzaDashboard")
        .inner_size(1280.0, 800.0)
        .min_inner_size(480.0, 320.0)
        .data_directory(dir)
        .initialization_script(FULLSCREEN_JS)
        .visible(false)
        .build();
    let w = match built {
        Ok(w) => w,
        Err(e) => { log(format!("Fenster konnte nicht geöffnet werden: {e}")); return; }
    };
    if first_run { let _ = w.center(); let _ = w.set_fullscreen(true); }
    let _ = w.show();
    let _ = w.set_focus();
    // Fenster zu = ForzaDashboard ganz beenden (verschwindet auch aus dem Infobereich)
    let s2 = s.clone();
    w.on_window_event(move |e| {
        if let WindowEvent::CloseRequested { .. } = e { quit(&s2); }
    });
}

// ---------------- Symbol unten rechts ----------------
fn build_tray(app: &AppHandle, s: &Arc<Shared>) -> tauri::Result<()> {
    let udp = s.udp_port.load(Ordering::Relaxed);
    let web = s.web_port.load(Ordering::Relaxed);
    let open = MenuItem::with_id(app, "open", "Dashboard öffnen", true, None::<&str>)?;
    let game = MenuItem::with_id(app, "game", format!("Im Spiel: IP 127.0.0.1 · Port {udp}"), false, None::<&str>)?;
    let ips: Vec<MenuItem<Wry>> = util::lan_ips().iter().enumerate()
        .map(|(i, ip)| MenuItem::with_id(app, format!("ip{i}"), format!("Handy/Tablet: http://{ip}:{web}"), false, None::<&str>))
        .collect::<tauri::Result<_>>()?;
    let folder = MenuItem::with_id(app, "folder", "Ordner mit Einstellungen öffnen", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let (sep1, sep2) = (PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?);
    let mut items: Vec<&dyn IsMenuItem<Wry>> = vec![&open, &sep1, &game];
    for i in &ips { items.push(i); }
    items.extend([&sep2 as &dyn IsMenuItem<Wry>, &folder, &exit]);
    let menu = Menu::with_items(app, &items)?;

    let (s_menu, s_click) = (s.clone(), s.clone());
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("ForzaDashboard")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, ev| match ev.id().as_ref() {
            "open" => open_window(app, &s_menu),
            "folder" => { let _ = std::process::Command::new("explorer").arg(&s_menu.root).spawn(); }
            "quit" => quit(&s_menu),
            _ => {}
        })
        .on_tray_icon_event(move |tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                open_window(tray.app_handle(), &s_click);
            }
        });
    if let Some(icon) = app.default_window_icon() { tray = tray.icon(icon.clone()); }
    tray.build(app)?;

    // Infotext am Symbol: kommen Daten von Forza an?
    let (s2, h) = (s.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            sleep(Duration::from_secs(2)).await;
            let n = s2.packets.load(Ordering::Relaxed);
            let tip = if n == 0 { format!("ForzaDashboard – noch keine Daten von Forza (Port {udp})") } else { format!("ForzaDashboard – {n} Pakete von Forza") };
            if let Some(t) = h.tray_by_id("main") { let _ = t.set_tooltip(Some(tip)); }
        }
    });
    Ok(())
}

// ---------------- Beenden / Neustart ----------------
pub fn quit(s: &Arc<Shared>) {
    // bei den anderen abmelden (Sitzung, Lobbys); das Journal ist ohnehin schon auf der Platte
    if let Some(c) = s.sitz().cur() { c.stop(); }
    s.lobbys().stop();
    let net = s.net.clone();
    std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(300)); net.stop(); });
    let Some(app) = s.app.get().cloned() else { std::process::exit(0) };
    let _ = app.save_window_state(state_flags());
    tauri::async_runtime::spawn(async move {
        sleep(Duration::from_millis(700)).await;
        app.exit(0);
    });
}

// nach einem Update: neue exe starten (sie wartet, bis diese beendet ist), dann beenden
// false = die neue exe ließ sich gar nicht starten (dann läuft diese Version weiter)
pub fn restart_self(s: &Arc<Shared>) -> bool {
    let window_open = s.app.get().and_then(|a| a.get_webview_window("main")).is_some();
    let started = std::env::current_exe().is_ok_and(|exe| std::process::Command::new(exe)
        .env("FD_WAIT_PID", std::process::id().to_string())
        .env("FD_RESTARTED", "1")
        .env("FD_WINDOW", if window_open { "1" } else { "0" })
        .spawn()
        .is_ok());
    if started { quit(s); }
    started
}
