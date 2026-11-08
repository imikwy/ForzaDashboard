// In-Game-Overlay: frei anordenbare Glas-Widgets über Forza (Minikarte, Abbiegehinweis, Musik, Tempo, Stapel …).
// Die Anordnung stellt man in den Einstellungen auf einem Beispielbild ein (nicht live im Spiel) – gespeichert in overlay.json.
// - Eigenes randloses, durchsichtiges Fenster, immer im Vordergrund. Mausklicks gehen hindurch ins Spiel,
//   es nimmt nie den Fokus (WS_EX_NOACTIVATE) – das Spiel merkt davon nichts.
// - Sichtbar nur, wenn Forza im Vollbildfenster (randlos) läuft; exklusives Vollbild verdeckt es.
// - Die Minikarte zeichnet das Dashboard (mit den eigenen Straßen) und legt sie hier ab; das Overlay holt sie sich.
use crate::state::Shared;
use crate::util::log;
use serde_json::Value;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

static LAST: Mutex<Option<Value>> = Mutex::new(None);
static MAP: Mutex<Vec<u8>> = Mutex::new(Vec::new());
const LABEL: &str = "overlay";

pub fn is_on(app: &AppHandle) -> bool { app.get_webview_window(LABEL).is_some() }

#[cfg(windows)]
fn no_activate(w: &tauri::WebviewWindow) {
    if let Ok(h) = w.hwnd() {
        unsafe {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
            let hwnd = HWND(h.0);
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize);
        }
    }
}
#[cfg(not(windows))]
fn no_activate(_w: &tauri::WebviewWindow) {}

// an/aus – Rückgabe: jetzt an?
pub fn toggle(app: &AppHandle, s: &Shared) -> bool {
    if let Some(w) = app.get_webview_window(LABEL) { let _ = w.close(); log("Overlay aus"); return false; }
    let url = format!("http://localhost:{}/overlay", s.web_port.load(Ordering::Relaxed));
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(url.parse().expect("URL")))
        .title("ForzaDashboard Overlay")
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false)
        .resizable(false)
        .visible(false)
        .data_directory(crate::webview_dir())
        .build();
    let w = match built { Ok(w) => w, Err(e) => { log(format!("Overlay konnte nicht geöffnet werden: {e}")); return false; } };
    // ganzen Hauptbildschirm abdecken – die Widgets liegen darin, wo man sie hingestellt hat
    if let Ok(Some(m)) = w.primary_monitor() { let _ = w.set_position(*m.position()); let _ = w.set_size(*m.size()); }
    let _ = w.set_ignore_cursor_events(true);
    no_activate(&w);
    let _ = w.show();
    log("Overlay an");
    true
}

// Navigation/Musik-Zusammenfassung vom Dashboard an das Overlay weiterreichen
pub fn push(s: &Shared, v: Value) {
    let txt = v.to_string();
    if txt.len() > 20_000 { return; }
    *LAST.lock().unwrap() = Some(v);
    s.emit(Some("ov"), txt);
}
pub fn last() -> Value { LAST.lock().unwrap().clone().unwrap_or(Value::Null) }

// Minikarten-Bild (WebP/PNG, höchstens 2 MB)
pub fn set_map(b: &[u8]) -> bool {
    if b.is_empty() || b.len() > 2 << 20 { return false; }
    *MAP.lock().unwrap() = b.to_vec();
    true
}
pub fn map() -> Vec<u8> { MAP.lock().unwrap().clone() }

// Anordnung der Widgets (aus den Einstellungen) – geprüft und begrenzt gespeichert, Overlay bekommt sie sofort
pub fn layout(s: &Shared) -> Value { crate::util::read_json(&s.file("overlay.json")).unwrap_or(Value::Null) }
pub fn set_layout(s: &Shared, v: Value) -> bool {
    if !v["w"].is_array() || v.to_string().len() > 20_000 { return false; }
    crate::util::write_json(&s.file("overlay.json"), &v);
    s.emit(Some("ovlayout"), v.to_string());
    true
}
