// Eigene Automatik: Forza steht auf "Manuell", das Programm schaltet für dich – über einen unsichtbaren Xbox-Controller
// (ViGEmBus, RB = hoch, LB = runter), damit Forza beim Lenkrad bleibt. Notlösung ohne Treiber: Tastatur (E/Q).
// Fahrstufen wie im Automatik-Wagen: P (Handbremse, macht das Dashboard), R, D, E (Eco), S (S+), M (selbst schalten).
// Schaltpunkte wie im echten Auto: wenig Gas = früh hoch (D ≈ 2.500/min), Vollgas = spät hoch, Kickdown, beim Bremsen
// sanft zurück. Die Übersetzungen lernt es pro Auto aus Drehzahl/Tempo, damit nach dem Schalten die Drehzahl passt
// und nichts hin- und herpendelt. Schaltet man selbst (Wippen/Tasten), geht es von allein auf M.
use serde_json::{json, Value};
use std::collections::HashMap;
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
    hold_t: u64,
    hold_done: bool,
    pad: bool,
}

static ST: Mutex<Option<St>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut St) -> R) -> R {
    let mut g = ST.lock().unwrap();
    let s = g.get_or_insert_with(|| St {
        an: false, mode: 'D', up: 0x12, dn: 0x10, car: 0, ratio: HashMap::new(), top: HashMap::new(), peak: HashMap::new(),
        gear: 255, seen: 0, pend: None, last_up: 0, last_dn: 0, ok_shifts: 0, miss: 0, hint: String::new(), hint_t: 0, spd: 0.0, thr0: 0.0, kick_t: 0, prev: 'D', hold_t: 0, hold_done: false, pad: true,
    });
    f(s)
}

// nur ins Spiel tippen – nicht ins Dashboard, Discord oder den Browser
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

fn key(scan: u16, down: bool) {
    let flags = if down { KEYEVENTF_SCANCODE } else { KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP };
    let inp = [INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }];
    unsafe { SendInput(&inp, std::mem::size_of::<INPUT>() as i32); }
}
// kurzer Tipp (35 ms) – lang genug, dass Forza ihn sicher sieht, kurz genug für schnelle Schaltvorgänge
fn tap(scan: u16) {
    std::thread::spawn(move || { key(scan, true); std::thread::sleep(std::time::Duration::from_millis(35)); key(scan, false); });
}


// ---- unsichtbarer Xbox-Controller (ViGEmBus) ----
// Tastatur-Eingaben lassen Forza kurz vom Lenkrad auf die Tastatur umschalten (Ruck, Lenkrad neu ausgerichtet).
// Ein Controller-Knopf tut das nicht in gleicher Weise; der Controller wird nur angesteckt, solange die Automatik an ist.
static PAD: Mutex<Option<vigem_client::Xbox360Wired<vigem_client::Client>>> = Mutex::new(None);
static PAD_ERR: Mutex<String> = Mutex::new(String::new());
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
        Ok(t) => { *g = Some(t); PAD_ERR.lock().unwrap().clear(); crate::util::log("Schaltung: virtueller Controller angesteckt"); true }
        Err(e) => { *PAD_ERR.lock().unwrap() = format!("{e:?}"); crate::util::log(format!("Schaltung: virtueller Controller ging nicht ({e:?})")); false }
    }
}
fn pad_off() { if PAD.lock().unwrap().take().is_some() { crate::util::log("Schaltung: virtueller Controller abgesteckt"); } }
fn pad_set(btn: u16) {
    if let Some(t) = PAD.lock().unwrap().as_mut() {
        let raw = btn | HELD.load(std::sync::atomic::Ordering::Relaxed);
        let _ = t.update(&vigem_client::XGamepad { buttons: vigem_client::XButtons { raw }, ..Default::default() });
    }
}
// gehaltene Knöpfe (Handbremse = A) – werden bei jedem Schalt-Tipp mitgeschickt
static HELD: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
static HB_PAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
// Handbremse übers Dashboard (P): läuft die Automatik über den Controller, dort A halten statt der Leertaste (kein Gerätewechsel).
// Gibt true zurück, wenn der Controller es übernommen hat.
pub fn hb_pad(down: bool) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if down {
        let use_pad = with(|s| s.an && s.pad) && pad_on();
        HB_PAD.store(use_pad, Relaxed);
        if use_pad { HELD.fetch_or(0x1000, Relaxed); pad_set(0); }
        use_pad
    } else if HB_PAD.swap(false, Relaxed) {
        HELD.fetch_and(!0x1000, Relaxed); pad_set(0); true
    } else { false }
}
// Knopf 40 ms drücken – Forza liest den Controller pro Bild, das reicht sicher für einen Schaltvorgang
fn pad_tap(btn: u16) {
    std::thread::spawn(move || {
        if !pad_on() { return; }
        pad_set(btn);
        std::thread::sleep(std::time::Duration::from_millis(40));
        pad_set(0);
    });
}
#[cfg(test)]
static SIM: Mutex<Vec<i8>> = Mutex::new(Vec::new());

fn shift(s: &mut St, dir: i8, now: u64) -> bool {
    #[cfg(not(test))]
    {
        if !spiel_vorne() { return false; }
        // Controller: RB = hoch, LB = runter
        if s.pad { pad_tap(if dir > 0 { 0x0200 } else { 0x0100 }); } else { tap(if dir > 0 { s.up } else { s.dn }); }
    }
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
        if thr > 0.85 && s.thr0 <= 0.85 { s.kick_t = now; }
        s.thr0 = thr;
        let fresh_kick = now.saturating_sub(s.kick_t) < 900;
        let br = n("brk") / 255.0;
        let clu = n("clu");
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

        // Ergebnis des letzten Tastendrucks
        if gear != s.gear {
            let ours = s.pend.as_ref().map(|p| now.saturating_sub(p.t) < 700 && (gear as i16 - p.from as i16).signum() as i8 == p.dir).unwrap_or(false);
            if ours { s.ok_shifts += 1; s.miss = 0; }
            else if (gear as i16 - s.gear as i16).abs() == 1 && spd > 1.0 && matches!(s.mode, 'D' | 'E' | 'S') {
                // selbst geschaltet (Wippe/Taste) – oder Forza schaltet noch selbst
                s.mode = 'M';
                if s.ok_shifts == 0 { note(s, "Forza schaltet selbst – im Spiel Getriebe auf „Manuell“ stellen", now); }
                else { note(s, "Selbst geschaltet – Wählhebel steht jetzt auf M", now); }
            }
            s.pend = None;
            s.gear = gear;
        } else if let Some(p) = &s.pend {
            if now.saturating_sub(p.t) > 450 {
                let (dir, from) = (p.dir, p.from);
                s.pend = None;
                if dir > 0 && s.ok_shifts > 0 && from >= 3 { s.top.insert(car, from); }   // oberster Gang erreicht
                else {
                    s.miss += 1;
                    if s.miss >= 3 && s.ok_shifts == 0 { note(s, if s.pad { "Schaltet nicht – im Spiel Getriebe auf „Manuell“ stellen; am Controller muss Hochschalten auf RB und Runterschalten auf LB liegen" } else { "Schaltet nicht – im Spiel Getriebe auf „Manuell“ stellen und Hoch/Runter auf E/Q lassen" }, now); }
                }
            }
        }
        if s.pend.is_some() { return label; }

        // Rückwärts wie beim Automatik-Wagen: im Stand die Bremse 1 s halten = R, nochmal 1 s halten = wieder vorwärts
        if matches!(s.mode, 'D' | 'E' | 'S' | 'R') && spd < 0.4 && br > 0.5 && thr < 0.05 {
            if s.hold_t == 0 { s.hold_t = now; }
            if !s.hold_done && now.saturating_sub(s.hold_t) > 1000 {
                s.hold_done = true;
                if s.mode == 'R' { s.mode = s.prev; } else { s.prev = s.mode; s.mode = 'R'; }
                return Some(s.mode.to_string());
            }
        } else { s.hold_t = 0; s.hold_done = false; }

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
                if gear >= 1 && spd < 1.2 && now.saturating_sub(s.last_dn) > 250 { shift(s, -1, now); }
            }
            'D' | 'E' | 'S' => {
                let m = s.mode;
                if gear == 0 { if spd < 2.0 && now.saturating_sub(s.last_up) > 250 { shift(s, 1, now); } return label; }
                // Stillstand / ganz langsam: zurück in den ersten
                if spd < 1.5 && gear > 1 { if now.saturating_sub(s.last_dn) > 220 { shift(s, -1, now); } return label; }
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
                        || (since_dn < 1400 && !limiter && r < hi);                     // nach Rückschalten nicht gleich wieder hoch
                    if !hold && r >= up_at && (limiter || after(gear + 1) > dn + 0.09) { shift(s, 1, now); return label; }
                }
                // --- runter ---
                if gear > 1 && since >= wait.min(400) {
                    let a = after(gear - 1);
                    if a < 0.93 {
                        let lug = r < dn && a < up_at.max(lo) * 0.95 + 0.05;                               // Drehzahl zu tief
                        let kick = m != 'E' && thr > 0.85 && fresh_kick && a < if m == 'S' { 0.93 } else { 0.84 } && r < if m == 'S' { 0.78 } else { 0.62 };
                        let eco_kick = m == 'E' && thr > 0.97 && fresh_kick && a < 0.72 && r < 0.4;
                        let brake = br > 0.25 && a < if m == 'S' { 0.86 } else { 0.48 } && r < if m == 'S' { 0.7 } else { 0.32 };
                        let sport = m == 'S' && thr > 0.3 && r < dn && a < 0.9 && since_up > 1500;
                        if (lug || kick || eco_kick || brake || sport) && slip < 1.6 && (since_up > 700 || kick || brake) { shift(s, -1, now); }
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
        let pad_err = PAD_ERR.lock().unwrap().clone();
        json!({"ok": true, "an": s.an, "mode": s.mode.to_string(), "via": if s.pad { "pad" } else { "key" }, "pad_err": pad_err, "up": s.up, "dn": s.dn, "gelernt": gears, "geschaltet": s.ok_shifts, "hint": hint})
    })
}

// {an, mode, via ("pad" = Controller, "key" = Tastatur), up, dn} – alles optional. R und P nur fast im Stand (wie beim echten Wählhebel).
pub fn set(b: &Value) -> Value {
    let r = with(|s| {
        if let Some(a) = b["an"].as_bool() { if a && !s.an { s.mode = 'D'; s.gear = 255; s.miss = 0; } s.an = a; }
        if let Some(v) = b["via"].as_str() { s.pad = v != "key"; }
        if let Some(u) = b["up"].as_u64() { if u > 0 && u < 0x80 { s.up = u as u16; } }
        if let Some(u) = b["dn"].as_u64() { if u > 0 && u < 0x80 { s.dn = u as u16; } }
        if let Some(m) = b["mode"].as_str().and_then(|m| m.chars().next()) {
            let m = m.to_ascii_uppercase();
            if !"PRDESM".contains(m) { return Some("Unbekannte Fahrstufe"); }
            if matches!(m, 'P' | 'R') && s.spd * 3.6 > 6.0 && s.mode != m { return Some("Erst anhalten"); }
            s.mode = m;
            if matches!(m, 'D' | 'E' | 'S') { s.prev = m; }
            if m != 'M' { s.hint.clear(); }
        }
        None
    });
    // Controller nur angesteckt, solange die Automatik über den Controller schaltet (Anstecken dauert etwas – im Hintergrund)
    let (an, pad) = with(|s| (s.an, s.pad));
    #[cfg(not(test))]
    { if an && pad { std::thread::spawn(|| { pad_on(); }); } else { pad_off(); } }
    #[cfg(test)]
    let _ = (an, pad);
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
                               "acc": (thr * 255.0).round(), "brk": (brk * 255.0).round(), "clu": 0, "slip": [0.1, 0.1, 0.1, 0.1], "pow": pow});
                tick(&d);
                for dir in SIM.lock().unwrap().drain(..) { queue.push((t + 0.06, dir)); log.push(format!("{:5.1}s {:>3.0} km/h  {} {}→{}  bei {:4.0}/min  Gas {:3.0}% Bremse {:3.0}%", t, v * 3.6, if dir > 0 { "▲" } else { "▼" }, g, (g as i8 + dir), rpm, thr * 100.0, brk * 100.0)); }
                queue.retain(|&(at, dir)| { if t >= at { g = ((g as i8 + dir).clamp(1, 7)) as u8; false } else { true } });
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
