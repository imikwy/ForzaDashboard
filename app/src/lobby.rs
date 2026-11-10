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
    // per Abstimmung oder vom Kolonnenführer entfernte Geräte (ihre Nachrichten werden ignoriert)
    pub kick: Vec<String>,
}

const CHAT_MAX: usize = 80;
// Ein-Klick-Emotes im Kolonnen-Chat (das Dashboard zeigt sie als Symbol)
const EMOTES: [&str; 10] = ["gg", "lol", "top", "wait", "go", "brb", "fire", "wow", "sorry", "thx"];

struct LFriend {
    f: Friend,
    lobbys: BTreeSet<String>,
    pos: bool,
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
    // Gruppen-Wegpunkt je Lobby (der zuletzt gesetzte gilt; bleibt beim Vermittler liegen für später Kommende)
    gwp: Mutex<HashMap<String, Value>>,
    // Kolonnen-Chat (alle offenen Kolonnen, neueste hinten) und Kick-Abstimmungen je Kolonne: Ziel -> Wähler
    chat: Mutex<Vec<Value>>,
    votes: Mutex<HashMap<String, HashMap<String, BTreeSet<String>>>>,
    owner_dev: Mutex<HashMap<String, String>>,
    // wurde ich selbst entfernt? -> im nächsten Takt die Kolonne verlassen (nicht im Empfangs-Takt des Kanals)
    kicked_me: Mutex<Vec<String>>,
    last_chat: Mutex<u64>,
}

impl Lobbys {
    pub fn new(net: &Arc<Net>, dev: &str, root: &std::path::Path, on_roads: OnRoads) -> Arc<Lobbys> {
        let file = root.join("lobbys.json");
        let liste: Vec<Lobby> = read_json(&file).and_then(|v| serde_json::from_value::<Vec<Lobby>>(v["liste"].clone()).ok()).unwrap_or_default()
            .into_iter().filter(|l| valid_code(&l.code)).collect();
        Arc::new(Lobbys {
            net: net.clone(), dev: dev.to_string(), file, liste: Mutex::new(liste), kanaele: Mutex::new(HashMap::new()),
            friends: Mutex::new(HashMap::new()), last_pub: Mutex::new(0), lr_sent: Mutex::new(HashMap::new()), on_roads, gwp: Mutex::new(HashMap::new()),
            chat: Mutex::new(Vec::new()), votes: Mutex::new(HashMap::new()), owner_dev: Mutex::new(HashMap::new()), kicked_me: Mutex::new(Vec::new()), last_chat: Mutex::new(0),
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
        if d.is_empty() { return; }
        // Gruppen-Wegpunkt auch vom eigenen Gerät annehmen (liegt nach einem Neustart beim Vermittler)
        if o["y"] == "wp" { self.on_wp(code, &o); return; }
        if d == self.dev { return; }
        if self.is_kicked(code, d) { return; }
        match o["y"].as_str().unwrap_or("") {
            "pos" => {
                // ohne Koordinaten: online, aber gerade nicht im Spiel unterwegs (nur in der Lobby-Liste, nicht auf der Karte)
                let (x, z) = (o["x"].as_f64(), o["z"].as_f64());
                let num = |k: &str| o[k].as_f64().filter(|f| f.is_finite()).unwrap_or(0.0);
                let f = Friend { n: take_chars(o["n"].as_str().unwrap_or("Freund"), 24), x: x.unwrap_or(0.0), z: z.unwrap_or(0.0), h: num("h"), v: num("v"), c: num("c") as i64, g: if num("g") != 0.0 { 1 } else { 0 }, seen: now_ms(), b: crate::state::blink_of(&o), pid: crate::state::pid_of(&o) };
                let mut fr = self.friends.lock().unwrap();
                let e = fr.entry(d.to_string()).or_insert_with(|| LFriend { f: f.clone(), lobbys: BTreeSet::new(), pos: false });
                e.f = f;
                e.pos = x.is_some() && z.is_some();
                e.lobbys.insert(code.to_string());
                drop(fr);
                if o["own"].as_u64() == Some(1) { self.owner_dev.lock().unwrap().insert(code.to_string(), d.to_string()); }
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
            "chat" => {
                let m = clean_text(o["m"].as_str().unwrap_or(""), 160);
                let e = o["e"].as_str().filter(|e| EMOTES.contains(e)).unwrap_or("");
                if m.is_empty() && e.is_empty() { return; }
                let n = take_chars(o["n"].as_str().unwrap_or("Freund"), 24);
                self.push_chat(json!({"l": code, "d": d, "n": n, "pid": crate::state::pid_of(&o), "m": m, "e": e, "t": now_ms()}));
            }
            "vote" => {
                let Some(t) = o["t"].as_str().filter(|t| t.len() <= 40) else { return };
                let on = o["on"].as_bool().unwrap_or(true);
                {
                    let mut v = self.votes.lock().unwrap();
                    let set = v.entry(code.to_string()).or_default().entry(t.to_string()).or_default();
                    if on { set.insert(d.to_string()); } else { set.remove(d); }
                }
                self.check_votes(code, t);
            }
            "kick" => {
                let Some(t) = o["t"].as_str() else { return };
                // gilt sofort, wenn es der Kolonnenführer schickt – sonst nur, wenn die Stimmen hier auch reichen
                if self.leader(code).as_deref() == Some(d) { self.apply_kick(code, t, "Kolonnenführer"); }
                else { self.check_votes(code, t); }
            }
            "lr" if sub.starts_with("lr/") => { if o["roads"].is_array() { (self.on_roads)(d, &o); } }
            _ => {}
        }
    }

    fn on_wp(&self, code: &str, o: &Value) {
        let t = o["t"].as_u64().unwrap_or(0);
        let mut g = self.gwp.lock().unwrap();
        if g.get(code).and_then(|p| p["t"].as_u64()).is_some_and(|alt| alt > t) { return; }
        if o["clear"].as_u64() == Some(1) { g.insert(code.to_string(), json!({"t": t, "clear": 1})); return; }
        let (Some(x), Some(z)) = (o["x"].as_f64().filter(|v| v.is_finite()), o["z"].as_f64().filter(|v| v.is_finite())) else { return };
        g.insert(code.to_string(), json!({"t": t, "d": o["d"], "n": take_chars(o["n"].as_str().unwrap_or("Freund"), 24), "x": x, "z": z, "wn": take_chars(o["wn"].as_str().unwrap_or(""), 40)}));
    }

    // Gruppen-Wegpunkt an alle offenen Lobbys (bleibt liegen, damit auch später Kommende ihn sehen)
    pub fn send_wp(&self, b: &Value, name: &str) -> bool {
        let (Some(x), Some(z)) = (b["x"].as_f64().filter(|v| v.is_finite()), b["z"].as_f64().filter(|v| v.is_finite())) else { return false };
        let m = json!({"y": "wp", "d": self.dev, "n": name, "x": (x * 10.0).round() / 10.0, "z": (z * 10.0).round() / 10.0, "wn": take_chars(b["n"].as_str().unwrap_or(""), 40), "t": now_ms()});
        self.wp_all(&m)
    }

    pub fn clear_wp(&self) -> bool {
        self.wp_all(&json!({"y": "wp", "d": self.dev, "clear": 1, "t": now_ms()}))
    }

    fn wp_all(&self, m: &Value) -> bool {
        let codes: Vec<String> = self.kanaele.lock().unwrap().keys().cloned().collect();
        if codes.is_empty() { return false; }
        for c in &codes { self.on_wp(c, m); }
        let ks = self.kanaele.lock().unwrap();
        for k in ks.values() { k.send_to(m, "wp", true); }
        true
    }

    // ---------------- Kolonnen-Chat und Kick-Abstimmung ----------------
    fn push_chat(&self, m: Value) {
        let mut c = self.chat.lock().unwrap();
        c.push(m);
        let n = c.len();
        if n > CHAT_MAX { c.drain(..n - CHAT_MAX); }
    }

    fn is_kicked(&self, code: &str, d: &str) -> bool {
        self.liste.lock().unwrap().iter().any(|l| l.code == code && l.kick.iter().any(|k| k == d))
    }

    // wer gerade in der Kolonne ist (ohne mich): (Gerät, Name, Profil-Id)
    fn members(&self, code: &str) -> Vec<(String, String, String)> {
        let kicked: Vec<String> = self.liste.lock().unwrap().iter().find(|l| l.code == code).map(|l| l.kick.clone()).unwrap_or_default();
        let mut m: Vec<(String, String, String)> = self.friends.lock().unwrap().iter()
            .filter(|(id, f)| f.lobbys.contains(code) && !kicked.contains(id))
            .map(|(id, f)| (id.clone(), f.f.n.clone(), f.f.pid.clone())).collect();
        m.sort();
        m
    }

    // Kolonnenführer: wer die Kolonne erstellt hat; ist der nicht (mehr) da, der mit der kleinsten Geräte-Id
    // (alle rechnen gleich, also sind sich alle einig, ohne zu verhandeln)
    pub fn leader(&self, code: &str) -> Option<String> {
        let mine = self.liste.lock().unwrap().iter().any(|l| l.code == code && l.owner);
        if mine { return Some(self.dev.clone()); }
        let mem = self.members(code);
        if let Some(o) = self.owner_dev.lock().unwrap().get(code) {
            if mem.iter().any(|(d, _, _)| d == o) { return Some(o.clone()); }
        }
        let mut ids: Vec<String> = mem.into_iter().map(|(d, _, _)| d).collect();
        ids.push(self.dev.clone());
        ids.into_iter().min()
    }

    // (Wähler, nötige Stimmen): mehr als die Hälfte aller anderen (ohne das Ziel), mindestens 2
    fn need(&self, code: &str, target: &str) -> (usize, usize) {
        let voters = self.members(code).iter().filter(|(d, _, _)| d != target).count() + if target == self.dev { 0 } else { 1 };
        (voters, (voters / 2 + 1).max(2))
    }

    fn check_votes(&self, code: &str, t: &str) {
        let online: BTreeSet<String> = self.members(code).into_iter().map(|(d, _, _)| d).chain(std::iter::once(self.dev.clone())).collect();
        let n = self.votes.lock().unwrap().get(code).and_then(|v| v.get(t)).map(|s| s.iter().filter(|d| online.contains(*d) && d.as_str() != t).count()).unwrap_or(0);
        let (voters, need) = self.need(code, t);
        if voters >= 2 && n >= need { self.apply_kick(code, t, "Abstimmung"); }
    }

    fn apply_kick(&self, code: &str, t: &str, by: &str) {
        if let Some(v) = self.votes.lock().unwrap().get_mut(code) { v.remove(t); }
        if t == self.dev {
            self.kicked_me.lock().unwrap().push(code.to_string());
            self.push_chat(json!({"l": code, "sys": 1, "m": format!("Du wurdest aus der Kolonne entfernt ({by})"), "t": now_ms(), "me": 1}));
            return;
        }
        let name = self.friends.lock().unwrap().get(t).map(|f| f.f.n.clone()).unwrap_or_else(|| "Ein Fahrer".into());
        {
            let mut ls = self.liste.lock().unwrap();
            let Some(l) = ls.iter_mut().find(|l| l.code == code) else { return };
            if l.kick.iter().any(|k| k == t) { return; }
            l.kick.push(t.to_string());
            if l.kick.len() > 200 { l.kick.remove(0); }
        }
        self.save();
        let mut fr = self.friends.lock().unwrap();
        if let Some(f) = fr.get_mut(t) { f.lobbys.remove(code); }
        fr.retain(|_, f| !f.lobbys.is_empty());
        drop(fr);
        self.push_chat(json!({"l": code, "sys": 1, "m": format!("{name} wurde aus der Kolonne entfernt ({by})"), "t": now_ms()}));
    }

    fn first_open(&self, code: &str) -> Option<String> {
        let ks = self.kanaele.lock().unwrap();
        if !code.is_empty() { let c = norm_code(code); return ks.contains_key(&c).then_some(c); }
        let ls = self.liste.lock().unwrap();
        ls.iter().find(|l| l.an && ks.contains_key(&l.code)).map(|l| l.code.clone())
    }

    pub fn send_chat(&self, b: &Value, name: &str) -> Value {
        let Some(code) = self.first_open(b["code"].as_str().unwrap_or("")) else { return json!({"ok": false, "why": "Keine Kolonne offen"}) };
        let m = clean_text(b["m"].as_str().unwrap_or(""), 160);
        let e = b["e"].as_str().filter(|e| EMOTES.contains(e)).unwrap_or("");
        if m.is_empty() && e.is_empty() { return json!({"ok": false}); }
        {
            let mut lc = self.last_chat.lock().unwrap();
            if now_ms().saturating_sub(*lc) < 600 { return json!({"ok": false, "why": "Etwas langsamer"}); }
            *lc = now_ms();
        }
        let pid = crate::profil::my_id().unwrap_or_default();
        let msg = json!({"y": "chat", "d": self.dev, "n": name, "pid": pid, "m": m, "e": e});
        if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send(&msg); }
        self.push_chat(json!({"l": code, "d": self.dev, "n": name, "pid": pid, "m": m, "e": e, "t": now_ms(), "me": 1}));
        json!({"ok": true})
    }

    // Stimme abgeben (oder zurücknehmen); der Kolonnenführer entfernt direkt
    pub fn vote(&self, b: &Value, name: &str) -> Value {
        let Some(code) = self.first_open(b["code"].as_str().unwrap_or("")) else { return json!({"ok": false, "why": "Keine Kolonne offen"}) };
        let Some(t) = b["t"].as_str().filter(|t| !t.is_empty() && *t != self.dev) else { return json!({"ok": false}) };
        if !self.members(&code).iter().any(|(d, _, _)| d == t) { return json!({"ok": false, "why": "Der Fahrer ist nicht mehr in der Kolonne"}); }
        let on = b["on"].as_bool().unwrap_or(true);
        let lead = self.leader(&code).as_deref() == Some(self.dev.as_str());
        if lead && on {
            if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send(&json!({"y": "kick", "d": self.dev, "n": name, "t": t})); }
            self.apply_kick(&code, t, "Kolonnenführer");
            return json!({"ok": true, "kicked": true});
        }
        if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send(&json!({"y": "vote", "d": self.dev, "t": t, "on": on})); }
        {
            let mut v = self.votes.lock().unwrap();
            let set = v.entry(code.clone()).or_default().entry(t.to_string()).or_default();
            if on { set.insert(self.dev.clone()); } else { set.remove(&self.dev); }
        }
        let before = self.is_kicked(&code, t);
        self.check_votes(&code, t);
        let kicked = !before && self.is_kicked(&code, t);
        // reicht meine Stimme, sagt es allen (wer eine Stimme verpasst hat, zählt selbst nach)
        if kicked { if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send(&json!({"y": "kick", "d": self.dev, "n": name, "t": t, "v": 1})); } }
        json!({"ok": true, "kicked": kicked})
    }

    // für das Dashboard: Chat, Mitglieder, Führer, laufende Abstimmungen
    pub fn kol_state(&self) -> Value {
        let open: Vec<String> = {
            let ks = self.kanaele.lock().unwrap();
            self.liste.lock().unwrap().iter().filter(|l| l.an && ks.contains_key(&l.code)).map(|l| l.code.clone()).collect()
        };
        let kol: Vec<Value> = open.iter().map(|c| {
            let mem = self.members(c);
            let online: BTreeSet<String> = mem.iter().map(|(d, _, _)| d.clone()).chain(std::iter::once(self.dev.clone())).collect();
            let mut votes = serde_json::Map::new();
            if let Some(v) = self.votes.lock().unwrap().get(c) {
                for (t, s) in v {
                    let n = s.iter().filter(|d| online.contains(*d) && *d != t).count();
                    if n == 0 || !online.contains(t) { continue; }
                    votes.insert(t.clone(), json!({"n": n, "mine": s.contains(&self.dev)}));
                }
            }
            for (t, v) in votes.iter_mut() { v["need"] = json!(self.need(c, t).1); }
            json!({"code": fmt_code(c), "leader": self.leader(c), "me": self.dev,
                "mem": mem.iter().map(|(d, n, p)| json!({"d": d, "n": n, "pid": p})).collect::<Vec<_>>(), "votes": votes})
        }).collect();
        let chat: Vec<Value> = self.chat.lock().unwrap().iter()
            .filter(|m| (m["me"] == 1 && m["sys"] == 1) || open.iter().any(|c| m["l"] == c.as_str())).cloned().collect();
        json!({"kol": kol, "chat": chat})
    }

    // aktuelle Gruppen-Wegpunkte der offenen Lobbys (höchstens 6 Stunden alt)
    pub fn group_wps(&self) -> Vec<Value> {
        let now = now_ms();
        let open: Vec<String> = self.kanaele.lock().unwrap().keys().cloned().collect();
        let names: HashMap<String, String> = self.liste.lock().unwrap().iter().map(|l| (l.code.clone(), l.name.clone())).collect();
        self.gwp.lock().unwrap().iter()
            .filter(|(c, w)| open.contains(c) && w["clear"].is_null() && now.saturating_sub(w["t"].as_u64().unwrap_or(0)) < 6 * 3600 * 1000)
            .map(|(c, w)| { let mut w = w.clone(); w["ln"] = json!(names.get(c).cloned().unwrap_or_default()); w["me"] = json!(w["d"] == self.dev); w })
            .collect()
    }

    // eigene Position: 4×/s beim Fahren, sonst alle 5 s ein Lebenszeichen
    pub fn pub_pos(&self, p: Option<MyPos>, name: &str, on: bool) {
        let now = now_ms();
        // wurde ich entfernt? Dann die Kolonne hier verlassen
        let gone: Vec<String> = std::mem::take(&mut *self.kicked_me.lock().unwrap());
        for c in gone { self.leave(&c); }
        self.friends.lock().unwrap().retain(|_, f| now.saturating_sub(f.f.seen) <= 12000);
        if !on || !self.net.ok() || self.kanaele.lock().unwrap().is_empty() { return; }
        let fresh = p.is_some_and(|p| now.saturating_sub(p.t) < 3000);
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
            let mut m = match p {
                Some(p) => json!({"y": "pos", "d": self.dev, "n": name, "x": r(p.x, 10.0), "z": r(p.z, 10.0), "h": r(p.h, 1000.0), "v": r(p.v, 10.0), "c": p.c, "g": if fresh { 1 } else { 0 }}),
                None => json!({"y": "pos", "d": self.dev, "n": name}),
            };
            if l.owner { m["ln"] = json!(l.name); m["own"] = json!(1); }
            if fresh { let b = crate::state::blink_now(); if !b.is_empty() { m["b"] = json!(b); } }
            if let Some(pid) = crate::profil::my_id() { m["pid"] = json!(pid); }
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
        let l = Lobby { code: code.clone(), name: if name.is_empty() { format!("Lobby von {my_name}") } else { name }, owner: true, an: true, created: now_ms(), kick: vec![] };
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
                None => ls.push(Lobby { code: code.clone(), name: format!("Lobby {}", fmt_code(&code)), owner: false, an: true, created: now_ms(), kick: vec![] }),
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

    // Name und normalisierter Code einer Lobby (für Einladungen)
    pub fn find(&self, code: &str) -> Option<(String, String)> {
        let c = norm_code(code);
        self.liste.lock().unwrap().iter().find(|l| l.code == c).map(|l| (l.name.clone(), l.code.clone()))
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
        self.friends.lock().unwrap().iter().filter(|(_, f)| f.pos).map(|(id, f)| {
            let f = &f.f;
            json!({"id": id, "n": f.n, "x": f.x, "z": f.z, "h": f.h, "v": f.v, "c": f.c, "g": f.g, "b": f.b, "pid": f.pid, "age": now.saturating_sub(f.seen)})
        }).collect()
    }
}

// Chat-Text: keine Steuerzeichen, Leerraum zusammengefasst, begrenzt
fn clean_text(s: &str, n: usize) -> String {
    let t: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    take_chars(t.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), n)
}
