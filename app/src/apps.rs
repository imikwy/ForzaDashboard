// Spotify und YouTube als eingebettete Webseiten im Dashboard-Fenster (nur in der exe, nicht auf Handy/Tablet).
// Das Dashboard meldet, wo der Platz für die App ist (CSS-Pixel = logische Pixel); hier wird die Seite genau dort
// angezeigt, versteckt oder bedient. Anmeldungen bleiben im Profil des Dashboard-Fensters gespeichert.
use crate::util::log;
use serde_json::{json, Value};
use std::collections::HashMap;
use tauri::webview::WebviewBuilder;
use tauri::{Manager, PhysicalPosition, PhysicalSize, WebviewUrl};
#[cfg(not(windows))]
use tauri::Rect;

fn start_url(id: &str) -> Option<&'static str> {
    match id {
        "spotify" => Some("https://open.spotify.com/"),
        "youtube" => Some("https://www.youtube.com/"),
        "discord" => Some("https://discord.com/app"),
        _ => None,
    }
}

// nur Seiten des jeweiligen Dienstes öffnen (z. B. eine YouTube-Suche aus dem Dashboard)
fn allowed(id: &str, url: &str) -> bool {
    let host = url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("");
    match id {
        "spotify" => host == "open.spotify.com" || host == "accounts.spotify.com",
        "youtube" => host == "www.youtube.com" || host == "m.youtube.com" || host == "youtube.com",
        "discord" => host == "discord.com" || host == "www.discord.com" || host == "ptb.discord.com" || host == "canary.discord.com",
        _ => false,
    }
}

// Lage und runde Ecken direkt über Windows setzen, in echten Bildschirmpixeln relativ zum Dashboard-Fenster.
// (Tauris set_bounds rechnet bei eingebetteten Seiten die Bildschirmskalierung doppelt ein – bei 125 % Skalierung war
// YouTube dann um diesen Faktor zu klein und verschoben.) Läuft im Hauptthread.
#[cfg(windows)]
fn platzieren(wv: &tauri::Webview, x: i32, y: i32, w: i32, h: i32, r: i32, f: f64) {
    if w <= 0 || h <= 0 { return; }
    let _ = wv.with_webview(move |pw| unsafe {
        use windows::Win32::Foundation::{HWND, POINT, RECT};
        use windows::Win32::Graphics::Gdi::{ClientToScreen, CreateRoundRectRgn, SetWindowRgn};
        use windows::Win32::UI::HiDpi::GetDpiForWindow;
        use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetParent, GetWindowLongPtrW, GetWindowRect, SetWindowPos, GWL_STYLE, SWP_NOACTIVATE, SWP_NOZORDER, WS_CHILD};
        let c = pw.controller();
        let mut hp = Default::default();
        if c.ParentWindow(&mut hp).is_err() { return; }
        let hwnd = HWND(hp.0);
        let mut b = Default::default();
        if c.Bounds(&mut b).is_err() { return; }
        if (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32) & WS_CHILD.0 == 0 {
            b.left = x; b.top = y; b.right = x + w; b.bottom = y + h;
            let _ = c.SetBounds(b);
            return;
        }
        let parent = GetParent(hwnd).unwrap_or_default();
        // tatsächliche Lage in Bildschirmpixeln relativ zur Innenfläche des Dashboard-Fensters
        let ist = || {
            let mut rw = RECT::default(); let _ = GetWindowRect(hwnd, &mut rw);
            let mut o = POINT::default(); let _ = ClientToScreen(parent, &mut o);
            (rw.left - o.x, rw.top - o.y, rw.right - rw.left, rw.bottom - rw.top)
        };
        let setzen = |x: i32, y: i32, w: i32, h: i32| { let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE); };
        setzen(x, y, w, h);
        let (ix, iy, iw, ih) = ist();
        // Windows rechnet bei diesem Fenster manchmal die Bildschirmskalierung ein: dann passend gegenrechnen
        let (kx, ky) = (w as f64 / iw.max(1) as f64, h as f64 / ih.max(1) as f64);
        if (kx - 1.0).abs() > 0.02 || (ky - 1.0).abs() > 0.02 || (ix - x).abs() > 3 || (iy - y).abs() > 3 {
            let k = (kx + ky) / 2.0;
            setzen((x as f64 * k).round() as i32, (y as f64 * k).round() as i32, (w as f64 * k).round() as i32, (h as f64 * k).round() as i32);
            let n = ist();
            static LETZTE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let jetzt = crate::util::now_ms();
            if jetzt.saturating_sub(LETZTE.load(std::sync::atomic::Ordering::Relaxed)) > 60_000 { LETZTE.store(jetzt, std::sync::atomic::Ordering::Relaxed); log(format!("Eingebettete Seite (selbst korrigiert): gewünscht {x}/{y} {w}×{h}, war {ix}/{iy} {iw}×{ih}, korrigiert (×{k:.3}) -> {}/{} {}×{} · Seitenfaktor {f:.3} · DPI {} / Fenster {}",
                n.0, n.1, n.2, n.3, GetDpiForWindow(hwnd), GetDpiForWindow(parent))); }
        }
        // Seite und runde Ecken nach der echten Innenfläche des Behälters ausrichten
        let mut rc = RECT::default(); let _ = GetClientRect(hwnd, &mut rc);
        let (cw, ch) = if rc.right > 0 && rc.bottom > 0 { (rc.right, rc.bottom) } else { (w, h) };
        let rr = (r as f64 * cw as f64 / w as f64).round() as i32;
        b.left = 0; b.top = 0; b.right = cw; b.bottom = ch;
        let _ = c.SetBounds(b);
        // das System übernimmt die Region (nicht selbst freigeben)
        SetWindowRgn(hwnd, Some(CreateRoundRectRgn(0, 0, cw + 1, ch + 1, 2 * rr, 2 * rr)), true);
    });
}
#[cfg(not(windows))]
fn platzieren(wv: &tauri::Webview, x: i32, y: i32, w: i32, h: i32, _r: i32, _f: f64) {
    let _ = wv.set_bounds(Rect { position: PhysicalPosition::new(x, y).into(), size: PhysicalSize::new(w, h).into() });
}

// CSS-Pixel des Dashboards -> echte Pixel im Fenster. Gemessen an der echten Fensterbreite statt am Skalierungsfaktor
// (der passt bei mehreren Bildschirmen mit unterschiedlicher Skalierung nicht immer zur Seite).
fn faktor(app: &tauri::AppHandle, vw: Option<f64>) -> f64 {
    let Some(main) = app.get_webview_window("main") else { return 1.0 };
    let sf = main.scale_factor().unwrap_or(1.0);
    match (vw, main.inner_size()) {
        (Some(vw), Ok(sz)) if vw > 100.0 && sz.width > 100 => sz.width as f64 / vw,
        _ => sf,
    }
}

pub fn steuern(app: &tauri::AppHandle, id: &str, q: &HashMap<String, String>) -> Value {
    let Some(start) = start_url(id) else { return json!({"ok": false}) };
    let label = format!("app-{id}");
    let num = |k: &str| q.get(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    let bounds = match (num("x"), num("y"), num("w"), num("h")) {
        (Some(x), Some(y), Some(w), Some(h)) if w >= 40.0 && h >= 40.0 => Some((x.max(0.0), y.max(0.0), w, h)),
        _ => None,
    };
    // Umrechnung CSS-Pixel -> echte Pixel: am zuverlässigsten liefert sie die Seite selbst (devicePixelRatio)
    let f = num("dpr").filter(|d| *d > 0.3 && *d < 8.0).unwrap_or_else(|| faktor(app, num("vw")));
    let px = |v: f64| (v * f).round() as i32;
    let rr = px(num("r").unwrap_or(0.0));
    if let Some(wv) = app.get_webview(&label) {
        if q.contains_key("hide") { let _ = wv.hide(); return json!({"ok": true}); }
        if q.contains_key("back") { let _ = wv.eval("history.back()"); }
        if q.contains_key("home") { if let Ok(u) = start.parse() { let _ = wv.navigate(u); } }
        if let Some(u) = q.get("nav").filter(|u| allowed(id, u)) { if let Ok(u) = u.parse() { let _ = wv.navigate(u); } }
        if let Some((x, y, w, h)) = bounds {
            let _ = wv.show();
            platzieren(&wv, px(x), px(y), px(w), px(h), rr, f);
        }
        return json!({"ok": true, "open": true});
    }
    if q.contains_key("hide") { return json!({"ok": true}); }
    let Some((x, y, w, h)) = bounds else { return json!({"ok": false, "why": "kein Platz angegeben"}) };
    let Some(main) = app.get_webview_window("main") else { return json!({"ok": false, "why": "kein Dashboard-Fenster"}) };
    let window = main.as_ref().window();
    let url = q.get("nav").filter(|u| allowed(id, u)).map(String::as_str).unwrap_or(start);
    let Ok(parsed) = url.parse() else { return json!({"ok": false}) };
    // gleicher Datenordner wie das Dashboard-Fenster (WebView2 verlangt das für Seiten im selben Fenster)
    let b = WebviewBuilder::new(&label, WebviewUrl::External(parsed)).data_directory(crate::webview_dir());
    match window.add_child(b, PhysicalPosition::new(px(x), px(y)), PhysicalSize::new(px(w), px(h))) {
        Ok(wv) => { platzieren(&wv, px(x), px(y), px(w), px(h), rr, f); log(format!("{id}: geöffnet bei {}/{} {}×{} (Faktor {f:.3})", px(x), px(y), px(w), px(h))); json!({"ok": true, "open": true}) }
        Err(e) => { log(format!("{id} konnte nicht geöffnet werden: {e}")); json!({"ok": false, "why": e.to_string()}) }
    }
}
