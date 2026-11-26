// Eigene Automatik: Forza steht auf "Manuell", das Programm schaltet für dich.
// Schaltwege (bester zuerst):
//  - vJoy: virtueller Joystick, in Forza im SELBEN eigenen Lenkrad-Profil wie das echte Lenkrad belegt
//    (Knopf 1 = hoch, 2 = runter, 3 = Handbremse) – Forza wechselt dabei kein Eingabegerät, kein Ruck am Lenkrad.
//  - Controller: unsichtbarer Xbox-Controller (ViGEmBus, Standard B = hoch, X = runter) – Forza springt kurz um.
//  - Tastatur (E/Q): Notlösung, Forza springt aufs Keyboard um.
// Fahrstufen wie im Automatik-Wagen: P (Handbremse, macht das Dashboard), R, D, E (Eco), S (S+), M (selbst schalten).
// Schaltpunkte wie im echten Auto: wenig Gas = früh hoch, Vollgas = spät hoch, Kickdown, beim Bremsen sanft zurück.
// Die Übersetzungen lernt es pro Auto aus Drehzahl/Tempo, damit nach dem Schalten die Drehzahl passt und nichts
// hin- und herpendelt. Schaltet man selbst (Wippen/Tasten), geht es von allein auf M.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering::Relaxed};
use std::sync::Mutex;
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, VIRTUAL_KEY};

struct Pend { dir: i8, t: u64, from: u8 }

struct St {
    an: bool,
    mode: char,
    up: u16,
    dn: u16,
    car: i64,
    ratio: HashMap<(i64, u8), f64>,
    top: HashMap<i64, u8>,
    peak: HashMap<i64, (f64, f64)>,
    gear: u8,
    seen: u64,
    pend: Option<Pend>,
    last_up: u64,
    last_dn: u64,
    ok_shifts: u32,
    miss: u32,
    hint: String,
    hint_t: u64,
    spd: f64,
    thr0: f64,
    kick_t: u64,
    prev: char,
    via: char,      // 'v' = vJoy, 'p' = Controller, 'k' = Tastatur
    pup: u16,       // Controller-Knopf hoch
    pdn: u16,       // Controller-Knopf runter
    wip_direct: Option<bool>,           // Wippe schaltet in Forza selbst (true) oder nur über vJoy (false); None = noch unbekannt
    wip_probe: Option<(u64, i8, u8)>,   // Wippe gerade gedrückt: (Zeit, Richtung, Gang vorher) – wartet, ob Forza selbst schaltet
}

static ST: Mutex<Option<St>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut St) -> R) -> R {
    let mut g = ST.lock().unwrap();
    let s = g.get_or_insert_with(|| St {
        an: false, mode: 'D', up: 0x12, dn: 0x10, car: 0, ratio: HashMap::new(), top: HashMap::new(), peak: HashMap::new(),
        gear: 255, seen: 0, pend: None, last_up: 0, last_dn: 0, ok_shifts: 0, miss: 0, hint: String::new(), hint_t: 0, spd: 0.0, thr0: 0.0, kick_t: 0, prev: 'D',
        via: 'p', pup: 0x2000, pdn: 0x4000, wip_direct: None, wip_probe: None,
    });
    f(s)
}

// nur schalten, wenn das Spiel vorne ist – nicht ins Dashboard, Discord oder den Browser
fn spiel_vorne() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    unsafe {
        let h = GetForegroundWindow();
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(h, &mut buf);
        let t = String::from_utf16_lossy(&buf[..n.max(0) as usize]).to_lowercase();
        t.contains("forza") && !t.contains("dashboard")
    }
}

// ---- Tastatur (Notlösung) ----
fn key(scan: u16, down: bool) {
    let flags = if down { KEYEVENTF_SCANCODE } else { KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP };
    let inp = [INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }];
    unsafe { SendInput(&inp, std::mem::size_of::<INPUT>() as i32); }
}

// ---- vJoy (virtueller DirectInput-Joystick, Gerät 1) ----
// vJoyInterface.dll wird zur Laufzeit geladen – ohne installiertes vJoy läuft alles andere normal weiter.
struct Vj {
    enabled: unsafe extern "C" fn() -> i32,
    status: unsafe extern "C" fn(u32) -> i32,
    acquire: unsafe extern "C" fn(u32) -> i32,
    relinquish: unsafe extern "C" fn(u32),
    reset: unsafe extern "C" fn(u32) -> i32,
    set_btn: unsafe extern "C" fn(i32, u32, u8) -> i32,
}
static VJ: std::sync::OnceLock<Vj> = std::sync::OnceLock::new();
static VJ_ON: AtomicBool = AtomicBool::new(false);
static DEV_ERR: Mutex<String> = Mutex::new(String::new());
fn vj_lib() -> Option<&'static Vj> {
    // nicht dauerhaft „fehlt“ merken: wird vJoy erst nach dem Start installiert, klappt es beim nächsten Versuch
    if let Some(v) = VJ.get() { return Some(v); }
    let v = (|| -> Option<Vj> { unsafe {
        use windows::core::{s, w};
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        for p in [w!("vJoyInterface.dll"), w!("C:\\Program Files\\vJoy\\x64\\vJoyInterface.dll")] {
            let Ok(h) = LoadLibraryW(p) else { continue };
            let (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)) = (
                GetProcAddress(h, s!("vJoyEnabled")), GetProcAddress(h, s!("GetVJDStatus")), GetProcAddress(h, s!("AcquireVJD")),
                GetProcAddress(h, s!("RelinquishVJD")), GetProcAddress(h, s!("ResetVJD")), GetProcAddress(h, s!("SetBtn")),
            ) else { continue };
            return Some(Vj {
                enabled: std::mem::transmute(a), status: std::mem::transmute(b), acquire: std::mem::transmute(c),
                relinquish: std::mem::transmute(d), reset: std::mem::transmute(e), set_btn: std::mem::transmute(f),
            });
        }
        None
    } })();
    let v = v?;
    let _ = VJ.set(v);
    VJ.get()
}
fn dev_err(t: &str) { *DEV_ERR.lock().unwrap() = t.to_string(); }
fn vj_on() -> bool {
    if VJ_ON.load(Relaxed) { return true; }
    let Some(v) = vj_lib() else { dev_err("vJoy ist nicht installiert"); return false };
    unsafe {
        if (v.enabled)() == 0 { dev_err("vJoy ist installiert, aber ausgeschaltet (vJoy Config: Enable vJoy)"); return false; }
        match (v.status)(1) {
            0 => {}
            1 => { if (v.acquire)(1) == 0 { dev_err("vJoy-Gerät 1 ließ sich nicht übernehmen"); return false; } }
            2 => { dev_err("vJoy-Gerät 1 wird von einem anderen Programm benutzt"); return false; }
            _ => { dev_err("vJoy-Gerät 1 fehlt (in „Configure vJoy“ Gerät 1 mit mindestens 3 Knöpfen anlegen)"); return false; }
        }
        (v.reset)(1);
    }
    VJ_ON.store(true, Relaxed);
    DEV_ERR.lock().unwrap().clear();
    crate::util::log("Schaltung: vJoy-Gerät 1 übernommen");
    true
}
fn vj_off() {
    if VJ_ON.swap(false, Relaxed) { if let Some(v) = vj_lib() { unsafe { (v.reset)(1); (v.relinquish)(1); } } crate::util::log("Schaltung: vJoy-Gerät 1 freigegeben"); }
}
fn vj_btn(n: u8, down: bool) { if vj_on() { if let Some(v) = vj_lib() { unsafe { (v.set_btn)(down as i32, 1, n); } } } }

// ---- unsichtbarer Xbox-Controller (ViGEmBus) ----
static PAD: Mutex<Option<vigem_client::Xbox360Wired<vigem_client::Client>>> = Mutex::new(None);
fn pad_on() -> bool {
    let mut g = PAD.lock().unwrap();
    if g.is_some() { return true; }
    let r = (|| -> Result<_, vigem_client::Error> {
        let c = vigem_client::Client::connect()?;
        let mut t = vigem_client::Xbox360Wired::new(c, vigem_client::TargetId::XBOX360_WIRED);
        t.plugin()?;
        t.wait_ready()?;
        t.update(&vigem_client::XGamepad::default())?;
        Ok(t)
    })();
    match r {
        Ok(t) => { *g = Some(t); DEV_ERR.lock().unwrap().clear(); crate::util::log("Schaltung: virtueller Controller angesteckt"); true }
        Err(e) => { dev_err("Virtueller Controller geht nicht – ViGEmBus-Treiber installiert?"); crate::util::log(format!("Schaltung: virtueller Controller ging nicht ({e:?})")); false }
    }
}
fn pad_off() { if PAD.lock().unwrap().take().is_some() { crate::util::log("Schaltung: virtueller Controller abgesteckt"); } }
// gehaltene Controller-Knöpfe (Handbremse = A) – werden bei jedem Schalt-Tipp mitgeschickt
static HELD: AtomicU16 = AtomicU16::new(0);
fn pad_set(btn: u16) {
    if let Some(t) = PAD.lock().unwrap().as_mut() {
        let raw = btn | HELD.load(Relaxed);
        let _ = t.update(&vigem_client::XGamepad { buttons: vigem_client::XButtons { raw }, ..Default::default() });
    }
}

// ---- ein Knopf drücken/loslassen, je nach Schaltweg ----
#[derive(Clone, Copy)]
enum Btn { Up, Dn, Hb }
fn press(via: char, b: Btn, down: bool) {
    let (pup, pdn, kup, kdn) = with(|s| (s.pup, s.pdn, s.up, s.dn));
    match via {
        'v' => vj_btn(match b { Btn::Up => 1, Btn::Dn => 2, Btn::Hb => 3 }, down),
        'p' => {
            if !pad_on() { return; }
            match b {
                Btn::Hb => { if down { HELD.fetch_or(0x1000, Relaxed); } else { HELD.fetch_and(!0x1000, Relaxed); } pad_set(0); }
                _ => pad_set(if down { if matches!(b, Btn::Up) { pup } else { pdn } } else { 0 }),
            }
        }
        _ => key(match b { Btn::Up => kup, Btn::Dn => kdn, Btn::Hb => 0x39 }, down),
    }
}
// kurz drücken: 40 ms reichen, Forza liest die Geräte jedes Bild
fn tap(via: char, b: Btn, ms: u64) {
    std::thread::spawn(move || { press(via, b, true); std::thread::sleep(std::time::Duration::from_millis(ms)); press(via, b, false); });
}

// Handbremse übers Dashboard (P): läuft die Automatik über vJoy/Controller, dort halten statt der Leertaste.
// Gibt true zurück, wenn es übernommen wurde (sonst drückt media.rs die Leertaste).
static HB_VIA: Mutex<Option<char>> = Mutex::new(None);
pub fn hb_pad(down: bool) -> bool {
    if down {
        let via = with(|s| if s.an && s.via != 'k' { Some(s.via) } else { None });
        let Some(via) = via else { return false };
        let ok = if via == 'v' { vj_on() } else { pad_on() };
        if !ok { return false; }
        press(via, Btn::Hb, true);
        *HB_VIA.lock().unwrap() = Some(via);
        true
    } else if let Some(via) = HB_VIA.lock().unwrap().take() {
        press(via, Btn::Hb, false); true
    } else { false }
}

// Probe-Druck zum Belegen in Forza: 12 s lang jede Sekunde kurz drücken (erst nach 2 s) – man muss in Forza nur das Feld
// auswählen, irgendein Druck in dieser Zeit wird erkannt. Läuft auch, wenn Forza nicht vorne ist.
pub fn probe(which: &str) -> Value {
    let b = match which { "up" => Btn::Up, "dn" => Btn::Dn, "hb" => Btn::Hb, _ => return json!({"ok": false}) };
    let via = with(|s| s.via);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2000));
        for _ in 0..12 { press(via, b, true); std::thread::sleep(std::time::Duration::from_millis(250)); press(via, b, false); std::thread::sleep(std::time::Duration::from_millis(750)); }
    });
    json!({"ok": true})
}


// ---- Fahrtenbuch der Schaltung (zur Fehlersuche): die letzten 600 Ereignisse, abrufbar unter /schaltung/log ----
static GLOG: Mutex<std::collections::VecDeque<String>> = Mutex::new(std::collections::VecDeque::new());
fn glog(line: String) {
    let mut g = GLOG.lock().unwrap();
    if g.len() >= 600 { g.pop_front(); }
    g.push_back(format!("{:>9.3} {line}", clock() as f64 / 1000.0 % 100000.0));
}
pub fn log_text() -> String { GLOG.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n") }

// Belegung der Wippen (vom Dashboard geschickt): direkt beim Lesen des Lenkrads ausgewertet – kein Umweg übers Fenster
static WIP: Mutex<[Option<(String, u16)>; 2]> = Mutex::new([None, None]);
pub fn wippe_btn(dev: &str, btn: u16) -> bool {
    let hit = { let w = WIP.lock().unwrap(); w.iter().position(|x| x.as_ref().is_some_and(|(d, b)| d == dev && *b == btn)) };
    let Some(i) = hit else { return false };
    if !with(|s| s.an) { return false; }
    wippe(if i == 0 { 1 } else { -1 });
    true
}
// Schaltwippe am Lenkrad: wer selbst schaltet, ist im Modus M (wie bei einer echten Automatik mit Wippen).
// Ob Forza die Wippe selbst kennt (zweite Belegung) oder nur noch vJoy, wird beim ersten Mal erkannt:
// ändert sich der Gang innerhalb von 250 ms von allein, bleibt die Wippe direkt – sonst reicht das Programm sie an vJoy weiter.
pub fn wippe(dir: i64) -> Value {
    let now = clock();
    let dir: i8 = if dir > 0 { 1 } else { -1 };
    let send = with(|s| {
        if s.an && matches!(s.mode, 'D' | 'E' | 'S') { s.prev = s.mode; s.mode = 'M'; glog(format!("Wippe {} → Modus M", if dir > 0 { "hoch" } else { "runter" })); }
        if s.via != 'v' { s.wip_probe = None; return None; }   // ohne vJoy schaltet die Wippe ohnehin direkt
        match s.wip_direct {
            Some(true) => { s.pend = Some(Pend { dir, t: now, from: s.gear }); None }
            Some(false) => { s.pend = Some(Pend { dir, t: now, from: s.gear }); Some(s.via) }
            None => { s.wip_probe = Some((now, dir, s.gear)); None }
        }
    });
    #[cfg(not(test))]
    if let Some(via) = send { tap(via, if dir > 0 { Btn::Up } else { Btn::Dn }, 70); }
    #[cfg(test)]
    let _ = send;
    json!({"ok": true})
}
#[cfg(test)]
static SIM: Mutex<Vec<i8>> = Mutex::new(Vec::new());

fn shift(s: &mut St, dir: i8, now: u64, why: &str) -> bool {
    #[cfg(not(test))]
    {
        if !spiel_vorne() { return false; }
        let via = s.via;
        // vJoy etwas länger halten: Forza liest DirectInput-Geräte nicht ganz so oft wie den Controller
        let ms = if via == 'v' { 70 } else { 40 };
        // erst nach dem Lock-Ende drücken (press liest selbst die Einstellungen)
        std::thread::spawn(move || tap(via, if dir > 0 { Btn::Up } else { Btn::Dn }, ms));
    }
    glog(format!("[{}] {} {}→{}  {}", s.mode, if dir > 0 { "▲" } else { "▼" }, s.gear, s.gear as i16 + dir as i16, why));
    #[cfg(test)]
    SIM.lock().unwrap().push(dir);
    s.pend = Some(Pend { dir, t: now, from: s.gear });
    if dir > 0 { s.last_up = now } else { s.last_dn = now }
    true
}

fn note(s: &mut St, t: &str, now: u64) { s.hint = t.to_string(); s.hint_t = now; }


// Wird für jedes Telemetrie-Paket aufgerufen; gibt die Fahrstufe zurück (für die Anzeige), wenn die Automatik an ist.
pub fn tick(d: &Value) -> Option<String> {
    let now = clock();
    with(|s| {
        if !s.an { return None; }
        let n = |k: &str| d[k].as_f64().unwrap_or(0.0);
        let on = n("on") != 0.0 && n("dash") != 0.0;
        let gear = n("gear") as u8;
        let car = n("car") as i64;
        let (rpm, mx, idle) = (n("rpm"), n("rpmMax"), n("rpmIdle"));
        let spd = n("spd").abs();
        s.spd = spd;
        let label = Some(s.mode.to_string());
        if !on || mx < 1000.0 { s.gear = 255; s.pend = None; return label; }
        let gap = now.saturating_sub(s.seen); s.seen = now;
        if car != s.car { s.car = car; s.gear = 255; s.pend = None; }
        if s.gear == 255 || gap > 1000 { s.gear = gear; s.pend = None; return label; }   // nach Pause/Ladebildschirm erst ansehen

        let thr = n("acc") / 255.0;
        // Kickdown nur, wenn das Gas gerade erst durchgetreten wurde (nicht dauernd bei Vollgas, sonst pendelt es nach dem Hochschalten)
        // Forza nimmt beim Schalten kurz das Gas weg (in der Telemetrie sichtbar) – das wieder Durchtreten danach ist kein neuer Kickdown
        let since_shift = now.saturating_sub(s.last_up.max(s.last_dn));
        if thr > 0.85 && s.thr0 <= 0.85 && since_shift > 700 { s.kick_t = now; }
        s.thr0 = thr;
        let fresh_kick = now.saturating_sub(s.kick_t) < 900;
        let br = n("brk") / 255.0;
        let clu = n("clu");
        let lat = n("ax").abs();   // Querbeschleunigung m/s² (Kurve)
        let slip = d["slip"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).fold(0.0, f64::max)).unwrap_or(0.0);
        let span = (mx - idle).max(500.0);
        let r = ((rpm - idle) / span).clamp(0.0, 1.2);

        // Übersetzung lernen: Drehzahl je m/s im Gang (nur bei Haftung und geschlossener Kupplung)
        if gear >= 1 && gear <= 10 && spd > 4.0 && clu < 10.0 && slip < 0.6 && rpm > idle + 150.0 && gear == s.gear {
            let k = rpm / spd;
            let e = s.ratio.entry((car, gear)).or_insert(k);
            *e = if (k / *e - 1.0).abs() > 0.25 { *e * 0.8 + k * 0.2 } else { *e * 0.93 + k * 0.07 };
        }
        // Leistungsspitze lernen (für S+: dort hochschalten, wo die Kraft nachlässt)
        let pow = n("pow");
        if thr > 0.95 && pow > 0.0 && slip < 1.0 {
            let p = s.peak.entry(car).or_insert((rpm, pow));
            if pow > p.1 { *p = (rpm, pow); }
        }

        // Wippe gedrückt: hat Forza selbst geschaltet (Wippe dort belegt) oder muss das Programm sie an vJoy weiterreichen?
        if let Some((t0, dir, from)) = s.wip_probe {
            if gear != from && (gear as i16 - from as i16).signum() as i8 == dir {
                s.wip_direct = Some(true); s.wip_probe = None;
                glog(format!("Wippe schaltet in Forza direkt ({} ms) – vJoy nur für die Automatik", now.saturating_sub(t0)));
            } else if now.saturating_sub(t0) > 250 {
                s.wip_direct = Some(false); s.wip_probe = None;
                s.pend = Some(Pend { dir, t: now, from: gear });
                glog("Wippe schaltet in Forza nicht direkt – wird ab jetzt an vJoy weitergereicht".into());
                #[cfg(not(test))]
                { let via = s.via; std::thread::spawn(move || tap(via, if dir > 0 { Btn::Up } else { Btn::Dn }, 70)); }
            }
        }

        // Ergebnis des letzten Knopfdrucks – Forza darf sich bis 1,2 s Zeit lassen (vJoy wird etwas seltener gelesen)
        if gear != s.gear {
            let ours = s.pend.as_ref().map(|p| now.saturating_sub(p.t) < 1200 && (gear as i16 - p.from as i16).signum() as i8 == p.dir).unwrap_or(false)
                || s.wip_probe.is_some();
            if let Some(p) = s.pend.as_ref().filter(|_| ours) { glog(format!("   ✓ Gang {} nach {} ms", gear, now.saturating_sub(p.t))); }
            if ours { s.ok_shifts += 1; s.miss = 0; }
            else if (gear as i16 - s.gear as i16).abs() == 1 && spd > 1.0 && matches!(s.mode, 'D' | 'E' | 'S') {
                // selbst geschaltet (Wippe ohne Belegung im Dashboard, Taste) – oder Forza schaltet noch selbst
                glog(format!("Gang {}→{} ohne Programm – Modus M", s.gear, gear));
                s.prev = s.mode; s.mode = 'M';
                if s.ok_shifts == 0 { note(s, "Forza schaltet selbst – im Spiel Getriebe auf „Manuell“ stellen", now); }
                else { note(s, "Selbst geschaltet – Wählhebel steht jetzt auf M", now); }
            }
            s.pend = None;
            s.gear = gear;
        } else if let Some(p) = &s.pend {
            if now.saturating_sub(p.t) > 1200 {
                let (dir, from) = (p.dir, p.from);
                s.pend = None;
                glog(format!("   ✗ kein Gangwechsel nach 1200 ms (Gang {from})"));
                if dir > 0 && s.ok_shifts > 0 && from >= 3 { s.top.insert(car, from); }   // oberster Gang erreicht
                else {
                    s.miss += 1;
                    if s.miss >= 3 && s.ok_shifts == 0 { note(s, match s.via { 'v' => "Schaltet nicht – in Forza Getriebe auf „Manuell“ und im Lenkrad-Profil Hoch/Runter auf die vJoy-Knöpfe legen (Einrichten)", 'p' => "Schaltet nicht – in Forza Getriebe auf „Manuell“; am Controller Hoch/Runter auf B / X", _ => "Schaltet nicht – in Forza Getriebe auf „Manuell“ und Hoch/Runter auf E/Q lassen" }, now); }
                }
            }
        }
        if s.pend.is_some() || s.wip_probe.is_some() { return label; }

        // Drehzahl nach dem Schalten vorhersagen (gelernt, sonst typischer Gangsprung)
        let rt: [Option<f64>; 3] = [s.ratio.get(&(car, gear.saturating_sub(1))).copied(), s.ratio.get(&(car, gear)).copied(), s.ratio.get(&(car, gear + 1)).copied()];
        let after = move |to: u8| -> f64 {
            let b = if to > gear { rt[2] } else { rt[0] };
            let f = match (rt[1], b) { (Some(a), Some(b)) if a > 0.0 => b / a, _ => if to > gear { 0.74 } else { 1.0 / 0.74 } };
            ((rpm * f - idle) / span).clamp(0.0, 1.5)
        };
        let top = s.top.get(&car).copied().unwrap_or(10);

        match s.mode {
            'R' => {
                if gear >= 1 && spd < 1.2 && now.saturating_sub(s.last_dn) > 250 { shift(s, -1, now, "R einlegen"); }
            }
            'D' | 'E' | 'S' => {
                let m = s.mode;
                if gear == 0 { if spd < 2.0 && now.saturating_sub(s.last_up) > 250 { shift(s, 1, now, "aus R in den 1."); } return label; }
                // Stillstand / ganz langsam: zurück in den ersten
                if spd < 1.5 && gear > 1 { if now.saturating_sub(s.last_dn) > 220 { shift(s, -1, now, &format!("Stillstand → 1. ({:.0} km/h)", spd * 3.6)); } return label; }
                let (lo, hi, dn, mut wait) = match m { 'E' => (0.15, 0.60, 0.09, 650), 'S' => (0.55, 0.97, 0.40, 220), _ => (0.22, 0.88, 0.12, 420) };
                if thr < 0.3 && m != 'S' { wait = wait.max(900); }   // bei wenig Gas gemächlich einen Gang nach dem anderen
                let mut up_at = lo + (hi - lo) * ((thr - 0.05) / 0.85).clamp(0.0, 1.0).powf(1.6);   // wenig Gas = früh hoch wie im echten Auto
                if m == 'S' {
                    // S+: kurz hinter der gelernten Leistungsspitze hoch (aber nie in den Begrenzer)
                    if let Some(&(prpm, _)) = s.peak.get(&car) { let pr = ((prpm - idle) / span + 0.04).clamp(0.75, 0.97); if thr > 0.8 { up_at = pr; } }
                }
                let since_up = now.saturating_sub(s.last_up);
                let since_dn = now.saturating_sub(s.last_dn);
                let since = since_up.min(since_dn);
                let limiter = r > 0.965;
                // --- hoch ---
                if gear < top && spd > 2.0 && (limiter || since >= wait) {
                    let hold = (br > 0.1 && !limiter)                                   // beim Bremsen nicht hoch
                        || (m == 'S' && thr < 0.05 && !limiter)                         // S+: Gas weg = Gang halten (Kurve)
                        || (slip > 1.6 && r < 0.93)                                     // durchdrehende Räder: halten
                        || (since_dn < 1400 && !limiter && r < hi)                      // nach Rückschalten nicht gleich wieder hoch
                        || (lat > if m == 'S' { 5.0 } else { 7.0 } && !limiter && r < 0.93);   // in der Kurve Gang halten (wie eine echte Automatik)
                    if !hold && r >= up_at && (limiter || after(gear + 1) > dn + 0.09) { let w = format!("hoch bei {:.0}/min (ab {:.0}), Gas {:.0}% {:.0} km/h{}", rpm, idle + up_at * span, thr * 100.0, spd * 3.6, if limiter { " Begrenzer" } else { "" }); shift(s, 1, now, &w); return label; }
                }
                // --- runter ---
                if gear > 1 && since >= wait.min(400) {
                    let a = after(gear - 1);
                    if a < 0.93 {
                        let dn2 = if m == 'S' && thr < 0.2 { 0.28 } else { dn };   // S+ rollend: etwas tiefer erlaubt
                        let lug = r < dn2 && a < up_at.max(lo) * 0.95 + 0.05 && since_dn > 700;                               // Drehzahl zu tief
                        let kick = m != 'E' && thr > 0.85 && fresh_kick && since_up > 1200 && a < if m == 'S' { 0.93 } else { 0.84 } && r < if m == 'S' { 0.78 } else { 0.62 };
                        let eco_kick = m == 'E' && thr > 0.97 && fresh_kick && since_up > 1200 && a < 0.72 && r < 0.4;
                        let brake = br > 0.25 && a < if m == 'S' { 0.86 } else { 0.48 } && r < if m == 'S' { 0.7 } else { 0.32 };
                        let sport = m == 'S' && thr > 0.3 && r < dn && a < 0.9 && since_up > 1500;
                        if (lug || kick || eco_kick || brake || sport) && slip < 1.6 && (since_up > 700 || kick || brake) { let w = format!("runter ({}) bei {:.0}/min, danach ~{:.0}/min, Gas {:.0}% Bremse {:.0}% {:.0} km/h", if kick { "Kickdown" } else if brake { "Bremsen" } else if sport { "Sport" } else if eco_kick { "Kickdown" } else { "Drehzahl tief" }, rpm, idle + a * span, thr * 100.0, br * 100.0, spd * 3.6); shift(s, -1, now, &w); }
                    }
                }
            }
            _ => {}   // P (Handbremse übers Dashboard) und M: nichts tun
        }
        label
    })
}

pub fn state() -> Value {
    with(|s| {
        let gears = s.ratio.keys().filter(|(c, _)| *c == s.car).count();
        let hint = if clock().saturating_sub(s.hint_t) < 15000 { s.hint.clone() } else { String::new() };
        let err = if s.an { DEV_ERR.lock().unwrap().clone() } else { String::new() };
        let vj = vj_lib().is_some();
        json!({"ok": true, "an": s.an, "mode": s.mode.to_string(), "via": match s.via { 'v' => "vjoy", 'p' => "pad", _ => "key" }, "err": err, "vjoy": vj, "pup": s.pup, "pdn": s.pdn, "gelernt": gears, "geschaltet": s.ok_shifts, "hint": hint})
    })
}

// {an, mode, via ("vjoy" | "pad" = Controller | "key" = Tastatur), pup, pdn (Controller-Knöpfe), up, dn (Tasten)} – alles optional. R und P nur fast im Stand (wie beim echten Wählhebel).
pub fn set(b: &Value) -> Value {
    let r = with(|s| {
        if let Some(a) = b["an"].as_bool() { if a && !s.an { s.mode = 'D'; s.gear = 255; s.miss = 0; } s.an = a; }
        if let Some(v) = b["via"].as_str() { DEV_ERR.lock().unwrap().clear(); s.via = match v { "vjoy" => 'v', "key" => 'k', _ => 'p' }; }
        if let Some(u) = b["pup"].as_u64() { if u > 0 && u <= 0xF000 { s.pup = u as u16; } }
        if let Some(u) = b["pdn"].as_u64() { if u > 0 && u <= 0xF000 { s.pdn = u as u16; } }
        for (i, k) in ["wup", "wdn"].iter().enumerate() {
            if b[*k].is_null() { continue; }
            WIP.lock().unwrap()[i] = b[*k]["dev"].as_str().zip(b[*k]["btn"].as_u64()).map(|(d, n)| (d.chars().take(40).collect(), n as u16));
        }
        if let Some(u) = b["up"].as_u64() { if u > 0 && u < 0x80 { s.up = u as u16; } }
        if let Some(u) = b["dn"].as_u64() { if u > 0 && u < 0x80 { s.dn = u as u16; } }
        if let Some(m) = b["mode"].as_str().and_then(|m| m.chars().next()) {
            let m = m.to_ascii_uppercase();
            if !"PRDESM".contains(m) { return Some("Unbekannte Fahrstufe"); }
            if matches!(m, 'P' | 'R') && s.spd * 3.6 > 6.0 && s.mode != m { return Some("Erst anhalten"); }
            if s.mode != m { glog(format!("Fahrstufe {} → {}", s.mode, m)); }
            s.mode = m;
            if matches!(m, 'D' | 'E' | 'S') { s.prev = m; }
            if m != 'M' { s.hint.clear(); }
        }
        None
    });
    // Controller nur angesteckt, solange die Automatik über den Controller schaltet (Anstecken dauert etwas – im Hintergrund)
    let (an, via) = with(|s| (s.an, s.via));
    #[cfg(not(test))]
    {
        if an && via == 'p' { std::thread::spawn(|| { pad_on(); }); } else { pad_off(); }
        if an && via == 'v' { std::thread::spawn(|| { vj_on(); }); } else { vj_off(); }
    }
    #[cfg(test)]
    let _ = (an, via);
    let mut v = state();
    if let Some(e) = r { v["ok"] = json!(false); v["err"] = json!(e); }
    v
}

#[cfg(not(test))]
fn clock() -> u64 { crate::util::now_ms() }
#[cfg(test)]
static CLK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_000_000);
#[cfg(test)]
fn clock() -> u64 { CLK.load(std::sync::atomic::Ordering::Relaxed) }

// Probefahrt mit einem einfachen Auto-Modell (7 Gänge, 7.000/min): zeigt, wo geschaltet wird
#[cfg(test)]
mod probe {
    use super::*;
    use std::sync::atomic::Ordering;

    fn fahrt(mode: &str, teile: &[(f64, f64, f64)]) -> Vec<String> {
        set(&json!({"an": true, "mode": mode}));
        with(|s| { s.gear = 255; s.ratio.clear(); s.peak.clear(); s.last_up = 0; s.last_dn = 0; s.pend = None; s.mode = mode.chars().next().unwrap(); });
        let gr = [3.6, 2.2, 1.55, 1.2, 1.0, 0.82, 0.68];
        let (fd, rad, mx, idle) = (3.5, 0.33, 7000.0, 800.0);
        let (mut v, mut g, mut t): (f64, u8, f64) = (0.0, 1, 0.0);
        let mut queue: Vec<(f64, i8)> = Vec::new();
        let mut log = Vec::new();
        let mut maxr: f64 = 0.0;
        let mut cut_until = -1.0;   // wie Forza: beim Gangwechsel 0,15 s kein Gas in der Telemetrie
        for &(dur, thr, brk) in teile {
            let end = t + dur;
            while t < end {
                let ratio = gr[(g - 1) as usize] * fd;
                let w = v / rad * 60.0 / (2.0 * std::f64::consts::PI) * ratio;
                let rpm = w.max(idle + thr * 1800.0 * (1.0 - (v / 3.0).min(1.0))).min(mx);
                maxr = maxr.max(rpm);
                let tq = 420.0 * (1.0 - ((rpm - 4600.0) / 4200.0).powi(2)).max(0.25);
                let f = if rpm >= mx * 0.995 { 0.0 } else { thr * tq * ratio / rad };
                let pow = tq * rpm * 2.0 * std::f64::consts::PI / 60.0 * thr;
                v = (v + (f / 1500.0 - 0.4 * v * v / 1500.0 - 0.15 - brk * 9.0) * 0.016).max(0.0);
                CLK.store((t * 1000.0) as u64 + 1_000_000, Ordering::Relaxed);
                let d = json!({"on": 1, "dash": 1, "gear": g, "car": 1, "rpm": rpm, "rpmMax": mx, "rpmIdle": idle, "spd": v,
                               "acc": if t < cut_until { 0.0 } else { (thr * 255.0).round() }, "brk": (brk * 255.0).round(), "clu": 0, "slip": [0.1, 0.1, 0.1, 0.1], "pow": pow});
                tick(&d);
                for dir in SIM.lock().unwrap().drain(..) { queue.push((t + 0.06, dir)); log.push(format!("{:5.1}s {:>3.0} km/h  {} {}→{}  bei {:4.0}/min  Gas {:3.0}% Bremse {:3.0}%", t, v * 3.6, if dir > 0 { "▲" } else { "▼" }, g, (g as i8 + dir), rpm, thr * 100.0, brk * 100.0)); }
                queue.retain(|&(at, dir)| { if t >= at { g = ((g as i8 + dir).clamp(1, 7)) as u8; cut_until = t + 0.15; false } else { true } });
                t += 0.016;
            }
        }
        log.push(format!("höchste Drehzahl {:.0}/min, Ende {:.0} km/h im {}.", maxr, v * 3.6, g));
        log
    }

    #[test]
    fn probefahrt() {
        let gemischt = [(14.0, 0.3, 0.0), (8.0, 1.0, 0.0), (3.0, 0.15, 0.0), (9.0, 0.0, 0.45), (3.0, 0.0, 0.0)];
        for m in ["D", "E", "S"] {
            println!("\n=== Fahrstufe {m}: sanft anfahren, Vollgas, rollen, bremsen bis Stillstand ===");
            for l in fahrt(m, &gemischt) { println!("{l}"); }
        }
    }
}
