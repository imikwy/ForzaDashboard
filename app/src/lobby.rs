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
    // Kolonnenführer per Abstimmung oder Übergabe: Kolonne -> Ziel -> Wähler, und der gewählte Führer je Kolonne
    promo: Mutex<HashMap<String, HashMap<String, BTreeSet<String>>>>,
    lead_ov: Mutex<HashMap<String, String>>,
    // Event-Daten je Kolonne und Gerät (Sterne beim Tuning-Wettbewerb, Rolle und Festnahmen bei der Police Chase)
    ev: Mutex<HashMap<String, HashMap<String, Value>>>,
    // Police-Chase-Runde je Kolonne (vom Kolonnenführer gestartet/beendet, liegt beim Vermittler)
    pc: Mutex<HashMap<String, Value>>,
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
            promo: Mutex::new(HashMap::new()), lead_ov: Mutex::new(HashMap::new()), ev: Mutex::new(HashMap::new()), pc: Mutex::new(HashMap::new()),
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
        if o["y"] == "ev" && sub.strip_prefix("ev/") == Some(d) {
            if self.is_kicked(code, d) { return; }
            self.ev.lock().unwrap().entry(code.to_string()).or_default().insert(d.to_string(), clean_ev(&o));
            return;
        }
        // Police-Chase-Runde: gilt nur vom Kolonnenführer (wer gerade erst beitritt und noch niemanden kennt, nimmt die liegende Runde an)
        if o["y"] == "pc" && sub == "pc" {
            if self.is_kicked(code, d) { return; }
            if d != self.dev && !self.members(code).is_empty() && self.leader(code).as_deref() != Some(d) { return; }
            if let Some(p) = clean_pc(&o) { self.on_pc(code, p); }
            return;
        }
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
                // wer per Abstimmung/Übergabe führt, sagt es mit – später Gekommene übernehmen das
                if o["lo"].as_str() == Some(d) { self.lead_ov.lock().unwrap().entry(code.to_string()).or_insert_with(|| d.to_string()); }
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
            "pvote" => {
                let Some(t) = o["t"].as_str().filter(|t| t.len() <= 40) else { return };
                let on = o["on"].as_bool().unwrap_or(true);
                {
                    let mut v = self.promo.lock().unwrap();
                    let set = v.entry(code.to_string()).or_default().entry(t.to_string()).or_default();
                    if on { set.insert(d.to_string()); } else { set.remove(d); }
                }
                self.check_promo(code, t);
            }
            "lead" => {
                let Some(t) = o["t"].as_str().filter(|t| t.len() <= 40) else { return };
                if self.leader(code).as_deref() == Some(d) { self.set_lead(code, t, "übergeben"); } else { self.check_promo(code, t); }
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
        let mem = self.members(code);
        let ov = self.lead_ov.lock().unwrap().get(code).cloned();
        if let Some(t) = ov { if t == self.dev || mem.iter().any(|(d, _, _)| *d == t) { return Some(t); } }
        let mine = self.liste.lock().unwrap().iter().any(|l| l.code == code && l.owner);
        if mine { return Some(self.dev.clone()); }
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

    fn check_promo(&self, code: &str, t: &str) {
        let online: BTreeSet<String> = self.members(code).into_iter().map(|(d, _, _)| d).chain(std::iter::once(self.dev.clone())).collect();
        if !online.contains(t) { return; }
        let n = self.promo.lock().unwrap().get(code).and_then(|v| v.get(t)).map(|s| s.iter().filter(|d| online.contains(*d) && d.as_str() != t).count()).unwrap_or(0);
        let (voters, need) = self.need(code, t);
        if voters >= 2 && n >= need { self.set_lead(code, t, "Abstimmung"); }
    }

    fn set_lead(&self, code: &str, t: &str, by: &str) {
        if let Some(v) = self.promo.lock().unwrap().get_mut(code) { v.remove(t); }
        {
            let mut lo = self.lead_ov.lock().unwrap();
            if lo.get(code).map(|x| x == t).unwrap_or(false) { return; }
            lo.insert(code.to_string(), t.to_string());
        }
        let who = if t == self.dev { "Du bist".to_string() } else { format!("{} ist", self.friends.lock().unwrap().get(t).map(|f| f.f.n.clone()).unwrap_or_else(|| "Ein Fahrer".into())) };
        self.push_chat(json!({"l": code, "sys": 1, "m": format!("{who} jetzt Kolonnenführer ({by})"), "t": now_ms()}));
    }

    // Kolonnenführer wählen (oder Stimme zurück); der Führer übergibt direkt
    pub fn promote(&self, b: &Value, name: &str) -> Value {
        let Some(code) = self.first_open(b["code"].as_str().unwrap_or("")) else { return json!({"ok": false, "why": "Keine Kolonne offen"}) };
        let Some(t) = b["t"].as_str().filter(|t| !t.is_empty() && *t != self.dev) else { return json!({"ok": false}) };
        if !self.members(&code).iter().any(|(d, _, _)| d == t) { return json!({"ok": false, "why": "Der Fahrer ist nicht mehr in der Kolonne"}); }
        let on = b["on"].as_bool().unwrap_or(true);
        if on && self.leader(&code).as_deref() == Some(self.dev.as_str()) {
            if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send(&json!({"y": "lead", "d": self.dev, "n": name, "t": t})); }
            self.set_lead(&code, t, "übergeben");
            return json!({"ok": true, "lead": true});
        }
        if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send(&json!({"y": "pvote", "d": self.dev, "t": t, "on": on})); }
        {
            let mut v = self.promo.lock().unwrap();
            let set = v.entry(code.clone()).or_default().entry(t.to_string()).or_default();
            if on { set.insert(self.dev.clone()); } else { set.remove(&self.dev); }
        }
        let before = self.leader(&code).as_deref() == Some(t);
        self.check_promo(&code, t);
        let now = self.leader(&code).as_deref() == Some(t);
        if now && !before { if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send(&json!({"y": "lead", "d": self.dev, "n": name, "t": t, "v": 1})); } }
        json!({"ok": true, "lead": now && !before})
    }

    // eigene Event-Daten ändern (Sterne, Rolle, gefangen) und für alle hinterlegen
    pub fn ev_set(&self, b: &Value, name: &str) -> Value {
        let Some(code) = self.first_open(b["code"].as_str().unwrap_or("")) else { return json!({"ok": false, "why": "Keine Kolonne offen"}) };
        let mut st = self.ev.lock().unwrap().get(&code).and_then(|m| m.get(&self.dev)).cloned().unwrap_or_else(|| clean_ev(&json!({})));
        if let Some(t) = b["rate"].as_str().filter(|t| !t.is_empty() && t.len() <= 40 && *t != self.dev) {
            let s = b["s"].as_u64().unwrap_or(0).min(5);
            if let Some(r) = st["r"].as_object_mut() { if s == 0 { r.remove(t); } else { r.insert(t.to_string(), json!(s)); } }
        }
        if let Some(r) = b["role"].as_str() { st["role"] = json!(if r == "cop" || r == "run" { r } else { "" }); }
        if let Some(by) = b["caught"].as_str().filter(|x| !x.is_empty() && x.len() <= 40) {
            if let Some(a) = st["caught"].as_array_mut() { a.push(json!({"by": by, "t": now_ms()})); }
        }
        if b["pc"].is_object() {
            // Rundendaten des Gejagten: Sterne, raus (von wem); die Zeit der Festnahme setzt das Gerät selbst
            let (old, mut p) = (st["pc"].clone(), b["pc"].clone());
            let same = old["rid"] == p["rid"] && old["out"] == true;
            p["t"] = if same { old["t"].clone() } else if p["out"] == true { json!(now_ms()) } else { json!(0) };
            st["pc"] = p;
        }
        st["n"] = json!(take_chars(name, 24));
        if let Some(c) = b["c"].as_u64() { st["c"] = json!(c % 100_000); }
        let st = clean_ev(&st);
        self.ev.lock().unwrap().entry(code.clone()).or_default().insert(self.dev.clone(), st.clone());
        let mut msg = st.clone(); msg["y"] = json!("ev"); msg["d"] = json!(self.dev);
        if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send_to(&msg, &format!("ev/{}", self.dev), true); }
        json!({"ok": true})
    }

    fn on_pc(&self, code: &str, p: Value) {
        let mut m = self.pc.lock().unwrap();
        if m.get(code).and_then(|x| x["t"].as_u64()).is_some_and(|alt| alt > p["t"].as_u64().unwrap_or(0)) { return; }
        m.insert(code.to_string(), p);
    }

    // Police Chase: Runde starten (Teams verteilen) oder beenden – nur der Kolonnenführer
    pub fn pc_set(&self, b: &Value, name: &str) -> Value {
        let Some(code) = self.first_open(b["code"].as_str().unwrap_or("")) else { return json!({"ok": false, "why": "Keine Kolonne offen"}) };
        if self.leader(&code).as_deref() != Some(self.dev.as_str()) { return json!({"ok": false, "why": "Das kann nur der Kolonnenführer"}); }
        let now = now_ms();
        let cur = self.pc.lock().unwrap().get(&code).cloned();
        let running = cur.as_ref().is_some_and(|p| pc_running(p, now));
        let msg = if b["start"].as_bool() == Some(true) {
            if running { return json!({"ok": false, "why": "Es läuft schon eine Runde"}); }
            let mut players: Vec<(String, String)> = self.members(&code).into_iter().map(|(d, n, _)| (d, n)).collect();
            players.push((self.dev.clone(), take_chars(name, 24)));
            if players.len() < PC_MIN { return json!({"ok": false, "why": format!("Mindestens {PC_MIN} Fahrer nötig (gerade {})", players.len())}); }
            let ev = self.ev.lock().unwrap().get(&code).cloned().unwrap_or_default();
            let wish: Vec<u8> = players.iter().map(|(d, _)| match ev.get(d).and_then(|e| e["role"].as_str()) { Some("cop") => 0, Some("run") => 2, _ => 1 }).collect();
            let mut rnd = vec![0u8; players.len() * 4];
            let _ = getrandom::fill(&mut rnd);
            let ids: Vec<String> = players.iter().map(|(d, _)| d.clone()).collect();
            let (cops, runs) = pick_teams(&ids, &wish, &rnd);
            let dur = match b["dur"].as_u64() { Some(m @ (15 | 20 | 30)) => m, _ => 20 };
            let nm: serde_json::Map<String, Value> = players.iter().map(|(d, n)| (d.clone(), json!(n))).collect();
            json!({"y": "pc", "d": self.dev, "rid": crate::util::new_code()[..6].to_string(), "start": now, "dur": dur * 60_000, "cops": cops, "runs": runs, "nm": nm, "end": 0, "res": "", "t": now})
        } else if b["end"].as_bool() == Some(true) {
            let Some(mut p) = cur.filter(|p| p["end"].as_u64() == Some(0)) else { return json!({"ok": false, "why": "Keine Runde offen"}) };
            if b["rid"].as_str().is_some_and(|r| p["rid"].as_str() != Some(r)) { return json!({"ok": false}); }
            let res = b["res"].as_str().filter(|r| ["cop", "run", "abort"].contains(r)).unwrap_or("abort");
            p["y"] = json!("pc"); p["d"] = json!(self.dev); p["end"] = json!(now); p["res"] = json!(res); p["t"] = json!(now);
            p
        } else { return json!({"ok": false}) };
        let Some(p) = clean_pc(&msg) else { return json!({"ok": false}) };
        self.on_pc(&code, p.clone());
        if let Some(k) = self.kanaele.lock().unwrap().get(&code) { k.send_to(&msg, "pc", true); }
        json!({"ok": true, "pc": p})
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
            let mut promo = serde_json::Map::new();
            if let Some(v) = self.promo.lock().unwrap().get(c) {
                for (t, s) in v {
                    let n = s.iter().filter(|d| online.contains(*d) && *d != t).count();
                    if n == 0 || !online.contains(t) { continue; }
                    promo.insert(t.clone(), json!({"n": n, "mine": s.contains(&self.dev)}));
                }
            }
            for (t, v) in promo.iter_mut() { v["need"] = json!(self.need(c, t).1); }
            let memj: Vec<Value> = {
                let fr = self.friends.lock().unwrap();
                mem.iter().map(|(d, n, p)| { let f = fr.get(d); json!({"d": d, "n": n, "pid": p, "c": f.map(|f| f.f.c), "v": f.map(|f| f.f.v), "g": f.map(|f| f.f.g), "x": f.filter(|f| f.pos).map(|f| f.f.x), "z": f.filter(|f| f.pos).map(|f| f.f.z)}) }).collect()
            };
            let ev = self.ev.lock().unwrap().get(c).cloned().unwrap_or_default();
            let ev: serde_json::Map<String, Value> = ev.into_iter().filter(|(d, _)| online.contains(d)).collect();
            let pc = self.pc.lock().unwrap().get(c).filter(|p| now_ms() < p["start"].as_u64().unwrap_or(0) + p["dur"].as_u64().unwrap_or(0) + 2 * 3_600_000).cloned().unwrap_or(Value::Null);
            json!({"code": fmt_code(c), "leader": self.leader(c), "me": self.dev, "mem": memj, "votes": votes, "promo": promo, "ev": ev, "pc": pc})
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
            if self.lead_ov.lock().unwrap().get(&l.code).map(|x| *x == self.dev).unwrap_or(false) { m["lo"] = json!(self.dev); }
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
        self.leave_others(&code);
        self.liste.lock().unwrap().push(l);
        self.save();
        self.open(&code);
        json!({"ok": true, "code": fmt_code(&code)})
    }

    pub fn join(self: &Arc<Self>, code: &str) -> Value {
        if !valid_code(code) { return json!({"ok": false, "why": "Der Code hat 9 Zeichen, z. B. K7M-2QX-9PA"}); }
        let code = norm_code(code);
        self.leave_others(&code);
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
        self.net.publish(&format!("{}/ev/{}", ch.topic, self.dev), &[], true);
        self.ev.lock().unwrap().remove(&code);
        self.pc.lock().unwrap().remove(&code);
        self.promo.lock().unwrap().remove(&code);
        self.lead_ov.lock().unwrap().remove(&code);
        self.liste.lock().unwrap().retain(|l| l.code != code);
        self.save();
        true
    }

    // man ist immer nur in einer Kolonne: beim Starten/Beitreten/Fortsetzen die anderen offenen verlassen
    fn leave_others(&self, keep: &str) {
        let others: Vec<String> = self.liste.lock().unwrap().iter().filter(|l| l.an && l.code != keep).map(|l| l.code.clone()).collect();
        for c in others { log(format!("Kolonne {} verlassen (nur eine gleichzeitig)", fmt_code(&c))); self.leave(&c); }
    }

    pub fn set_on(self: &Arc<Self>, code: &str, on: bool) -> bool {
        let code = norm_code(code);
        if on { self.leave_others(&code); }
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

// Event-Daten eines Geräts: Sterne (Ziel -> 1–5), Rolle (cop/run), Festnahmen (von wem, wann), Name, Auto
fn clean_ev(o: &Value) -> Value {
    let mut r = serde_json::Map::new();
    if let Some(m) = o["r"].as_object() { for (k, v) in m.iter().take(60) { if k.len() <= 40 { if let Some(s) = v.as_u64().filter(|s| (1..=5).contains(s)) { r.insert(k.clone(), json!(s)); } } } }
    let role = o["role"].as_str().filter(|r| *r == "cop" || *r == "run").unwrap_or("");
    let p = &o["pc"];
    let pc = match p["rid"].as_str().filter(|r| !r.is_empty() && r.len() <= 16) {
        // why: Grund fürs Ausscheiden ("pause" = zu lange im Pausemenü), dl: Frist (ms) eines gerade pausierten Flüchtigen
        Some(rid) => json!({"rid": rid, "st": p["st"].as_u64().unwrap_or(0).min(6), "out": p["out"] == true, "by": take_chars(p["by"].as_str().unwrap_or(""), 40), "t": p["t"].as_u64().unwrap_or(0),
            "why": if p["why"] == "pause" { "pause" } else { "" }, "dl": if p["out"] == true { 0 } else { p["dl"].as_u64().unwrap_or(0) }}),
        None => Value::Null,
    };
    let caught: Vec<Value> = o["caught"].as_array().map(|a| { let skip = a.len().saturating_sub(50); a.iter().skip(skip).filter_map(|x| Some(json!({"by": take_chars(x["by"].as_str()?, 40), "t": x["t"].as_u64().unwrap_or(0)}))).collect() }).unwrap_or_default();
    json!({"r": r, "role": role, "caught": caught, "n": take_chars(o["n"].as_str().unwrap_or(""), 24), "c": o["c"].as_u64().unwrap_or(0) % 100_000, "pc": pc})
}

// ---------------- Police Chase ----------------
const PC_MIN: usize = 4;

// Polizisten je Spielerzahl: bis 6 Spieler 2, bis 12 Spieler 4, bis 18 Spieler 6 … (immer mindestens 1 Flüchtiger)
fn pc_cops(n: usize) -> usize { (2 * n.div_ceil(6)).min(n.saturating_sub(1)) }

// Teams verteilen: Wunsch Polizei (0) zuerst, dann ohne Wunsch (1), Wunsch Flucht (2) zuletzt – innerhalb gleich zufällig
fn pick_teams(ids: &[String], wish: &[u8], rnd: &[u8]) -> (Vec<String>, Vec<String>) {
    let key = |i: usize| rnd.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(i as u32);
    let mut ord: Vec<usize> = (0..ids.len()).collect();
    ord.sort_by_key(|&i| (wish.get(i).copied().unwrap_or(1), key(i)));
    let n = pc_cops(ids.len());
    let cops = ord[..n].iter().map(|&i| ids[i].clone()).collect();
    let runs = ord[n..].iter().map(|&i| ids[i].clone()).collect();
    (cops, runs)
}

fn pc_running(p: &Value, now: u64) -> bool {
    p["end"].as_u64().unwrap_or(0) == 0 && now < p["start"].as_u64().unwrap_or(0) + p["dur"].as_u64().unwrap_or(0)
}

// Rundendaten prüfen: Kennung, Start, Dauer (5–60 min), Teams, Namen, Ende und Ergebnis
fn clean_pc(o: &Value) -> Option<Value> {
    let rid = o["rid"].as_str().filter(|r| !r.is_empty() && r.len() <= 16 && r.chars().all(|c| c.is_ascii_alphanumeric()))?;
    let start = o["start"].as_u64().filter(|s| *s > 0)?;
    let dur = o["dur"].as_u64().filter(|d| (300_000..=3_600_000).contains(d))?;
    let ids = |k: &str| -> Vec<String> { o[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| !x.is_empty() && x.len() <= 40).take(60).map(String::from).collect()).unwrap_or_default() };
    let (cops, mut runs) = (ids("cops"), ids("runs"));
    runs.retain(|r| !cops.contains(r));
    if cops.is_empty() || runs.is_empty() { return None; }
    let mut nm = serde_json::Map::new();
    for d in cops.iter().chain(runs.iter()) { nm.insert(d.clone(), json!(take_chars(o["nm"][d.as_str()].as_str().unwrap_or("Fahrer"), 24))); }
    let res = o["res"].as_str().filter(|r| ["cop", "run", "abort"].contains(r)).unwrap_or("");
    Some(json!({"rid": rid, "d": take_chars(o["d"].as_str().unwrap_or(""), 40), "start": start, "dur": dur, "cops": cops, "runs": runs, "nm": nm,
        "end": o["end"].as_u64().unwrap_or(0), "res": res, "t": o["t"].as_u64().unwrap_or(start)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polizisten_je_spielerzahl() {
        let want = [(4, 2), (5, 2), (6, 2), (7, 4), (12, 4), (13, 6), (18, 6), (19, 8)];
        for (n, c) in want { assert_eq!(pc_cops(n), c, "{n} Spieler"); }
    }

    #[test]
    fn teams_wunsch_zuerst() {
        let ids: Vec<String> = (0..7).map(|i| format!("g{i}")).collect();
        let wish = [2, 1, 0, 2, 1, 2, 0];
        for seed in 0..50u8 {
            let rnd: Vec<u8> = (0..28u8).map(|i| i.wrapping_mul(37).wrapping_add(seed.wrapping_mul(91))).collect();
            let (cops, runs) = pick_teams(&ids, &wish, &rnd);
            assert_eq!(cops.len(), 4); assert_eq!(runs.len(), 3);
            assert!(cops.contains(&"g2".to_string()) && cops.contains(&"g6".to_string()));
            assert!(!cops.contains(&"g0".to_string()) && !cops.contains(&"g3".to_string()) && !cops.contains(&"g5".to_string()), "Wunsch Flucht bleibt Flucht");
            assert!(cops.iter().all(|c| !runs.contains(c)));
        }
    }

    #[test]
    fn runde_pruefen() {
        let ok = json!({"rid": "AB12CD", "start": 1000, "dur": 1_200_000, "cops": ["a", "b"], "runs": ["c", "a", "d"], "nm": {"a": "Anna"}});
        let p = clean_pc(&ok).unwrap();
        assert_eq!(p["runs"], json!(["c", "d"]));
        assert_eq!(p["nm"]["a"], "Anna"); assert_eq!(p["nm"]["c"], "Fahrer");
        assert!(pc_running(&p, 5000) && !pc_running(&p, 1000 + 1_200_000));
        assert!(clean_pc(&json!({"rid": "x y", "start": 1, "dur": 1_200_000, "cops": ["a"], "runs": ["b"]})).is_none());
        assert!(clean_pc(&json!({"rid": "X", "start": 1, "dur": 60_000, "cops": ["a"], "runs": ["b"]})).is_none());
        assert!(clean_pc(&json!({"rid": "X", "start": 1, "dur": 1_200_000, "cops": ["a"], "runs": ["a"]})).is_none());
    }

    #[test]
    fn ev_rundendaten() {
        let e = clean_ev(&json!({"pc": {"rid": "AB12CD", "st": 9, "out": true, "by": "a", "t": 5}}));
        assert_eq!(e["pc"]["st"], 6); assert_eq!(e["pc"]["out"], true);
        assert!(clean_ev(&json!({"pc": {"st": 2}}))["pc"].is_null());
        let z = clean_ev(&json!({"pc": {"rid": "AB12CD", "st": 1, "out": false, "dl": 123_456, "why": "egal"}}));
        assert_eq!(z["pc"]["dl"], 123_456); assert_eq!(z["pc"]["why"], "");
        let r = clean_ev(&json!({"pc": {"rid": "AB12CD", "out": true, "dl": 5, "why": "pause"}}));
        assert_eq!(r["pc"]["why"], "pause"); assert_eq!(r["pc"]["dl"], 0);
    }
}

// Chat-Text: keine Steuerzeichen, Leerraum zusammengefasst, begrenzt
fn clean_text(s: &str, n: usize) -> String {
    let t: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    take_chars(t.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), n)
}
