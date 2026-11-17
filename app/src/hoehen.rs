// Gelände aus den Fahrdaten aller Spieler: Jedes Dashboard misst beim Fahren die Höhe je 16-m-Feld und teilt seine
// Messungen über einen gemeinsamen, verschlüsselten Kanal (liegt beim Vermittler, auch für später Kommende).
// Hier werden alle Messungen zusammengeführt (gewichteter Mittelwert, mehr Messungen = mehr Gewicht).
// Geteilt werden nur Feldnummer, Höhe und Anzahl der Messungen – keine Namen, keine Fahrten, keine Zeiten.
use crate::kanal::{seal, Kanal};
use crate::mqtt::Net;
use crate::state::Shared;
use crate::util::{log, now_ms};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const KANAL: &str = "FDHOEHE01";
const MAX_SEND: usize = 24_000;      // eigene Felder je Veröffentlichung (die am häufigsten gemessenen)
const MAX_PER_DEV: usize = 40_000;
const MAX_DEVS: usize = 400;
const BUF_MAX: usize = 190_000;      // verschlüsselt + komprimiert

struct Hoehen {
    net: Arc<Net>,
    dev: String,
    k: Mutex<Option<Kanal>>,
    alle: Mutex<HashMap<String, (u64, Vec<[i32; 4]>)>>,   // Gerät -> (Zeit, [i, j, h*10, n])
    mein: Mutex<Vec<[i32; 4]>>,
    mein_sig: Mutex<(usize, i64)>,
    last_pub: Mutex<u64>,
}
static H: OnceLock<Arc<Hoehen>> = OnceLock::new();

fn ok_cell(c: &Value) -> Option<[i32; 4]> {
    let a = c.as_array()?;
    if a.len() != 4 { return None; }
    let v: Vec<i64> = a.iter().map(|x| x.as_i64()).collect::<Option<Vec<_>>>()?;
    if v[0].abs() > 200_000 || v[1].abs() > 200_000 || v[2].abs() > 30_000 || v[3] < 1 || v[3] > 1000 { return None; }
    Some([v[0] as i32, v[1] as i32, v[2] as i32, v[3].min(30) as i32])
}

fn parse_flat(v: &Value, max: usize) -> Vec<[i32; 4]> {
    let Some(a) = v.as_array() else { return vec![] };
    a.chunks(4).take(max).filter_map(|c| ok_cell(&json!(c))).collect()
}

pub fn init(s: &Arc<Shared>) {
    let h = Arc::new(Hoehen { net: s.net.clone(), dev: s.sid.clone(), k: Mutex::new(None), alle: Mutex::new(HashMap::new()),
        mein: Mutex::new(vec![]), mein_sig: Mutex::new((0, 0)), last_pub: Mutex::new(0) });
    if H.set(h.clone()).is_err() { return; }
    let w = Arc::downgrade(&h);
    let k = Kanal::new(&s.net, "hgt", KANAL, Arc::new(move |o: Value, sub: &str| {
        let Some(h) = w.upgrade() else { return };
        let Some(dev) = sub.strip_prefix("u/") else { return };
        if dev == h.dev || dev.len() > 40 || o["y"] != "hgt" { return; }
        let cells = parse_flat(&o["c"], MAX_PER_DEV);
        let mut a = h.alle.lock().unwrap();
        if a.len() >= MAX_DEVS && !a.contains_key(dev) { return; }
        a.insert(dev.to_string(), (now_ms(), cells));
    }));
    *h.k.lock().unwrap() = Some(k);
}

// eigene Messungen vom Dashboard übernehmen und (höchstens alle 10 Minuten, bei Änderung) teilen
pub fn mein(v: &Value) -> Value {
    let Some(h) = H.get() else { return json!({"ok": false}) };
    let mut cells = parse_flat(v, 200_000);
    cells.sort_by(|a, b| b[3].cmp(&a[3]));
    cells.truncate(MAX_SEND);
    let sig = (cells.len(), cells.iter().map(|c| c[2] as i64 * 7 + c[3] as i64).sum::<i64>());
    let changed = *h.mein_sig.lock().unwrap() != sig;
    // keine eigenen Daten mehr (z. B. gelöscht): sofort auch beim Vermittler entfernen
    if cells.is_empty() {
        if changed && h.net.ok() { *h.mein_sig.lock().unwrap() = sig; if let Some(k) = h.k.lock().unwrap().as_ref() { h.net.publish(&format!("{}/u/{}", k.topic(), h.dev), &[], true); } }
        *h.mein.lock().unwrap() = cells;
        return json!({"ok": true});
    }
    *h.mein.lock().unwrap() = cells;
    let due = now_ms().saturating_sub(*h.last_pub.lock().unwrap()) > 10 * 60_000;
    if changed && due && h.net.ok() {
        *h.mein_sig.lock().unwrap() = sig;
        *h.last_pub.lock().unwrap() = now_ms();
        let flat: Vec<i32> = h.mein.lock().unwrap().iter().flat_map(|c| c.iter().copied()).collect();
        let ks = h.k.lock().unwrap();
        if let Some(k) = ks.as_ref() {
            let mut n = flat.len() / 4;
            let buf = loop {
                let b = seal(&k.ch, &json!({"y": "hgt", "c": flat[..n * 4], "t": now_ms()}));
                if b.len() <= BUF_MAX || n < 500 { break b; }
                n = n * 3 / 4;
            };
            h.net.publish(&format!("{}/u/{}", k.topic(), h.dev), &buf, true);
            log(format!("Gelände: {n} eigene Höhenfelder geteilt"));
        }
    }
    json!({"ok": true})
}

// alle fremden Messungen zusammengeführt (das Dashboard mischt die eigenen selbst dazu)
pub fn alle() -> Value {
    let Some(h) = H.get() else { return json!({"ok": false}) };
    let a = h.alle.lock().unwrap();
    let mut m: HashMap<(i32, i32), (f64, f64)> = HashMap::new();
    for (_, cells) in a.values() {
        for c in cells {
            let w = c[3] as f64;
            let e = m.entry((c[0], c[1])).or_insert((0.0, 0.0));
            e.0 += c[2] as f64 * w; e.1 += w;
        }
    }
    let mut flat: Vec<i64> = Vec::with_capacity(m.len() * 4);
    for ((i, j), (s, w)) in m.iter() { flat.extend([*i as i64, *j as i64, (s / w).round() as i64, w.round().min(300.0) as i64]); }
    json!({"ok": true, "spieler": a.len(), "felder": m.len(), "c": flat})
}
