// Spiel-Lobbys: Freunde live auf der Karte – einfach und sicher.
// - Eine Lobby wird EINMAL erstellt (oder per Code beigetreten) und bleibt gespeichert (lobbys.json).
//   Danach reicht: Spiel starten, Dashboard starten, losfahren – die Verbindung kommt von selbst.
// - Geteilt werden nur Name, Position, Tempo und die selbst gelernten Straßen, verschlüsselt mit dem Lobby-Code.
//   Im Spielmodus kann niemand die Karte verändern – das geht nur in einer Bearbeiten-Sitzung.
use crate::kanal::{seal, Kanal};
use crate::mqtt::Net;
use crate::state::{Friend, MyPos};
use crate::util::{fmt_code, log, new_code, norm_code, now_ms, read_json, take_chars, valid_code, write_json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

const LR_MAX: usize = 180_000;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Lobby {
    pub code: String,
    pub name: String,
    pub owner: bool,
    pub an: bool,
    pub created: u64,
}

struct LFriend {
    f: Friend,
    lobbys: BTreeSet<String>,
}

pub type OnRoads = Arc<dyn Fn(&str, &Value) + Send + Sync>;

pub struct Lobbys {
    net: Arc<Net>,
    dev: String,
    file: PathBuf,
    liste: Mutex<Vec<Lobby>>,
    kanaele: Mutex<HashMap<String, Kanal>>,
    friends: Mutex<HashMap<String, LFriend>>,
    last_pub: Mutex<u64>,
    lr_sent: Mutex<HashMap<String, String>>,
    on_roads: OnRoads,
}

impl Lobbys {
    pub fn new(net: &Arc<Net>, dev: &str, root: &std::path::Path, on_roads: OnRoads) -> Arc<Lobbys> {
        let file = root.join("lobbys.json");
        let liste: Vec<Lobby> = read_json(&file).and_then(|v| serde_json::from_value::<Vec<Lobby>>(v["liste"].clone()).ok()).unwrap_or_default()
            .into_iter().filter(|l| valid_code(&l.code)).collect();
        Arc::new(Lobbys {
            net: net.clone(), dev: dev.to_string(), file, liste: Mutex::new(liste), kanaele: Mutex::new(HashMap::new()),
            friends: Mutex::new(HashMap::new()), last_pub: Mutex::new(0), lr_sent: Mutex::new(HashMap::new()), on_roads,
        })
    }

    fn save(&self) {
        write_json(&self.file, &json!({"liste": *self.liste.lock().unwrap()}));
    }

    pub fn start(self: &Arc<Self>) {
        let ls: Vec<Lobby> = self.liste.lock().unwrap().iter().filter(|l| l.an).cloned().collect();
        for l in ls { self.open(&l.code); }
    }

    fn open(self: &Arc<Self>, code: &str) {
        if self.kanaele.lock().unwrap().contains_key(code) { return; }
        let w: Weak<Lobbys> = Arc::downgrade(self);
        let c = code.to_string();
        let k = Kanal::new(&self.net, "play", code, Arc::new(move |o: Value, sub: &str| {
            if let Some(l) = w.upgrade() { l.on_msg(&c, o, sub); }
        }));
        self.kanaele.lock().unwrap().insert(code.to_string(), k);
        self.lr_sent.lock().unwrap().remove(code);
    }

    fn close(&self, code: &str, bye: bool) {
        let Some(k) = self.kanaele.lock().unwrap().remove(code) else { return };
        if bye { k.send(&json!({"y": "bye", "d": self.dev})); }
        drop(k);
        let mut fr = self.friends.lock().unwrap();
        for f in fr.values_mut() { f.lobbys.remove(code); }
        fr.retain(|_, f| !f.lobbys.is_empty());
    }

    fn on_msg(&self, code: &str, o: Value, sub: &str) {
        let d = o["d"].as_str().unwrap_or("");
        if d.is_empty() || d == self.dev { return; }
        match o["y"].as_str().unwrap_or("") {
            "pos" => {
                let (Some(x), Some(z)) = (o["x"].as_f64(), o["z"].as_f64()) else { return };
                let num = |k: &str| o[k].as_f64().filter(|f| f.is_finite()).unwrap_or(0.0);
                let f = Friend { n: take_chars(o["n"].as_str().unwrap_or("Freund"), 24), x, z, h: num("h"), v: num("v"), c: num("c") as i64, g: if num("g") != 0.0 { 1 } else { 0 }, seen: now_ms() };
                let mut fr = self.friends.lock().unwrap();
                let e = fr.entry(d.to_string()).or_insert_with(|| LFriend { f: f.clone(), lobbys: BTreeSet::new() });
                e.f = f;
                e.lobbys.insert(code.to_string());
                drop(fr);
                // Lobby-Name vom Ersteller übernehmen (wer beitritt, kennt erst nur den Code)
                if let (Some(ln), true) = (o["ln"].as_str(), o["own"].as_u64() == Some(1)) {
                    let mut ls = self.liste.lock().unwrap();
                    if let Some(l) = ls.iter_mut().find(|l| l.code == code && !l.owner && l.name != ln) {
                        l.name = take_chars(ln, 30);
                        drop(ls);
                        self.save();
                    }
                }
            }
            "bye" => { self.friends.lock().unwrap().remove(d); }
            "lr" if sub.starts_with("lr/") => { if o["roads"].is_array() { (self.on_roads)(d, &o); } }
            _ => {}
        }
    }

    // eigene Position: 2×/s beim Fahren, sonst alle 5 s ein Lebenszeichen
    pub fn pub_pos(&self, p: Option<MyPos>, name: &str, on: bool) {
        let now = now_ms();
        self.friends.lock().unwrap().retain(|_, f| now.saturating_sub(f.f.seen) <= 12000);
        let Some(p) = p else { return };
        if !on || !self.net.ok() || self.kanaele.lock().unwrap().is_empty() { return; }
        let fresh = now.saturating_sub(p.t) < 3000;
        {
            let mut lp = self.last_pub.lock().unwrap();
            if !fresh && now.saturating_sub(*lp) < 5000 { return; }
            *lp = now;
        }
        let r = |v: f64, k: f64| (v * k).round() / k;
        let ls = self.liste.lock().unwrap().clone();
        let ks = self.kanaele.lock().unwrap();
        for l in ls {
            let Some(k) = ks.get(&l.code) else { continue };
            let mut m = json!({"y": "pos", "d": self.dev, "n": name, "x": r(p.x, 10.0), "z": r(p.z, 10.0), "h": r(p.h, 1000.0), "v": r(p.v, 10.0), "c": p.c, "g": if fresh { 1 } else { 0 }});
            if l.owner { m["ln"] = json!(l.name); m["own"] = json!(1); }
            k.send(&m);
        }
    }

    // gelernte Straßen mit der Lobby teilen (bleibt beim Vermittler liegen, damit später Kommende sie auch bekommen)
    pub fn pub_roads(&self, learned: &Value, lver: u64, name: &str, on: bool, force: bool) {
        if !on || !self.net.ok() { return; }
        let Some(roads) = learned["roads"].as_array() else { return };
        let long = |v: &Value| v["p"].as_array().is_some_and(|p| p.len() >= 4);
        let roads: Vec<Value> = roads.iter().filter(|r| long(r) && r["s"].is_null()).map(|r| json!({"i": r["i"], "t": r["t"], "lv": r.get("lv").cloned().unwrap_or(json!(0)), "m": r["m"], "ts": r["ts"], "c": r["c"], "p": r["p"]})).collect();
        let mut cands: Vec<&Value> = learned["cands"].as_array().map(|c| c.iter().filter(|c| long(c)).collect()).unwrap_or_default();
        cands.sort_by(|a, b| b["ts"].as_f64().unwrap_or(0.0).partial_cmp(&a["ts"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        let cands: Vec<Value> = cands.into_iter().map(|c| json!({"p": c["p"], "k": c["k"]})).collect();
        let ks = self.kanaele.lock().unwrap();
        for (code, k) in ks.iter() {
            let sig = format!("{lver}|{code}");
            if !force && self.lr_sent.lock().unwrap().get(code) == Some(&sig) { continue; }
            let mut n = cands.len();
            let buf = loop {
                let msg = json!({"y": "lr", "d": self.dev, "n": name, "ts": now_ms(), "roads": roads, "cands": cands[..n]});
                let b = seal(&k.ch, &msg);
                if b.len() <= LR_MAX || n == 0 { break b; }
                n = n * 3 / 4;
            };
            self.net.publish(&format!("{}/lr/{}", k.topic(), self.dev), &buf, true);
            self.lr_sent.lock().unwrap().insert(code.clone(), sig);
        }
    }

    pub fn stop(&self) {
        let codes: Vec<String> = self.kanaele.lock().unwrap().keys().cloned().collect();
        for c in codes { self.close(&c, true); }
    }

    // ---------------- Verwaltung ----------------
    pub fn create(self: &Arc<Self>, name: &str, my_name: &str) -> Value {
        let code = new_code();
        let name = take_chars(name.trim(), 30);
        let l = Lobby { code: code.clone(), name: if name.is_empty() { format!("Lobby von {my_name}") } else { name }, owner: true, an: true, created: now_ms() };
        log(format!("Lobby erstellt: {} ({})", l.name, fmt_code(&code)));
        self.liste.lock().unwrap().push(l);
        self.save();
        self.open(&code);
        json!({"ok": true, "code": fmt_code(&code)})
    }

    pub fn join(self: &Arc<Self>, code: &str) -> Value {
        if !valid_code(code) { return json!({"ok": false, "why": "Der Code hat 9 Zeichen, z. B. K7M-2QX-9PA"}); }
        let code = norm_code(code);
        {
            let mut ls = self.liste.lock().unwrap();
            match ls.iter_mut().find(|l| l.code == code) {
                Some(l) => l.an = true,
                None => ls.push(Lobby { code: code.clone(), name: format!("Lobby {}", fmt_code(&code)), owner: false, an: true, created: now_ms() }),
            }
        }
        self.save();
        self.open(&code);
        json!({"ok": true})
    }

    pub fn leave(&self, code: &str) -> bool {
        let code = norm_code(code);
        if !self.liste.lock().unwrap().iter().any(|l| l.code == code) { return false; }
        self.close(&code, true);
        // eigene gespeicherte Straßen beim Vermittler entfernen
        let ch = crate::kanal::channel("play", &code);
        self.net.publish(&format!("{}/lr/{}", ch.topic, self.dev), &[], true);
        self.liste.lock().unwrap().retain(|l| l.code != code);
        self.save();
        true
    }

    pub fn set_on(self: &Arc<Self>, code: &str, on: bool) -> bool {
        let code = norm_code(code);
        {
            let mut ls = self.liste.lock().unwrap();
            let Some(l) = ls.iter_mut().find(|l| l.code == code) else { return false };
            l.an = on;
        }
        self.save();
        if on { self.open(&code); } else { self.close(&code, true); }
        true
    }

    pub fn rename(&self, code: &str, name: &str) -> bool {
        let code = norm_code(code);
        let name = take_chars(name.trim(), 30);
        if name.is_empty() { return false; }
        {
            let mut ls = self.liste.lock().unwrap();
            let Some(l) = ls.iter_mut().find(|l| l.code == code) else { return false };
            l.name = name;
        }
        self.save();
        true
    }

    pub fn names(&self) -> String {
        self.liste.lock().unwrap().iter().filter(|l| l.an).map(|l| l.name.clone()).collect::<Vec<_>>().join(", ")
    }

    pub fn list(&self) -> Value {
        let fr = self.friends.lock().unwrap();
        let ls = self.liste.lock().unwrap();
        json!(ls.iter().map(|l| json!({"code": fmt_code(&l.code), "name": l.name, "owner": l.owner, "an": l.an, "created": l.created,
            "online": fr.values().filter(|f| f.lobbys.contains(&l.code)).map(|f| f.f.n.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>())
    }

    pub fn friend_list(&self) -> Vec<Value> {
        let now = now_ms();
        self.friends.lock().unwrap().iter().map(|(id, f)| {
            let f = &f.f;
            json!({"id": id, "n": f.n, "x": f.x, "z": f.z, "h": f.h, "v": f.v, "c": f.c, "g": f.g, "age": now.saturating_sub(f.seen)})
        }).collect()
    }
}
