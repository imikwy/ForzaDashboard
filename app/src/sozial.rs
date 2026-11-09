// Freunde, Einladungen und Online-Modus.
// - Jedes Dashboard hat ein Postfach: einen verschlüsselten Kanal aus der eigenen Geräte-Kennung. Wer die Kennung
//   kennt (aus einer gemeinsamen Kolonne oder dem Online-Modus), kann dort Freundschaftsanfragen und Einladungen
//   ablegen. Sie bleiben beim Vermittler liegen, bis der Empfänger online ist, und werden danach weggeräumt.
// - Freunde schicken sich alle 30 s ein Lebenszeichen ins Postfach – so sieht man, wer gerade online ist.
// - Einladungen in eine Kolonne verfallen nach 10 Minuten oder sobald die Kolonne geschlossen wird.
// - Online-Modus (abschaltbar, Standard aus): die eigene Position geht in einen öffentlichen Kanal; alle anderen
//   mit eingeschaltetem Online-Modus erscheinen auf der Karte.
use crate::kanal::{channel, seal, Kanal};
use crate::mqtt::Net;
use crate::state::{Friend, MyPos};
use crate::util::{log, now_ms, read_json, take_chars, write_json};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

const INV_TTL: u64 = 10 * 60 * 1000;
const WELT: &str = "FDWELT001";

#[derive(Default)]
struct Daten {
    freunde: Vec<Value>, // {id, n, t}
    rein: Vec<Value>,    // erhaltene Anfragen {id, n, t}
    raus: Vec<Value>,    // gesendete Anfragen {id, n, t}
}

pub struct Sozial {
    net: Arc<Net>,
    dev: String,
    file: PathBuf,
    d: Mutex<Daten>,
    inbox: Mutex<Option<Kanal>>,
    welt: Mutex<Option<Kanal>>,
    spieler: Mutex<HashMap<String, Friend>>,
    seen: Mutex<HashMap<String, u64>>,
    inv_in: Mutex<Vec<Value>>,  // {id, n, code, ln, t}
    inv_out: Mutex<Vec<Value>>, // {id, code, t}
    last_hi: Mutex<u64>,
    last_pub: Mutex<u64>,
    me_n: Mutex<String>,
}

fn id_of(v: &Value) -> &str { v["id"].as_str().unwrap_or("") }
fn valid_id(id: &str) -> bool { id.len() == 10 && id.bytes().all(|b| b.is_ascii_hexdigit()) }

impl Sozial {
    pub fn new(net: &Arc<Net>, dev: &str, root: &std::path::Path) -> Arc<Sozial> {
        let file = root.join("freunde.json");
        let v = read_json(&file).unwrap_or(Value::Null);
        let arr = |k: &str| v[k].as_array().cloned().unwrap_or_default().into_iter().filter(|e| valid_id(id_of(e))).collect::<Vec<_>>();
        Arc::new(Sozial {
            net: net.clone(), dev: dev.to_string(), file,
            d: Mutex::new(Daten { freunde: arr("freunde"), rein: arr("rein"), raus: arr("raus") }),
            inbox: Mutex::new(None), welt: Mutex::new(None), spieler: Mutex::new(HashMap::new()), seen: Mutex::new(HashMap::new()),
            inv_in: Mutex::new(Vec::new()), inv_out: Mutex::new(Vec::new()), last_hi: Mutex::new(0), last_pub: Mutex::new(0), me_n: Mutex::new(String::new()),
        })
    }

    fn save(&self) {
        let d = self.d.lock().unwrap();
        write_json(&self.file, &json!({"freunde": d.freunde, "rein": d.rein, "raus": d.raus}));
    }

    pub fn start(self: &Arc<Self>, welt: bool) {
        let w: Weak<Sozial> = Arc::downgrade(self);
        let k = Kanal::new(&self.net, "inbox", &self.dev, Arc::new(move |o: Value, sub: &str| {
            if let Some(s) = w.upgrade() { s.on_msg(o, sub); }
        }));
        *self.inbox.lock().unwrap() = Some(k);
        if welt { self.set_welt(true); }
    }

    // ---------------- Postfach ----------------
    fn send(&self, to: &str, kind: &str, msg: &Value, retain: bool) -> bool {
        if !valid_id(to) || to == self.dev { return false; }
        let ch = channel("inbox", to);
        let topic = if retain { format!("{}/r/{kind}/{}", ch.topic, self.dev) } else { format!("{}/m", ch.topic) };
        self.net.publish(&topic, &seal(&ch, msg), retain)
    }

    fn on_msg(&self, o: Value, sub: &str) {
        let d = o["d"].as_str().unwrap_or("");
        if !valid_id(d) || d == self.dev { return; }
        // liegengebliebene Nachricht gelesen -> beim Vermittler wegräumen
        if sub.starts_with("r/") {
            if let Some(k) = self.inbox.lock().unwrap().as_ref() { self.net.publish(&format!("{}/{sub}", k.topic()), &[], true); }
        }
        let n = take_chars(o["n"].as_str().unwrap_or("Fahrer"), 24);
        let t = o["t"].as_u64().unwrap_or(0);
        let now = now_ms();
        let mut changed = false;
        match o["y"].as_str().unwrap_or("") {
            "freq" => {
                let mut dd = self.d.lock().unwrap();
                if dd.freunde.iter().any(|f| id_of(f) == d) { drop(dd); let me = self.me_n.lock().unwrap().clone(); self.send(d, "f", &json!({"y": "facc", "d": self.dev, "n": me, "t": now}), true); return; }
                if dd.raus.iter().any(|f| id_of(f) == d) {
                    // beide haben sich gegenseitig angefragt -> gleich befreundet
                    dd.raus.retain(|f| id_of(f) != d);
                    dd.freunde.push(json!({"id": d, "n": n, "t": now}));
                } else if !dd.rein.iter().any(|f| id_of(f) == d) {
                    dd.rein.push(json!({"id": d, "n": n, "t": t.min(now)}));
                    log(format!("Freundschaftsanfrage von {n}"));
                }
                changed = true;
            }
            "facc" => {
                let mut dd = self.d.lock().unwrap();
                dd.raus.retain(|f| id_of(f) != d);
                dd.rein.retain(|f| id_of(f) != d);
                if !dd.freunde.iter().any(|f| id_of(f) == d) { dd.freunde.push(json!({"id": d, "n": n, "t": now})); }
                self.seen.lock().unwrap().insert(d.to_string(), now);
                changed = true;
            }
            "fdec" => { self.d.lock().unwrap().raus.retain(|f| id_of(f) != d); changed = true; }
            "fcan" => { self.d.lock().unwrap().rein.retain(|f| id_of(f) != d); changed = true; }
            "fdel" => { self.d.lock().unwrap().freunde.retain(|f| id_of(f) != d); changed = true; }
            "hi" => {
                let mut dd = self.d.lock().unwrap();
                if let Some(f) = dd.freunde.iter_mut().find(|f| id_of(f) == d) {
                    if f["n"] != json!(n) { f["n"] = json!(n); changed = true; }
                    self.seen.lock().unwrap().insert(d.to_string(), now);
                }
            }
            "inv" => {
                let code = o["code"].as_str().unwrap_or("").to_string();
                if code.is_empty() || now.saturating_sub(t) > INV_TTL { return; }
                let mut ii = self.inv_in.lock().unwrap();
                ii.retain(|i| id_of(i) != d);
                ii.push(json!({"id": d, "n": n, "code": code, "ln": take_chars(o["ln"].as_str().unwrap_or("Kolonne"), 30), "t": t.min(now)}));
                log(format!("Einladung von {n}"));
            }
            "invx" => { let code = o["code"].clone(); self.inv_in.lock().unwrap().retain(|i| !(id_of(i) == d && (code.is_null() || i["code"] == code))); }
            _ => {}
        }
        if changed { self.save(); }
    }

    // ---------------- Freundschaften ----------------
    pub fn anfrage(&self, id: &str, n: &str, my_name: &str) -> bool {
        if !valid_id(id) || id == self.dev { return false; }
        {
            let mut dd = self.d.lock().unwrap();
            if dd.freunde.iter().any(|f| id_of(f) == id) { return true; }
            if dd.rein.iter().any(|f| id_of(f) == id) { drop(dd); return self.annehmen(id, my_name); }
            dd.raus.retain(|f| id_of(f) != id);
            dd.raus.push(json!({"id": id, "n": take_chars(n, 24), "t": now_ms()}));
        }
        self.save();
        self.send(id, "f", &json!({"y": "freq", "d": self.dev, "n": my_name, "t": now_ms()}), true)
    }

    pub fn annehmen(&self, id: &str, my_name: &str) -> bool {
        {
            let mut dd = self.d.lock().unwrap();
            let Some(r) = dd.rein.iter().find(|f| id_of(f) == id).cloned() else { return false };
            dd.rein.retain(|f| id_of(f) != id);
            if !dd.freunde.iter().any(|f| id_of(f) == id) { dd.freunde.push(json!({"id": id, "n": r["n"], "t": now_ms()})); }
        }
        self.save();
        self.seen.lock().unwrap().insert(id.to_string(), now_ms());
        self.send(id, "f", &json!({"y": "facc", "d": self.dev, "n": my_name, "t": now_ms()}), true)
    }

    pub fn ablehnen(&self, id: &str) -> bool {
        self.d.lock().unwrap().rein.retain(|f| id_of(f) != id);
        self.save();
        self.send(id, "f", &json!({"y": "fdec", "d": self.dev, "t": now_ms()}), true)
    }

    pub fn zurueckziehen(&self, id: &str) -> bool {
        self.d.lock().unwrap().raus.retain(|f| id_of(f) != id);
        self.save();
        self.send(id, "f", &json!({"y": "fcan", "d": self.dev, "t": now_ms()}), true)
    }

    pub fn entfernen(&self, id: &str) -> bool {
        self.d.lock().unwrap().freunde.retain(|f| id_of(f) != id);
        self.save();
        self.send(id, "f", &json!({"y": "fdel", "d": self.dev, "t": now_ms()}), true)
    }

    // ---------------- Einladungen ----------------
    pub fn einladen(&self, id: &str, code: &str, ln: &str, my_name: &str) -> bool {
        if code.is_empty() { return false; }
        let now = now_ms();
        {
            let mut io = self.inv_out.lock().unwrap();
            io.retain(|i| !(id_of(i) == id && i["code"] == json!(code)));
            io.push(json!({"id": id, "code": code, "t": now}));
        }
        self.send(id, "i", &json!({"y": "inv", "d": self.dev, "n": my_name, "code": code, "ln": ln, "t": now}), true)
    }

    // Kolonne geschlossen/verlassen -> offene Einladungen dafür zurücknehmen
    pub fn einladungen_zurueck(&self, code_norm: &str) {
        let weg: Vec<Value> = {
            let mut io = self.inv_out.lock().unwrap();
            let w: Vec<Value> = io.iter().filter(|i| crate::util::norm_code(i["code"].as_str().unwrap_or("")) == code_norm).cloned().collect();
            io.retain(|i| crate::util::norm_code(i["code"].as_str().unwrap_or("")) != code_norm);
            w
        };
        for i in weg { self.send(id_of(&i), "i", &json!({"y": "invx", "d": self.dev, "code": i["code"], "t": now_ms()}), true); }
    }

    // Einladung annehmen (liefert den Code zum Beitreten) oder ablehnen
    pub fn einladung(&self, id: &str, an: bool) -> Option<String> {
        let inv = {
            let mut ii = self.inv_in.lock().unwrap();
            let i = ii.iter().find(|i| id_of(i) == id).cloned();
            ii.retain(|i| id_of(i) != id);
            i
        };
        let inv = inv?;
        if !an || now_ms().saturating_sub(inv["t"].as_u64().unwrap_or(0)) > INV_TTL { return None; }
        inv["code"].as_str().map(String::from)
    }

    // ---------------- Online-Modus ----------------
    pub fn set_welt(self: &Arc<Self>, on: bool) {
        let mut w = self.welt.lock().unwrap();
        if on && w.is_none() {
            let me: Weak<Sozial> = Arc::downgrade(self);
            *w = Some(Kanal::new(&self.net, "welt", WELT, Arc::new(move |o: Value, _sub: &str| {
                if let Some(s) = me.upgrade() { s.on_welt(o); }
            })));
        } else if !on {
            if let Some(k) = w.take() { k.send(&json!({"y": "bye", "d": self.dev})); }
            self.spieler.lock().unwrap().clear();
        }
    }

    fn on_welt(&self, o: Value) {
        let d = o["d"].as_str().unwrap_or("");
        if !valid_id(d) || d == self.dev { return; }
        match o["y"].as_str().unwrap_or("") {
            "pos" => {
                let num = |k: &str| o[k].as_f64().filter(|f| f.is_finite()).unwrap_or(0.0);
                let (Some(x), Some(z)) = (o["x"].as_f64(), o["z"].as_f64()) else { return };
                self.spieler.lock().unwrap().insert(d.to_string(), Friend { n: take_chars(o["n"].as_str().unwrap_or("Fahrer"), 24), x, z, h: num("h"), v: num("v"), c: num("c") as i64, g: if num("g") != 0.0 { 1 } else { 0 }, seen: now_ms(), b: crate::state::blink_of(&o) });
            }
            "bye" => { self.spieler.lock().unwrap().remove(d); }
            _ => {}
        }
    }

    pub fn welt_an(&self) -> bool { self.welt.lock().unwrap().is_some() }

    // alle 250 ms: Position in den Online-Kanal, Lebenszeichen an Freunde, Abgelaufenes wegräumen
    pub fn tick(&self, p: Option<MyPos>, name: &str, on: bool) {
        let now = now_ms();
        *self.me_n.lock().unwrap() = name.to_string();
        self.spieler.lock().unwrap().retain(|_, f| now.saturating_sub(f.seen) <= 12000);
        self.inv_in.lock().unwrap().retain(|i| now.saturating_sub(i["t"].as_u64().unwrap_or(0)) <= INV_TTL);
        self.inv_out.lock().unwrap().retain(|i| now.saturating_sub(i["t"].as_u64().unwrap_or(0)) <= INV_TTL);
        if !on || !self.net.ok() { return; }
        {
            let mut lh = self.last_hi.lock().unwrap();
            if now.saturating_sub(*lh) > 30_000 {
                *lh = now;
                let ids: Vec<String> = self.d.lock().unwrap().freunde.iter().map(|f| id_of(f).to_string()).collect();
                for id in ids { self.send(&id, "", &json!({"y": "hi", "d": self.dev, "n": name, "t": now}), false); }
            }
        }
        let w = self.welt.lock().unwrap();
        let Some(k) = w.as_ref() else { return };
        let fresh = p.is_some_and(|p| now.saturating_sub(p.t) < 3000);
        {
            let mut lp = self.last_pub.lock().unwrap();
            if !fresh && now.saturating_sub(*lp) < 5000 { return; }
            *lp = now;
        }
        let r = |v: f64, k: f64| (v * k).round() / k;
        let m = match p {
            Some(p) if fresh => { let mut m = json!({"y": "pos", "d": self.dev, "n": name, "x": r(p.x, 10.0), "z": r(p.z, 10.0), "h": r(p.h, 1000.0), "v": r(p.v, 10.0), "c": p.c, "g": 1}); let b = crate::state::blink_now(); if !b.is_empty() { m["b"] = json!(b); } m }
            _ => json!({"y": "hb", "d": self.dev, "n": name}),
        };
        k.send(&m);
    }

    // Online-Spieler für die Karte (gleiches Format wie die Freunde aus den Kolonnen, dazu w = 1)
    pub fn welt_list(&self) -> Vec<Value> {
        let now = now_ms();
        self.spieler.lock().unwrap().iter().map(|(id, f)| json!({"id": id, "n": f.n, "x": f.x, "z": f.z, "h": f.h, "v": f.v, "c": f.c, "g": f.g, "b": f.b, "age": now.saturating_sub(f.seen), "w": 1})).collect()
    }

    pub fn state(&self) -> Value {
        let now = now_ms();
        let seen = self.seen.lock().unwrap();
        let d = self.d.lock().unwrap();
        let freunde: Vec<Value> = d.freunde.iter().map(|f| {
            let s = seen.get(id_of(f)).copied().unwrap_or(0);
            json!({"id": f["id"], "n": f["n"], "on": now.saturating_sub(s) < 75_000, "seen": s})
        }).collect();
        let inv: Vec<Value> = self.inv_in.lock().unwrap().iter().map(|i| { let mut i = i.clone(); i["rest"] = json!(INV_TTL.saturating_sub(now.saturating_sub(i["t"].as_u64().unwrap_or(0))) / 1000); i }).collect();
        let inv_out: Vec<Value> = self.inv_out.lock().unwrap().iter().map(|i| json!({"id": i["id"], "code": i["code"]})).collect();
        json!({"freunde": freunde, "rein": d.rein, "raus": d.raus, "inv": inv, "invOut": inv_out, "welt": self.welt.lock().unwrap().is_some(), "me": self.dev})
    }
}
