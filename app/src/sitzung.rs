// Bearbeiten-Sitzungen: mehrere Editoren (2, 3 oder mehr) arbeiten gleichzeitig an derselben Karte.
//
// So bleibt nichts verloren und alle sehen dasselbe:
// - Ein PC ist "Host" und legt die Reihenfolge aller Änderungen fest (fortlaufende Nummer = seq).
//   Damit gibt es keine Uhrzeit-Konflikte mehr: jeder wendet dieselben Schritte in derselben Reihenfolge an.
// - Jede Änderung wird auf JEDEM PC sofort ins Journal (ops.jsonl) geschrieben. Absturz/Neustart = nichts weg.
// - Eigene Änderungen, die der Host noch nicht bestätigt hat, bleiben "ausstehend" (pending.json) und werden
//   so lange erneut geschickt, bis sie bestätigt sind – auch nach einem Neustart oder wenn der Host offline war.
// - Ist der Host weg, übernimmt automatisch der Teilnehmer mit dem vollständigsten Stand (Amtszeit "term" +1).
// - Eine Prüfsummen-Kette über die Reihenfolge erkennt, wenn zwei PCs getrennt weitergearbeitet haben (z. B. zwei
//   Hosts während einer Netz-Störung). Dann folgt der Teilnehmer dem Host und reicht seine Schritte neu ein.
// - Grundlage ist immer eine veröffentlichte Kartenversion (base). Fehlt sie einem Teilnehmer, schickt der Host sie
//   mit (signiert, wird wie ein Download geprüft).
use crate::kanal::Kanal;
use crate::karte::{canon_ent, check_ent, gunzip_bytes, live, strip, Karten};
use crate::mqtt::Net;
use crate::util::{append_line, fmt_code, log, new_code, norm_code, now_ms, random_hex, read_json, sha_hex, take_chars, valid_code, write_atomic, write_json};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

const HB: u64 = 3000;
const HOST_TIMEOUT: u64 = 11000;
const PEER_TIMEOUT: u64 = 15000;
const RETRY: u64 = 3000;
const BATCH: usize = 400;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Op {
    pub s: u64,
    pub id: String,
    pub by: String,
    pub n: String,
    pub t: u64,
    pub tm: u64,
    pub ents: Vec<Value>,
    pub grp: Option<String>,
    pub u: Option<String>,
    pub lb: String,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Sub {
    pub id: String,
    pub by: String,
    pub n: String,
    pub t0: u64,
    pub ents: Vec<Value>,
    pub grp: Option<String>,
    pub u: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Info {
    pub code: String,
    pub title: String,
    pub created: u64,
    pub base: u64,
    pub term: u64,
    pub owner: String,
}

struct Peer {
    n: String,
    r: String,
    tm: u64,
    s: u64,
    last: u64,
}

#[derive(Clone)]
struct Group {
    g: String,
    seqs: Vec<u64>,
    by: String,
    n: String,
    t: u64,
    t1: u64,
    lb: String,
    is_undo: bool,
    u: Option<String>,
    n_e: usize,
    er: usize,
    undone: bool,
}

// Ereignisse für das Dashboard
pub enum SEv {
    Change(Vec<Value>),
    Reload,
    Status,
    Presence(Value),
    Note(String),
}

pub type Emit = Arc<dyn Fn(SEv) + Send + Sync>;

#[derive(Clone)]
pub struct Ctx {
    pub net: Arc<Net>,
    pub karten: Arc<Karten>,
    pub dev: String,
    pub name: Arc<dyn Fn() -> String + Send + Sync>,
    pub dir: PathBuf,
    pub emit: Emit,
}

const TYP: &[(&str, &str)] = &[("motor", "Autobahn"), ("road", "Straße"), ("gravel", "Schotter"), ("off", "Offroad"), ("tunnel", "Tunnel")];
const POI: &[(&str, &str)] = &[("park", "Parkplatz"), ("meet", "Car-Meet"), ("fav", "Favorit"), ("photo", "Aussicht"), ("home", "Zuhause"), ("fuel", "Tankstelle"), ("other", "Ort")];

fn lookup(t: &[(&str, &'static str)], k: &str) -> Option<&'static str> {
    t.iter().find(|(a, _)| *a == k).map(|(_, b)| *b)
}

fn km(p: &[Value]) -> String {
    let f: Vec<f64> = p.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
    let mut l = 0.0;
    let mut i = 2;
    while i + 1 < f.len() { l += (f[i] - f[i - 2]).hypot(f[i + 1] - f[i - 1]); i += 2; }
    format!("{:.2}", l / 1000.0).replace('.', ",")
}

// Bezeichnung eines Schritts aus dem Vorher/Nachher der betroffenen Objekte
pub fn describe(ents: &[Value], before: &dyn Fn(&str) -> Option<Value>) -> String {
    let (mut r_neu, mut r_ge, mut r_del, mut fix, mut x, mut x_aus, mut e_n, mut o_neu, mut o_ge, mut o_del) = (0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    let mut cfg: Vec<String> = Vec::new();
    let mut road: Option<&Value> = None;
    for e in ents {
        let id = e["id"].as_str().unwrap_or("");
        let b = before(id).unwrap_or(Value::Null);
        match e["k"].as_str().unwrap_or("") {
            "r" => {
                if !live(e) { r_del += 1; } else if !live(&b) { if id.starts_with("fh:") { fix += 1; } else { r_neu += 1; } road = Some(e); } else { r_ge += 1; road = Some(e); }
            }
            "x" => { if live(e) { x += 1; } else { x_aus += 1; } }
            "e" => { if live(e) { e_n += 1; } }
            "o" => { if !live(e) { o_del += 1; } else if !live(&b) { o_neu += 1; } else { o_ge += 1; } }
            "c" => cfg.push(id.to_string()),
            _ => {}
        }
    }
    let n = ents.len();
    let only_road = r_neu + r_ge + fix + r_del == 1 && n - x <= 1;
    let t = road.map(|r| {
        let typ = lookup(TYP, r["t"].as_str().unwrap_or("")).unwrap_or("Straße");
        match r["p"].as_array() { Some(p) => format!("{typ} · {} km", km(p)), None => typ.to_string() }
    }).unwrap_or_default();
    if n > 0 && e_n == n { return if n > 1 { format!("Radiert · {n} Stellen") } else { "Radiert".into() }; }
    if only_road && r_neu > 0 { return format!("Neue Straße · {t}"); }
    if only_road && fix > 0 { return format!("Forza-Straße korrigiert · {t}"); }
    if only_road && r_ge > 0 { return format!("{}{t}", if x > 0 { "Gelernte Straße übernommen · " } else { "Straße geändert · " }); }
    if only_road && r_del > 0 { return if x > 0 { "Korrektur entfernt".into() } else { "Straße gelöscht".into() }; }
    if n == x { return if n > 1 { format!("{n} Straßen ausgeblendet") } else { "Straße gelöscht".into() }; }
    if n == x_aus { return "Wieder eingeblendet".into(); }
    if n == 1 && o_neu + o_ge + o_del > 0 {
        let p = &ents[0];
        let b = before(p["id"].as_str().unwrap_or("")).unwrap_or(Value::Null);
        let k = lookup(POI, p["kind"].as_str().unwrap_or("")).or_else(|| if live(&b) { lookup(POI, b["kind"].as_str().unwrap_or("")) } else { None }).unwrap_or("Ort");
        return format!("{}{k}", if o_neu > 0 { "Ort gesetzt · " } else if o_ge > 0 { "Ort geändert · " } else { "Ort gelöscht · " });
    }
    if cfg.len() == n {
        return if cfg.iter().any(|c| c == "cfg:roadfix") { "Straßenkorrekturen übernommen".into() } else if cfg.iter().any(|c| c == "cfg:mapfit") { "Kartenausrichtung geändert".into() } else { "Einstellung geändert".into() };
    }
    let mut parts = Vec::new();
    if r_neu + fix > 0 { parts.push(format!("{} Straßen neu", r_neu + fix)); }
    if r_ge > 0 { parts.push(format!("{r_ge} geändert")); }
    if r_del + x > 0 { parts.push(format!("{} gelöscht", r_del + x)); }
    if e_n > 0 { parts.push(format!("{e_n} Radierungen")); }
    if o_neu + o_ge + o_del > 0 { parts.push(format!("{} Orte", o_neu + o_ge + o_del)); }
    format!("Sammel-Änderung · {}", if parts.is_empty() { format!("{n} Objekte") } else { parts.join(", ") })
}

fn with_ts(e: &Value, ts: u64, a: &str) -> Value {
    let mut e = e.clone();
    if let Some(o) = e.as_object_mut() { o.insert("ts".into(), json!(ts)); o.insert("a".into(), json!(a)); }
    e
}

fn id_of(e: &Value) -> String {
    e["id"].as_str().unwrap_or("").to_string()
}

struct Inner {
    info: Info,
    dir: PathBuf,
    ops: Vec<Op>,
    chain: Vec<String>,
    by_id: HashMap<String, usize>,
    touch: HashMap<String, Vec<u64>>,
    held: BTreeMap<u64, Op>,
    pending: Vec<Sub>,
    peers: HashMap<String, Peer>,
    base_parts: Option<(u64, usize, HashMap<usize, String>, Option<String>)>,
    role: &'static str,
    term: u64,
    host_dev: Option<String>,
    last_host: u64,
    last_hb: u64,
    last_retry: u64,
    last_need: u64,
    last_t: u64,
    sub_n: u64,
    closed: bool,
    ready: bool,
    state: HashMap<String, Value>,
    base_max_ts: u64,
    base_ents: Option<(u64, Arc<Vec<Value>>, HashMap<String, usize>)>,
    groups: Option<(u64, Vec<Group>, HashMap<String, usize>)>,
    out: Vec<Value>,
    evs: Vec<SEv>,
}

pub struct Sitzung {
    inner: Mutex<Inner>,
    kanal: Mutex<Option<Arc<Kanal>>>,
    ctx: Ctx,
}

impl Inner {
    fn journal(&self) -> PathBuf {
        self.dir.join("ops.jsonl")
    }

    fn seq(&self) -> u64 {
        self.ops.len() as u64
    }

    fn chain_at(&self, s: u64) -> String {
        if s == 0 { String::new() } else { self.chain.get(s as usize - 1).cloned().unwrap_or_default() }
    }

    fn push(&mut self, op: Op) {
        let prev = self.chain.last().cloned().unwrap_or_default();
        self.chain.push(sha_hex(format!("{prev}|{}", op.id).as_bytes())[..16].to_string());
        for e in &op.ents { self.touch.entry(id_of(e)).or_default().push(op.s); }
        self.by_id.insert(op.id.clone(), self.ops.len());
        self.ops.push(op);
    }

    fn send(&mut self, mut v: Value, dev: &str) {
        v["d"] = json!(dev);
        self.out.push(v);
    }

    fn save_pending(&self) {
        write_json(&self.dir.join("pending.json"), &serde_json::to_value(&self.pending).unwrap_or_default());
    }

    fn save_info(&mut self) {
        self.info.term = self.term;
        write_json(&self.dir.join("sitzung.json"), &serde_json::to_value(&self.info).unwrap_or_default());
    }

    fn rewrite_journal(&self) {
        let mut t: String = self.ops.iter().map(|o| serde_json::to_string(o).unwrap_or_default() + "\n").collect();
        if t.is_empty() { t = String::new(); }
        if let Err(e) = write_atomic(&self.journal(), t.as_bytes()) { log(format!("Journal konnte nicht geschrieben werden: {e}")); }
    }

    fn append(&self, op: &Op) {
        if let Err(e) = append_line(&self.journal(), &serde_json::to_string(op).unwrap_or_default()) { log(format!("Journal: {e}")); }
    }

    fn drop_pending(&mut self, id: &str) {
        let n = self.pending.len();
        self.pending.retain(|p| p.id != id);
        if self.pending.len() != n { self.save_pending(); }
    }

    fn apply(&mut self, i: usize) {
        let op = &self.ops[i];
        for e in &op.ents {
            let id = id_of(e);
            // nach dem Umstellen auf eine neuere Kartenversion gewinnt dort die jeweils neuere Änderung
            if self.state.get(&id).is_some_and(|c| c["ts"].as_u64().unwrap_or(0) > op.t) { continue; }
            self.state.insert(id, with_ts(e, op.t, &op.by));
        }
    }

    fn base_lookup(&self, id: &str) -> Option<Value> {
        let (_, ents, idx) = self.base_ents.as_ref()?;
        idx.get(id).map(|i| ents[*i].clone())
    }

    // Wert eines Objekts direkt vor Schritt seq
    fn before(&self, id: &str, seq: u64) -> Option<Value> {
        if let Some(a) = self.touch.get(id) {
            for s in a.iter().rev() {
                if *s < seq {
                    let op = &self.ops[*s as usize - 1];
                    if let Some(e) = op.ents.iter().find(|e| e["id"] == id) { return Some(with_ts(e, op.t, &op.by)); }
                }
            }
        }
        self.base_lookup(id)
    }

    fn view(&self, dev: &str) -> Vec<Value> {
        if self.pending.is_empty() { return self.state.values().cloned().collect(); }
        let mut m = self.state.clone();
        for s in &self.pending { for e in &s.ents { m.insert(id_of(e), with_ts(e, s.t0, dev)); } }
        m.into_values().collect()
    }

    fn groups(&mut self) -> &(u64, Vec<Group>, HashMap<String, usize>) {
        if self.groups.as_ref().is_none_or(|g| g.0 != self.seq()) {
            let mut out: Vec<Group> = Vec::new();
            let mut idx: HashMap<String, usize> = HashMap::new();
            for op in &self.ops {
                let key = match &op.grp { Some(g) => format!("{}:{g}", op.by), None => format!("s{}", op.s) };
                let gi = match idx.get(&key) {
                    Some(i) => *i,
                    None => {
                        out.push(Group { g: key.clone(), seqs: vec![], by: op.by.clone(), n: op.n.clone(), t: op.t, t1: op.t, lb: op.lb.clone(), is_undo: op.u.is_some(), u: op.u.clone(), n_e: 0, er: 0, undone: false });
                        idx.insert(key, out.len() - 1);
                        out.len() - 1
                    }
                };
                let g = &mut out[gi];
                g.seqs.push(op.s);
                g.t1 = op.t;
                g.n_e += op.ents.len();
                if op.ents.iter().all(|e| e["k"] == "e") { g.er += op.ents.len(); }
                if g.er == g.n_e && g.n_e > 1 { g.lb = format!("Radiert · {} Stellen", g.n_e); }
            }
            // rückgängig gemacht? (von hinten nach vorn: ein Rückgängig zählt nur, wenn es selbst noch gilt)
            let mut undone: HashSet<String> = HashSet::new();
            for g in out.iter_mut().rev() {
                g.undone = undone.contains(&g.g);
                if g.is_undo && !g.undone { if let Some(u) = &g.u { undone.insert(u.clone()); } }
            }
            self.groups = Some((self.seq(), out, idx));
        }
        self.groups.as_ref().unwrap()
    }

    fn group(&mut self, key: &str) -> Option<Group> {
        let (_, gs, idx) = self.groups();
        idx.get(key).map(|i| gs[*i].clone())
    }
}

impl Sitzung {
    pub fn new(info: Info, ctx: Ctx) -> Arc<Sitzung> {
        let dir = ctx.dir.join(&info.code);
        let _ = std::fs::create_dir_all(&dir);
        let term = info.term;
        let mut inner = Inner {
            info, dir, ops: vec![], chain: vec![], by_id: HashMap::new(), touch: HashMap::new(), held: BTreeMap::new(), pending: vec![],
            peers: HashMap::new(), base_parts: None, role: "member", term, host_dev: None, last_host: 0, last_hb: 0, last_retry: 0,
            last_need: 0, last_t: 0, sub_n: 0, closed: false, ready: false, state: HashMap::new(), base_max_ts: 0, base_ents: None,
            groups: None, out: vec![], evs: vec![],
        };
        // Journal laden (eine halbe letzte Zeile nach einem Absturz wird übersprungen)
        if let Ok(t) = std::fs::read_to_string(inner.journal()) {
            for line in t.lines().filter(|l| !l.trim().is_empty()) {
                match serde_json::from_str::<Op>(line) {
                    Ok(op) if op.s == inner.seq() + 1 => inner.push(op),
                    Ok(_) => {}
                    Err(_) => log(format!("Sitzung {}: beschädigte Journal-Zeile übersprungen", inner.info.code)),
                }
            }
            inner.last_t = inner.ops.last().map_or(0, |o| o.t);
        }
        if let Some(Value::Array(a)) = read_json(&inner.dir.join("pending.json")) {
            for p in a {
                if let Ok(s) = serde_json::from_value::<Sub>(p) { if !s.id.is_empty() && !inner.by_id.contains_key(&s.id) { inner.pending.push(s); } }
            }
        }
        let s = Arc::new(Sitzung { inner: Mutex::new(inner), kanal: Mutex::new(None), ctx });
        s.materialize(&mut s.inner.lock().unwrap());
        s
    }

    fn materialize(&self, i: &mut Inner) {
        let base = if i.info.base > 0 { self.ctx.karten.ents(i.info.base) } else { None };
        i.ready = base.is_some();
        i.state.clear();
        i.base_max_ts = 0;
        i.base_ents = None;
        if let Some(b) = base {
            let mut idx = HashMap::new();
            for (k, e) in b.iter().enumerate() {
                i.state.insert(id_of(e), e.clone());
                idx.insert(id_of(e), k);
                i.base_max_ts = i.base_max_ts.max(e["ts"].as_u64().unwrap_or(0));
            }
            i.base_ents = Some((i.info.base, b, idx));
        }
        for k in 0..i.ops.len() { i.apply(k); }
    }

    // gesammelte Nachrichten senden und Ereignisse melden – immer erst, wenn nichts mehr gesperrt ist
    fn flush(&self, mut g: std::sync::MutexGuard<Inner>) {
        let out = std::mem::take(&mut g.out);
        let evs = std::mem::take(&mut g.evs);
        drop(g);
        if !out.is_empty() {
            let k = self.kanal.lock().unwrap().clone();
            if let Some(k) = k { for o in &out { k.send(o); } }
        }
        for e in evs { (self.ctx.emit)(e); }
    }

    pub fn code(&self) -> String {
        self.inner.lock().unwrap().info.code.clone()
    }

    pub fn start(self: &Arc<Self>, as_host: bool) {
        let w: Weak<Sitzung> = Arc::downgrade(self);
        let code = self.code();
        let k = Kanal::new(&self.ctx.net, "edit", &code, Arc::new(move |o: Value, _sub: &str| {
            if let Some(s) = w.upgrade() { s.on_msg(o); }
        }));
        *self.kanal.lock().unwrap() = Some(Arc::new(k));
        let mut g = self.inner.lock().unwrap();
        g.last_host = now_ms();   // erst auf den Host warten, bevor dieser PC selbst übernimmt
        if as_host { self.become_host(&mut g, true); }
        self.send_sync(&mut g);
        self.hb(&mut g);
        self.flush(g);
        let w = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(s) = w.upgrade() else { break };
                if s.inner.lock().unwrap().closed { break; }
                s.tick();
            }
        });
    }

    pub fn stop(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.closed { return; }
        g.closed = true;
        let dev = self.ctx.dev.clone();
        g.send(json!({"y": "bye"}), &dev);
        self.flush(g);
        *self.kanal.lock().unwrap() = None;
    }

    // Abgleich mit dem Host: Kettenwerte an den Stellen seq, seq-1, seq-2, seq-4, … – der Host findet so schnell die
    // letzte gemeinsame Stelle und schickt alles danach
    fn send_sync(&self, g: &mut Inner) {
        let mut pts = Vec::new();
        let seq = g.seq() as i64;
        let (mut k, mut s) = (0u32, seq);
        while s > 0 { pts.push(json!([s, g.chain_at(s as u64)])); s = seq - (1i64 << k); k += 1; }
        pts.push(json!([0, ""]));
        let b = g.info.base;
        g.send(json!({"y": "sync", "pts": pts, "b": b}), &self.ctx.dev);
    }

    fn hb(&self, g: &mut Inner) {
        g.last_hb = now_ms();
        let v = json!({"y": "hb", "n": (self.ctx.name)(), "r": g.role, "tm": g.term, "s": g.seq(), "b": g.info.base, "ow": g.info.owner, "ti": g.info.title});
        g.send(v, &self.ctx.dev);
    }

    fn become_host(&self, g: &mut Inner, initial: bool) {
        let max_tm = g.peers.values().map(|p| p.tm).chain([g.term]).max().unwrap_or(0);
        g.role = "host";
        g.host_dev = Some(self.ctx.dev.clone());
        g.term = if initial && g.ops.is_empty() { max_tm.max(1) } else { max_tm + 1 };
        g.last_host = now_ms();
        g.save_info();
        log(format!("Sitzung {}: dieser PC ist jetzt Host (Amtszeit {})", g.info.code, g.term));
        self.hb(g);
        for s in g.pending.clone() { self.accept(g, s); }
        g.evs.push(SEv::Status);
    }

    fn step_down(&self, g: &mut Inner, dev: &str, tm: u64) {
        if g.role == "host" { log(format!("Sitzung {}: {dev} ist Host (Amtszeit {tm}) – dieser PC macht als Teilnehmer weiter", g.info.code)); }
        g.role = "member";
        g.host_dev = Some(dev.to_string());
        g.term = tm;
        g.last_host = now_ms();
        g.save_info();
        self.send_sync(g);
        g.evs.push(SEv::Status);
    }

    fn better(g: &Inner, tm: u64, d: &str) -> bool {
        tm > g.term || (tm == g.term && g.host_dev.as_deref().is_none_or(|h| d < h))
    }

    fn tick(&self) {
        let mut g = self.inner.lock().unwrap();
        let now = now_ms();
        if now.saturating_sub(g.last_hb) >= HB { self.hb(&mut g); }
        let n = g.peers.len();
        g.peers.retain(|_, p| now.saturating_sub(p.last) <= PEER_TIMEOUT);
        if g.peers.len() != n { g.evs.push(SEv::Status); }
        if g.role == "member" {
            if !self.ctx.net.ok() {
                g.last_host = g.last_host.max((now + 6000).saturating_sub(HOST_TIMEOUT));   // offline: nach dem Verbinden erst zuhören
            } else if now.saturating_sub(g.last_host) > HOST_TIMEOUT {
                self.elect(&mut g);
            } else if !g.pending.is_empty() && g.host_dev.is_some() && now.saturating_sub(g.last_retry) > RETRY {
                g.last_retry = now;
                for s in g.pending.clone() { g.send(json!({"y": "sub", "op": s}), &self.ctx.dev); }
            }
        }
        self.flush(g);
    }

    // Host fehlt: der Teilnehmer mit dem vollständigsten Stand übernimmt (gleich -> kleinste Kennung)
    fn elect(&self, g: &mut Inner) {
        let now = now_ms();
        let mut c: Vec<(u64, String)> = vec![(g.seq(), self.ctx.dev.clone())];
        for (d, p) in &g.peers { if now.saturating_sub(p.last) < HOST_TIMEOUT { c.push((p.s, d.clone())); } }
        c.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        if c[0].1 == self.ctx.dev && g.ready { self.become_host(g, false); } else { g.last_host = (now + 4000).saturating_sub(HOST_TIMEOUT); }
    }

    fn on_msg(&self, o: Value) {
        let d = o["d"].as_str().unwrap_or("").to_string();
        if d.is_empty() || d == self.ctx.dev { return; }
        let mut g = self.inner.lock().unwrap();
        if g.closed { return; }
        let now = now_ms();
        let num = |k: &str| o[k].as_u64().unwrap_or(0);
        match o["y"].as_str().unwrap_or("") {
            "hb" => {
                let is_new = !g.peers.contains_key(&d);
                g.peers.insert(d.clone(), Peer { n: take_chars(o["n"].as_str().unwrap_or("Freund"), 24), r: o["r"].as_str().unwrap_or("").to_string(), tm: num("tm"), s: num("s"), last: now });
                if o["r"] == "host" {
                    let tm = num("tm");
                    if g.host_dev.as_deref() == Some(&d) && tm == g.term { g.last_host = now; }
                    else if Self::better(&g, tm, &d) || (g.role == "member" && now.saturating_sub(g.last_host) > HOST_TIMEOUT) { self.step_down(&mut g, &d, tm); }
                    else if g.role == "host" { self.hb(&mut g); }   // der andere soll merken, dass es hier einen besseren Host gibt
                    if g.host_dev.as_deref() == Some(&d) {
                        let b = num("b");
                        if b > 0 && b != g.info.base { self.rebase(&mut g, b); }
                        if num("s") > g.seq() && now.saturating_sub(g.last_need) > 2000 {
                            g.last_need = now;
                            let from = g.seq() + 1;
                            g.send(json!({"y": "need", "from": from}), &self.ctx.dev);
                        }
                        if g.info.title.is_empty() { if let Some(t) = o["ti"].as_str().filter(|t| !t.is_empty()) { g.info.title = take_chars(t, 40); g.save_info(); } }
                        if g.info.owner.is_empty() { if let Some(t) = o["ow"].as_str().filter(|t| !t.is_empty()) { g.info.owner = t.to_string(); g.save_info(); } }
                    }
                }
                if is_new { g.evs.push(SEv::Status); }
            }
            "sync" => {
                if g.role != "host" { return; }
                let Some(pts) = o["pts"].as_array() else { return };
                let mut p: Vec<(u64, String)> = pts.iter().filter_map(|x| Some((x[0].as_u64()?, x[1].as_str()?.to_string()))).collect();
                p.sort_by(|a, b| b.0.cmp(&a.0));
                let m = p.iter().find(|(s, h)| *s == 0 || (*s <= g.seq() && &g.chain_at(*s) == h)).map_or(0, |x| x.0);
                self.hb(&mut g);
                let (hs, hh) = (g.seq(), g.chain_at(g.seq()));
                g.send(json!({"y": "syncok", "to": d, "s": m, "hs": hs, "hh": hh}), &self.ctx.dev);
                self.send_ops(&mut g, m + 1);
            }
            "syncok" => {
                // dieser PC hat Schritte, die der Host nicht kennt (z. B. von einem inzwischen abgelösten Host) -> neu einreichen
                let hs = num("hs");
                if o["to"] == self.ctx.dev.as_str() && g.host_dev.as_deref() == Some(&d) && g.seq() > hs && o["hh"].as_str() == Some(&g.chain_at(hs)) {
                    self.truncate(&mut g, hs + 1);
                    g.evs.push(SEv::Reload);
                }
            }
            "need" => { if g.role == "host" { self.send_ops(&mut g, num("from")); } }
            "sub" => {
                if g.role == "host" {
                    if let Ok(s) = serde_json::from_value::<Sub>(o["op"].clone()) { if !s.id.is_empty() { self.accept(&mut g, s); } }
                }
            }
            "ops" => {
                let tm = num("tm");
                if g.host_dev.as_deref() == Some(&d) || Self::better(&g, tm, &d) {
                    if g.host_dev.as_deref() != Some(&d) { self.step_down(&mut g, &d, tm); }
                    let list: Vec<Op> = o["list"].as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()).unwrap_or_default();
                    self.receive(&mut g, list);
                }
            }
            "rej" => {
                let id = o["id"].as_str().unwrap_or("");
                if g.pending.iter().any(|p| p.id == id) {
                    g.drop_pending(id);
                    g.evs.push(SEv::Reload);
                    g.evs.push(SEv::Note(format!("Eine Änderung wurde abgelehnt: {}", o["why"].as_str().unwrap_or("ungültig"))));
                }
            }
            "pr" => g.evs.push(SEv::Presence(o.clone())),
            "bye" => {
                g.peers.remove(&d);
                if g.host_dev.as_deref() == Some(&d) { g.last_host = 0; }
                g.evs.push(SEv::Status);
            }
            "bq" => { if g.role == "host" { self.send_base(&mut g, num("v")); } }
            "bp" => self.base_recv(&mut g, &o),
            "pub" => g.evs.push(SEv::Note(format!("{} hat die Karte veröffentlicht – die neue Version kommt in 1–2 Minuten automatisch", o["n"].as_str().unwrap_or("Ein Editor")))),
            _ => {}
        }
        self.flush(g);
    }

    // ---------------- Host: Änderungen annehmen ----------------
    fn accept(&self, g: &mut Inner, sub: Sub) {
        if let Some(i) = g.by_id.get(&sub.id).copied() {
            let op = g.ops[i].clone();
            self.broadcast(g, vec![op]);
            g.drop_pending(&sub.id);
            return;
        }
        let mut ents = Vec::new();
        let mut bad = Vec::new();
        for raw in &sub.ents {
            let e = canon_ent(&strip(raw));
            match check_ent(&e) { Some(err) => bad.push(err), None => ents.push(e) }
        }
        if ents.is_empty() {
            g.send(json!({"y": "rej", "id": sub.id, "why": bad.first().cloned().unwrap_or_else(|| "leer".into())}), &self.ctx.dev);
            if g.pending.iter().any(|p| p.id == sub.id) { g.drop_pending(&sub.id); g.evs.push(SEv::Reload); }
            return;
        }
        let s = g.seq() + 1;
        let t = now_ms().max(g.last_t + 1).max(g.base_max_ts + 1);   // immer neuer als alles Bisherige
        let lb = match &sub.u {
            Some(u) => match g.group(u) {
                Some(gr) if gr.is_undo => format!("Wiederholt: {}", gr.lb.trim_start_matches("Rückgängig: ")),
                Some(gr) => format!("Rückgängig: {}", gr.lb),
                None => "Rückgängig".into(),
            },
            None => { let st = &g.state; describe(&ents, &|id| st.get(id).cloned()) }
        };
        let op = Op {
            s, id: take_chars(&sub.id, 40), by: take_chars(if sub.by.is_empty() { &self.ctx.dev } else { &sub.by }, 12),
            n: take_chars(if sub.n.is_empty() { "Freund" } else { &sub.n }, 24), t, tm: g.term, ents,
            grp: sub.grp.as_deref().map(|x| take_chars(x, 40)), u: sub.u.clone(), lb,
        };
        self.commit(g, op.clone());
        self.broadcast(g, vec![op]);
    }

    fn commit(&self, g: &mut Inner, op: Op) {
        g.last_t = op.t;
        g.append(&op);
        let id = op.id.clone();
        g.push(op);
        let i = g.ops.len() - 1;
        g.apply(i);
        g.drop_pending(&id);
        let ch: Vec<Value> = g.ops[i].ents.iter().filter_map(|e| g.state.get(&id_of(e)).cloned()).collect();
        g.evs.push(SEv::Change(ch));
    }

    fn broadcast(&self, g: &mut Inner, list: Vec<Op>) {
        let tm = g.term;
        g.send(json!({"y": "ops", "tm": tm, "list": list}), &self.ctx.dev);
    }

    fn send_ops(&self, g: &mut Inner, from: u64) {
        let from = from.max(1) as usize;
        let mut i = from;
        while i as u64 <= g.seq() {
            let end = (i - 1 + BATCH).min(g.ops.len());
            let list = g.ops[i - 1..end].to_vec();
            self.broadcast(g, list);
            i += BATCH;
        }
    }

    // ---------------- Teilnehmer: Änderungen vom Host übernehmen ----------------
    fn receive(&self, g: &mut Inner, mut list: Vec<Op>) {
        list.retain(|op| op.s > 0);
        list.sort_by_key(|op| op.s);
        let (mut changed, mut reload) = (false, false);
        let mut queue: std::collections::VecDeque<Op> = list.into();
        while let Some(op) = queue.pop_front() {
            if op.s <= g.seq() {
                if g.ops[op.s as usize - 1].id == op.id { continue; }
                // eigener Stand weicht ab: ab hier dem Host folgen, die eigenen Schritte werden neu eingereicht
                self.truncate(g, op.s);
                reload = true;
            }
            if op.s == g.seq() + 1 {
                if op.ents.iter().any(|e| check_ent(e).is_some()) { log("Sitzung: ungültiger Schritt vom Host ignoriert"); break; }
                g.last_t = op.t;
                g.append(&op);
                let id = op.id.clone();
                g.push(op);
                let i = g.ops.len() - 1;
                g.apply(i);
                g.drop_pending(&id);
                changed = true;
                if !reload {
                    let ch: Vec<Value> = g.ops[i].ents.iter().filter_map(|e| g.state.get(&id_of(e)).cloned()).collect();
                    g.evs.push(SEv::Change(ch));
                }
                // vorher zu früh angekommene Schritte nachholen
                let next = g.seq() + 1;
                if let Some(h) = g.held.remove(&next) { queue.push_front(h); }
            } else if op.s > g.seq() + 1 {
                g.held.insert(op.s, op);
            }
        }
        if !g.held.is_empty() && now_ms().saturating_sub(g.last_need) > 2000 {
            g.last_need = now_ms();
            let from = g.seq() + 1;
            g.send(json!({"y": "need", "from": from}), &self.ctx.dev);
        }
        if reload { g.evs.push(SEv::Reload); }
        if changed { g.evs.push(SEv::Status); }
    }

    fn truncate(&self, g: &mut Inner, s: u64) {
        let all = std::mem::take(&mut g.ops);
        let (keep, removed) = all.split_at((s as usize - 1).min(all.len()));
        // der alte Stand bleibt als Datei liegen
        let _ = std::fs::copy(g.journal(), g.dir.join(format!("ops.jsonl.vor-abgleich-{}", now_ms())));
        g.chain.clear();
        g.by_id.clear();
        g.touch.clear();
        for op in keep { g.push(op.clone()); }
        g.last_t = keep.last().map_or(0, |o| o.t);
        g.rewrite_journal();
        self.materialize(g);
        for op in removed {
            if !g.pending.iter().any(|p| p.id == op.id) {
                g.pending.push(Sub { id: op.id.clone(), by: op.by.clone(), n: op.n.clone(), t0: op.t, ents: op.ents.clone(), grp: op.grp.clone(), u: op.u.clone() });
            }
        }
        g.save_pending();
        log(format!("Sitzung {}: {} Schritte werden neu eingereicht (Abgleich mit dem Host)", g.info.code, removed.len()));
    }

    // ---------------- Basis (Kartenversion) ----------------
    fn rebase(&self, g: &mut Inner, v: u64) {
        if v == g.info.base { return; }
        g.info.base = v;
        g.save_info();
        if !self.ctx.karten.has(v) {
            g.ready = false;
            g.send(json!({"y": "bq", "v": v}), &self.ctx.dev);
            g.evs.push(SEv::Status);
            return;
        }
        self.materialize(g);
        g.evs.push(SEv::Reload);
        g.evs.push(SEv::Status);
    }

    pub fn rebase_to(&self, v: u64) {
        let mut g = self.inner.lock().unwrap();
        self.rebase(&mut g, v);
        self.hb(&mut g);
        self.flush(g);
    }

    fn send_base(&self, g: &mut Inner, v: u64) {
        let Some((man, buf)) = self.ctx.karten.raw(v) else { return };
        let gz = B64.encode(crate::util::gzip(&buf));
        const PARTB: usize = 45000;
        let n = gz.len().div_ceil(PARTB);
        for i in 0..n {
            let data = &gz[i * PARTB..((i + 1) * PARTB).min(gz.len())];
            let mut m = json!({"y": "bp", "v": v, "i": i, "n": n, "data": data});
            if i == 0 { m["man"] = json!(man); }
            g.send(m, &self.ctx.dev);
        }
    }

    fn base_recv(&self, g: &mut Inner, o: &Value) {
        let (v, i, n) = (o["v"].as_u64().unwrap_or(0), o["i"].as_u64().unwrap_or(0) as usize, o["n"].as_u64().unwrap_or(0) as usize);
        if v != g.info.base || self.ctx.karten.has(v) || n == 0 || n > 2000 { return; }
        if g.base_parts.as_ref().is_none_or(|p| p.0 != v || p.1 != n) { g.base_parts = Some((v, n, HashMap::new(), None)); }
        let bp = g.base_parts.as_mut().unwrap();
        bp.2.insert(i, o["data"].as_str().unwrap_or("").to_string());
        if let Some(m) = o["man"].as_str() { bp.3 = Some(m.to_string()); }
        if bp.2.len() < bp.1 || bp.3.is_none() { return; }
        let (_, n, parts, man) = g.base_parts.take().unwrap();
        let b64: String = (0..n).map(|k| parts.get(&k).cloned().unwrap_or_default()).collect();
        let Some(buf) = B64.decode(b64).ok().and_then(|gz| gunzip_bytes(&gz, 256 << 20)) else { log("Kartenversion aus der Sitzung kaputt"); return };
        match self.ctx.karten.install(&man.unwrap(), &buf, "Sitzung") {
            Ok(_) => { self.materialize(g); g.evs.push(SEv::Reload); g.evs.push(SEv::Status); }
            Err(e) => log(format!("Kartenversion aus der Sitzung abgelehnt: {e}")),
        }
    }

    // ---------------- Eigene Änderungen ----------------
    fn submit_locked(&self, g: &mut Inner, ents: &[Value], grp: Option<String>, u: Option<String>) -> Sub {
        g.sub_n += 1;
        let sub = Sub {
            id: format!("{}-{:x}-{:x}{}", self.ctx.dev, now_ms(), g.sub_n, random_hex(2)),
            by: self.ctx.dev.clone(), n: (self.ctx.name)(), t0: now_ms(),
            ents: ents.iter().map(|e| canon_ent(&strip(e))).collect(), grp, u,
        };
        g.pending.push(sub.clone());
        g.save_pending();
        if g.role == "host" { self.accept(g, sub.clone()); } else {
            g.send(json!({"y": "sub", "op": sub}), &self.ctx.dev);
            g.last_retry = now_ms();
        }
        sub
    }

    pub fn submit(&self, ents: &[Value], grp: Option<String>) -> Sub {
        let mut g = self.inner.lock().unwrap();
        let s = self.submit_locked(&mut g, ents, grp, None);
        self.flush(g);
        s
    }

    pub fn send(&self, v: Value) {
        let mut g = self.inner.lock().unwrap();
        g.send(v, &self.ctx.dev);
        self.flush(g);
    }

    // ---------------- Verlauf ----------------
    pub fn history(&self) -> Value {
        let mut g = self.inner.lock().unwrap();
        let (_, gs, _) = g.groups();
        let list: Vec<Value> = gs.iter().rev().map(|g| json!({"g": g.g, "by": g.by, "n": g.n, "t": g.t, "t1": g.t1, "lb": g.lb, "undone": g.undone,
            "isUndo": g.is_undo, "u": g.u, "steps": g.seqs.len(), "objs": g.n_e})).collect();
        json!(list)
    }

    // Vorher/Nachher eines Eintrags – zum Hervorheben auf der Karte
    pub fn detail(&self, key: &str) -> Option<Value> {
        let mut g = self.inner.lock().unwrap();
        let gr = g.group(key)?;
        let (first, last) = (gr.seqs[0], *gr.seqs.last().unwrap());
        let mut ids: Vec<String> = Vec::new();
        for s in &gr.seqs { for e in &g.ops[*s as usize - 1].ents { let id = id_of(e); if !ids.contains(&id) { ids.push(id); } } }
        let items: Vec<Value> = ids.iter().map(|id| {
            let b = g.before(id, first).map(|v| strip(&v));
            let a = g.before(id, last + 1).map(|v| strip(&v));
            json!({"id": id, "before": b, "after": a})
        }).collect();
        Some(json!({"g": gr.g, "lb": gr.lb, "n": gr.n, "by": gr.by, "t": gr.t, "undone": gr.undone, "items": items}))
    }

    // Eintrag rückgängig machen. Hat jemand die Objekte danach noch geändert, wird zuerst nachgefragt.
    pub fn undo(&self, key: &str, force: bool) -> Value {
        let mut g = self.inner.lock().unwrap();
        let Some(gr) = g.group(key) else { return json!({"ok": false, "why": "Schritt nicht gefunden"}) };
        if gr.undone { return json!({"ok": false, "why": "Schon rückgängig gemacht"}); }
        let (first, last) = (gr.seqs[0], *gr.seqs.last().unwrap());
        let mut ids: Vec<String> = Vec::new();
        for s in &gr.seqs { for e in &g.ops[*s as usize - 1].ents { let id = id_of(e); if !ids.contains(&id) { ids.push(id); } } }
        let mut later: BTreeMap<String, String> = BTreeMap::new();
        for id in &ids {
            for s in g.touch.get(id).cloned().unwrap_or_default() {
                if s > last { let op = &g.ops[s as usize - 1]; later.insert(op.by.clone(), op.n.clone()); }
            }
        }
        for p in &g.pending { if p.ents.iter().any(|e| ids.contains(&id_of(e))) { later.insert(self.ctx.dev.clone(), "dir".into()); } }
        if !later.is_empty() && !force {
            return json!({"ok": false, "conflict": later.values().collect::<Vec<_>>(), "why": "Danach noch geändert"});
        }
        let ents: Vec<Value> = ids.iter().map(|id| match g.before(id, first) {
            Some(b) => strip(&b),
            None => {
                let k = g.state.get(id).and_then(|c| c["k"].as_str().map(String::from)).unwrap_or_else(|| if id.starts_with("x:") { "x".into() } else { "r".into() });
                json!({"id": id, "k": k, "d": 1})
            }
        }).collect();
        self.submit_locked(&mut g, &ents, None, Some(gr.g.clone()));
        self.flush(g);
        json!({"ok": true})
    }

    // Strg+Z: letzter eigener, noch gültiger Schritt; Strg+Y: letztes eigenes Rückgängig wieder aufheben
    pub fn last_own(&self, redo: bool) -> Option<String> {
        let mut g = self.inner.lock().unwrap();
        let dev = self.ctx.dev.clone();
        let (_, gs, _) = g.groups();
        gs.iter().rev().find(|g| g.by == dev && !g.undone && g.is_undo == redo).map(|g| g.g.clone())
    }

    // ---------------- Abfragen ----------------
    pub fn ready(&self) -> bool {
        self.inner.lock().unwrap().ready
    }

    pub fn is_host(&self) -> bool {
        self.inner.lock().unwrap().role == "host"
    }

    pub fn base(&self) -> u64 {
        self.inner.lock().unwrap().info.base
    }

    pub fn pending_n(&self) -> usize {
        self.inner.lock().unwrap().pending.len()
    }

    pub fn view(&self) -> Vec<Value> {
        self.inner.lock().unwrap().view(&self.ctx.dev)
    }

    pub fn state_ents(&self) -> Vec<Value> {
        self.inner.lock().unwrap().state.values().cloned().collect()
    }

    pub fn status(&self) -> Value {
        let g = self.inner.lock().unwrap();
        let now = now_ms();
        let mut members = vec![json!({"d": self.ctx.dev, "n": (self.ctx.name)(), "r": g.role, "me": true, "s": g.seq()})];
        for (d, p) in &g.peers { if now.saturating_sub(p.last) < PEER_TIMEOUT { members.push(json!({"d": d, "n": p.n, "r": p.r, "s": p.s})); } }
        json!({"code": g.info.code, "title": g.info.title, "base": g.info.base, "ready": g.ready, "role": g.role, "host": g.host_dev, "term": g.term,
            "seq": g.seq(), "pending": g.pending.len(), "members": members, "owner": g.info.owner, "created": g.info.created, "net": self.ctx.net.ok(),
            "hostOk": g.role == "host" || now.saturating_sub(g.last_host) < HOST_TIMEOUT})
    }
}

// ---------------- Verwaltung: erstellen, beitreten, verlassen, nach Neustart fortsetzen ----------------
pub struct Sitzungen {
    ctx: Ctx,
    cur: Mutex<Option<Arc<Sitzung>>>,
}

impl Sitzungen {
    pub fn new(ctx: Ctx) -> Sitzungen {
        let _ = std::fs::create_dir_all(&ctx.dir);
        Sitzungen { ctx, cur: Mutex::new(None) }
    }

    fn aktiv(&self) -> PathBuf {
        self.ctx.dir.join("aktiv.json")
    }

    fn info_of(&self, code: &str) -> Option<Info> {
        read_json(&self.ctx.dir.join(code).join("sitzung.json")).and_then(|v| serde_json::from_value(v).ok())
    }

    pub fn cur(&self) -> Option<Arc<Sitzung>> {
        self.cur.lock().unwrap().clone()
    }

    // nach einem Neustart automatisch wieder in die letzte Sitzung
    pub fn resume(&self) {
        if let Some(code) = read_json(&self.aktiv()).and_then(|a| a["code"].as_str().map(String::from)) {
            if let Some(info) = self.info_of(&code) { self.open(info, false); }
        }
    }

    fn open(&self, info: Info, as_host: bool) -> Arc<Sitzung> {
        self.close(false);
        let code = info.code.clone();
        let s = Sitzung::new(info, self.ctx.clone());
        s.inner.lock().unwrap().save_info();
        *self.cur.lock().unwrap() = Some(s.clone());
        s.start(as_host);
        write_json(&self.aktiv(), &json!({"code": code}));
        log(format!("Bearbeiten-Sitzung {} {}", fmt_code(&code), if as_host { "erstellt" } else { "beigetreten" }));
        (self.ctx.emit)(SEv::Reload);
        (self.ctx.emit)(SEv::Status);
        s
    }

    pub fn create(&self, title: &str) -> Value {
        let base = self.ctx.karten.active_version();
        if base == 0 { return json!({"ok": false, "why": "Keine gültige Kartenversion vorhanden"}); }
        let code = new_code();
        let title = take_chars(title.trim(), 40);
        let title = if title.is_empty() { format!("Sitzung von {}", (self.ctx.name)()) } else { title };
        self.open(Info { code: code.clone(), title, created: now_ms(), base, term: 1, owner: self.ctx.dev.clone() }, true);
        json!({"ok": true, "code": fmt_code(&code)})
    }

    pub fn join(&self, code: &str) -> Value {
        if !valid_code(code) { return json!({"ok": false, "why": "Der Code hat 9 Zeichen, z. B. K7M-2QX-9PA"}); }
        let code = norm_code(code);
        if self.cur().is_some_and(|s| s.code() == code) { return json!({"ok": true, "code": fmt_code(&code)}); }
        // schon einmal dabei gewesen? Dann mit dem gespeicherten Journal weitermachen
        let info = self.info_of(&code).unwrap_or(Info { code: code.clone(), created: now_ms(), ..Default::default() });
        self.open(info, false);
        json!({"ok": true, "code": fmt_code(&code)})
    }

    pub fn close(&self, clear_active: bool) {
        let Some(c) = self.cur.lock().unwrap().take() else { return };
        c.stop();
        if clear_active {
            let _ = std::fs::remove_file(self.aktiv());
            log(format!("Bearbeiten-Sitzung {} verlassen", fmt_code(&c.code())));
        }
        (self.ctx.emit)(SEv::Reload);
        (self.ctx.emit)(SEv::Status);
    }

    // Sitzungen, in denen dieser PC schon war (zum schnellen Wieder-Beitreten)
    pub fn saved(&self) -> Value {
        let mut out: Vec<(u64, Value)> = Vec::new();
        for e in std::fs::read_dir(&self.ctx.dir).into_iter().flatten().flatten() {
            let Some(code) = e.file_name().to_str().map(String::from) else { continue };
            let Some(info) = self.info_of(&code) else { continue };
            let n = std::fs::read_to_string(e.path().join("ops.jsonl")).map(|t| t.lines().filter(|l| !l.is_empty()).count()).unwrap_or(0);
            out.push((info.created, json!({"code": fmt_code(&info.code), "title": info.title, "created": info.created, "base": info.base, "schritte": n, "mine": info.owner == self.ctx.dev})));
        }
        out.sort_by(|a, b| b.0.cmp(&a.0));
        json!(out.into_iter().take(12).map(|x| x.1).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mqtt::TestBus;
    use std::sync::atomic::Ordering;

    struct Pc {
        net: Arc<Net>,
        sz: Sitzungen,
    }

    fn pc(bus: &Arc<TestBus>, root: &std::path::Path, dev: &str) -> Pc {
        let dir = root.join(dev);
        let net = bus.net(dev);
        let karten = Arc::new(Karten::new(&dir));
        let name = format!("Editor {dev}");
        let ctx = Ctx { net: net.clone(), karten, dev: dev.to_string(), name: Arc::new(move || name.clone()), dir: dir.join("Sitzungen"), emit: Arc::new(|_| {}) };
        Pc { net, sz: Sitzungen::new(ctx) }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fd-test-{name}-{}", random_hex(4)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn until(what: &str, secs: u64, f: impl Fn() -> bool) {
        let end = now_ms() + secs * 1000;
        while !f() {
            assert!(now_ms() < end, "Zeitüberschreitung: {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn road(id: &str, x: f64) -> Value {
        json!({"id": id, "k": "r", "t": "road", "p": [x, 0, x + 100.0, 100]})
    }

    fn st(p: &Pc) -> Value {
        p.sz.cur().unwrap().status()
    }

    fn get(p: &Pc, id: &str) -> Option<Value> {
        p.sz.cur().unwrap().state_ents().into_iter().find(|e| e["id"] == id)
    }

    fn same(pcs: &[&Pc], ids: &[&str]) -> bool {
        let seq = st(pcs[0])["seq"].clone();
        pcs.iter().all(|p| st(p)["seq"] == seq && st(p)["pending"] == 0)
            && ids.iter().all(|id| { let v = get(pcs[0], id).map(|e| strip(&e)); pcs.iter().all(|p| get(p, id).map(|e| strip(&e)) == v) })
    }

    #[test]
    fn drei_editoren_host_wechsel_und_wiedereinstieg() {
        let (bus, root) = (Arc::new(TestBus::default()), tmp("drei"));
        let (a, b, c) = (pc(&bus, &root, "aaaaaaaaaa"), pc(&bus, &root, "bbbbbbbbbb"), pc(&bus, &root, "cccccccccc"));
        let code = a.sz.create("Test")["code"].as_str().unwrap().to_string();
        b.sz.join(&code);
        c.sz.join(&code);
        until("alle sehen den Host", 15, || [&b, &c].iter().all(|p| st(p)["host"] == "aaaaaaaaaa" && st(p)["ready"] == true));
        a.sz.cur().unwrap().submit(&[road("r-a", 0.0)], None);
        b.sz.cur().unwrap().submit(&[road("r-b", 10.0)], None);
        c.sz.cur().unwrap().submit(&[road("r-c", 20.0)], None);
        until("gleicher Stand bei allen", 15, || same(&[&a, &b, &c], &["r-a", "r-b", "r-c"]) && st(&a)["seq"] == 3);
        // Host fällt weg -> einer der anderen übernimmt, es geht weiter
        a.sz.close(false);
        until("neuer Host", 30, || st(&b)["role"] == "host" || st(&c)["role"] == "host");
        c.sz.cur().unwrap().submit(&[road("r-c2", 30.0)], None);
        until("weiter ohne den alten Host", 15, || same(&[&b, &c], &["r-c2"]) && st(&b)["seq"] == 4);
        // der alte Host kommt zurück (Journal von der Platte) und holt nach
        a.sz.join(&code);
        until("Wiedereinstieg", 20, || same(&[&a, &b, &c], &["r-a", "r-b", "r-c", "r-c2"]) && st(&a)["role"] == "member");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn netztrennung_nichts_geht_verloren() {
        let (bus, root) = (Arc::new(TestBus::default()), tmp("trennung"));
        let (a, b) = (pc(&bus, &root, "aaaaaaaaaa"), pc(&bus, &root, "bbbbbbbbbb"));
        let code = a.sz.create("Test")["code"].as_str().unwrap().to_string();
        b.sz.join(&code);
        a.sz.cur().unwrap().submit(&[road("r-1", 0.0)], None);
        until("Start", 15, || same(&[&a, &b], &["r-1"]) && st(&b)["seq"] == 1);
        // getrennt, beide arbeiten weiter (b wird selbst Host)
        b.net.part.store(1, Ordering::Relaxed);
        until("zweiter Host", 30, || st(&b)["role"] == "host");
        a.sz.cur().unwrap().submit(&[road("r-a", 10.0)], None);
        b.sz.cur().unwrap().submit(&[road("r-b", 20.0)], None);
        until("beide getrennt weiter", 10, || st(&a)["seq"] == 2 && st(&b)["seq"] == 2);
        b.net.part.store(0, Ordering::Relaxed);
        until("wieder zusammen, beide Änderungen da", 30, || same(&[&a, &b], &["r-1", "r-a", "r-b"]) && st(&a)["seq"] == 3);
        assert!(get(&a, "r-a").is_some() && get(&a, "r-b").is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rueckgaengig_ueber_kreuz_mit_nachfrage() {
        let (bus, root) = (Arc::new(TestBus::default()), tmp("undo"));
        let (a, b) = (pc(&bus, &root, "aaaaaaaaaa"), pc(&bus, &root, "bbbbbbbbbb"));
        let code = a.sz.create("Test")["code"].as_str().unwrap().to_string();
        b.sz.join(&code);
        until("verbunden", 15, || st(&b)["host"] == "aaaaaaaaaa");
        a.sz.cur().unwrap().submit(&[road("r-x", 0.0)], Some("g1".into()));
        until("1", 10, || same(&[&a, &b], &["r-x"]) && st(&b)["seq"] == 1);
        b.sz.cur().unwrap().submit(&[json!({"id": "r-x", "k": "r", "t": "gravel", "p": [0, 0, 50, 50]})], None);
        until("2", 10, || same(&[&a, &b], &["r-x"]) && st(&a)["seq"] == 2);
        let hist = a.sz.cur().unwrap().history();
        assert_eq!(hist[0]["lb"].as_str().unwrap(), "Straße geändert · Schotter · 0,07 km");
        let key = hist[1]["g"].as_str().unwrap().to_string();
        // B hat die Straße danach geändert -> erst nachfragen
        let r = a.sz.cur().unwrap().undo(&key, false);
        assert_eq!(r["conflict"], json!(["Editor bbbbbbbbbb"]));
        assert_eq!(a.sz.cur().unwrap().undo(&key, true)["ok"], true);
        until("rückgängig bei beiden", 10, || same(&[&a, &b], &["r-x"]) && st(&b)["seq"] == 3);
        assert!(!live(&get(&b, "r-x").unwrap()));
        // Strg+Y: wiederholt -> Stand direkt vor dem Rückgängig (B's Schotter-Version)
        let g = a.sz.cur().unwrap().last_own(true).unwrap();
        assert_eq!(a.sz.cur().unwrap().undo(&g, false)["ok"], true);
        until("wiederholt", 10, || same(&[&a, &b], &["r-x"]) && st(&b)["seq"] == 4);
        assert_eq!(get(&b, "r-x").unwrap()["t"], "gravel");
        let hist = b.sz.cur().unwrap().history();
        assert!(hist[0]["lb"].as_str().unwrap().starts_with("Wiederholt: "));
        // Journal überlebt einen Neustart
        a.sz.close(false);
        let a2 = pc(&bus, &root, "aaaaaaaaaa");
        a2.sz.resume();
        assert_eq!(st(&a2)["seq"], 4);
        assert_eq!(get(&a2, "r-x").unwrap()["t"], "gravel");
        let _ = std::fs::remove_dir_all(root);
    }
}
