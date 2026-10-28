// Die Karte: EINE gemeinsame, geschützte Quelle der Wahrheit.
// - Jede Kartenversion ist von den Herausgebern signiert (gleicher Schlüssel wie die Programm-Updates) und liegt als
//   eigene, schreibgeschützte Datei in Kartenversionen\ (karte-v12.json + Manifest). Nichts überschreibt eine Version.
// - Eine Version wird nur übernommen, wenn Signatur, Prüfsumme und Inhalt stimmen. Die letzten 10 bleiben liegen.
// - Alles, was die Straßen beeinflusst, steht in der Version: gezeichnete Straßen, Radierungen, Ausblendungen,
//   Kartenausrichtung (cfg:mapfit) und Straßenkorrekturen (cfg:roadfix). Darum sieht jeder PC dieselbe Karte.
//
// Objekte: {id, k, …, ts, a}
//   r = Straße {t, lv, sm, p:[x,z,x,z,…]}      o = Ort {kind, n, x, z}      e = Radier-Kreis {x, z, r}
//   x = ausgeblendet ("x:" + Ziel)             c = Einstellung ("cfg:…")    d:1 = gelöscht (Grabstein)
use crate::util::{log, now_ms, read_json, sha_hex, ver_newer, write_atomic, write_json};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::Verifier;
use flate2::read::GzDecoder;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// öffentlicher Schlüssel der Herausgeber – nur was damit signiert ist, wird als Karte oder Programm übernommen
pub const SIG_PUB: &str = "MCowBQYDK2VwAyEARFw+U6bmc7q6IDkl02pwPx2CWif6zv+oSYvhpUlze7Y=";
const KEEP: usize = 10;
static START_MAP: &[u8] = include_bytes!("../karte-start.json.gz");
static START_MAN: &str = include_str!("../karte-start.manifest.json");
static FH6_KARTE: &[u8] = include_bytes!("../../src/fh6-karte.js");

// Prüfsumme der eingebauten Forza-Straßen: eine Karte passt nur zu genau diesem Straßennetz
pub fn base_hash() -> String {
    sha_hex(FH6_KARTE)[..16].to_string()
}

// ---------------- Prüfung und einheitliches Format ----------------
fn fin(v: &Value) -> bool {
    v.as_f64().is_some_and(|f| f.is_finite() && f.abs() < 30000.0)
}

pub fn live(e: &Value) -> bool {
    !e.is_null() && e["d"].as_u64() != Some(1)
}

pub fn check_ent(e: &Value) -> Option<String> {
    let Some(o) = e.as_object() else { return Some("kein Objekt".into()) };
    let id = o.get("id").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() || id.len() > 100 { return Some("ungültige id".into()); }
    let k = o.get("k").and_then(Value::as_str).unwrap_or("");
    if !matches!(k, "r" | "o" | "e" | "x" | "c") { return Some(format!("unbekannte Art {k}")); }
    if let Some(ts) = o.get("ts") { if !ts.as_f64().is_some_and(|t| t >= 0.0) { return Some("ungültige Zeit".into()); } }
    if o.get("d").and_then(Value::as_u64) == Some(1) { return None; }
    match k {
        "r" => {
            let ok = o.get("p").and_then(Value::as_array).is_some_and(|p| p.len() >= 4 && p.len() % 2 == 0 && p.len() <= 40000 && p.iter().all(fin));
            if !ok { return Some("Straße ohne gültige Punkte".into()); }
            if let Some(t) = o.get("t") { if !matches!(t.as_str(), Some("motor" | "road" | "gravel" | "off" | "tunnel")) { return Some(format!("unbekannter Straßentyp {t}")); } }
            if let Some(lv) = o.get("lv") { if !matches!(lv.as_i64(), Some(-1..=2)) { return Some("ungültige Ebene".into()); } }
        }
        "o" => {
            if !o.get("x").is_some_and(fin) || !o.get("z").is_some_and(fin) { return Some("Ort ohne Position".into()); }
            if let Some(n) = o.get("n") { if !n.as_str().is_some_and(|n| n.chars().count() <= 60) { return Some("Ortsname zu lang".into()); } }
        }
        "e" => {
            let r = o.get("r").and_then(Value::as_f64).unwrap_or(0.0);
            if !o.get("x").is_some_and(fin) || !o.get("z").is_some_and(fin) || !(r > 0.0 && r <= 500.0) { return Some("ungültiger Radier-Kreis".into()); }
        }
        "x" => { if !id.starts_with("x:") || id.len() < 3 { return Some("Ausblendung ohne Ziel".into()); } }
        _ => {
            if !id.starts_with("cfg:") { return Some("Einstellung ohne cfg:-Kennung".into()); }
            if id == "cfg:mapfit" && !o.get("T").and_then(Value::as_array).is_some_and(|t| t.len() == 6 && t.iter().all(|v| v.as_f64().is_some_and(f64::is_finite))) {
                return Some("ungültige Kartenausrichtung".into());
            }
            if id == "cfg:roadfix" && !o.get("lines").is_some_and(Value::is_object) { return Some("ungültige Straßenkorrekturen".into()); }
        }
    }
    None
}

// gerundet wie Math.round in JavaScript (die Werkzeuge auf GitHub rechnen genauso)
fn r1(v: &Value) -> Value {
    let Some(f) = v.as_f64() else { return v.clone() };
    let r = (f * 10.0 + 0.5).floor() / 10.0;
    if r.fract() == 0.0 && r.abs() < 9e15 { json!(r as i64) } else { json!(r) }
}

// feste Reihenfolge der Felder, Punkte auf 0,1 m gerundet -> gleiche Karte = gleiche Bytes
pub fn canon_ent(e: &Value) -> Value {
    let Some(o) = e.as_object() else { return e.clone() };
    let mut out = Map::new();
    out.insert("id".into(), o.get("id").cloned().unwrap_or(Value::Null));
    out.insert("k".into(), o.get("k").cloned().unwrap_or(Value::Null));
    let rest: BTreeMap<&String, &Value> = o.iter().filter(|(k, _)| !matches!(k.as_str(), "id" | "k" | "ts" | "a")).collect();
    for (k, v) in rest {
        let v = if k == "p" { v.as_array().map(|a| Value::Array(a.iter().map(r1).collect())).unwrap_or_else(|| v.clone()) } else { v.clone() };
        out.insert(k.clone(), v);
    }
    if let Some(ts) = o.get("ts") { out.insert("ts".into(), ts.clone()); }
    if let Some(a) = o.get("a") { out.insert("a".into(), a.clone()); }
    Value::Object(out)
}

pub fn strip(e: &Value) -> Value {
    let mut e = e.clone();
    if let Some(o) = e.as_object_mut() { o.remove("ts"); o.remove("a"); }
    e
}

fn id_of(e: &Value) -> &str {
    e["id"].as_str().unwrap_or("")
}

// Datei: eine Zeile pro Objekt – gut lesbar, und Git zeigt genau, was sich geändert hat
pub fn serialize(ents: &[Value]) -> String {
    let mut list: Vec<&Value> = ents.iter().collect();
    list.sort_by(|a, b| id_of(a).cmp(id_of(b)));
    let lines: Vec<String> = list.iter().map(|e| canon_ent(e).to_string()).collect();
    format!("{{\"format\":\"fd-karte\",\"schema\":1,\"ents\":[\n{}\n]}}\n", lines.join(",\n"))
}

pub fn parse(text: &str) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "Karten-Datei nicht lesbar".to_string())?;
    v.get("ents").and_then(Value::as_array).cloned().ok_or_else(|| "keine Karten-Datei".to_string())
}

pub struct Check {
    pub ok: bool,
    pub errors: Vec<String>,
    pub stats: Value,
}

pub fn validate(ents: &[Value]) -> Check {
    let mut errors = Vec::new();
    let mut ids = HashSet::new();
    let mut st: BTreeMap<&str, u64> = ["gesamt", "aktiv", "strassen", "orte", "radiert", "ausgeblendet", "einstellungen", "geloescht"].into_iter().map(|k| (k, 0)).collect();
    for e in ents {
        if let Some(err) = check_ent(e) { if errors.len() < 50 { errors.push(format!("{}: {err}", e["id"].as_str().unwrap_or("?"))); } continue; }
        if !ids.insert(id_of(e).to_string()) { if errors.len() < 50 { errors.push(format!("{}: doppelt", id_of(e))); } continue; }
        *st.get_mut("gesamt").unwrap() += 1;
        if !live(e) { *st.get_mut("geloescht").unwrap() += 1; continue; }
        *st.get_mut("aktiv").unwrap() += 1;
        let k = match e["k"].as_str() { Some("r") => "strassen", Some("o") => "orte", Some("e") => "radiert", Some("x") => "ausgeblendet", _ => "einstellungen" };
        *st.get_mut(k).unwrap() += 1;
    }
    Check { ok: errors.is_empty(), errors, stats: json!(st) }
}

// Was ändert sich von alt -> neu? (Zusammenfassung vor dem Veröffentlichen, Schutz vor Massenlöschung)
pub fn diff(old: &[Value], new: &[Value]) -> Value {
    let a: HashMap<&str, &Value> = old.iter().map(|e| (id_of(e), e)).collect();
    let b: HashMap<&str, &Value> = new.iter().map(|e| (id_of(e), e)).collect();
    let (mut neu, mut ge, mut del, mut weg) = (0u64, 0u64, 0u64, 0u64);
    let mut by: BTreeMap<String, [u64; 3]> = BTreeMap::new();
    let same = |x: &Value, y: &Value| canon_ent(&strip(x)) == canon_ent(&strip(y));
    for (id, e) in &b {
        let k = e["k"].as_str().unwrap_or("?").to_string();
        let old = a.get(id).copied();
        let (al, bl) = (old.is_some_and(live), live(e));
        if !al && bl { neu += 1; by.entry(k).or_default()[0] += 1; }
        else if al && !bl { del += 1; by.entry(k).or_default()[2] += 1; }
        else if al && bl && !same(old.unwrap(), e) { ge += 1; by.entry(k).or_default()[1] += 1; }
    }
    for (id, e) in &a {
        if b.contains_key(id) { continue; }
        weg += 1;
        if live(e) { del += 1; by.entry(e["k"].as_str().unwrap_or("?").to_string()).or_default()[2] += 1; }
    }
    let live_old = old.iter().filter(|e| live(e)).count() as f64;
    let nach: Map<String, Value> = by.into_iter().map(|(k, v)| (k, json!({"neu": v[0], "geaendert": v[1], "geloescht": v[2]}))).collect();
    json!({"neu": neu, "geaendert": ge, "geloescht": del, "verschwunden": weg, "nachArt": nach,
           "anteilGeloescht": if live_old > 0.0 { del as f64 / live_old } else { 0.0 }})
}

// Schutzregel: mehr als 5 % (mind. 200 Objekte) auf einmal gelöscht oder Objekte ganz verschwunden -> nur mit Freigabe
pub fn guard(d: &Value) -> Vec<String> {
    let mut p = Vec::new();
    let weg = d["verschwunden"].as_u64().unwrap_or(0);
    let del = d["geloescht"].as_u64().unwrap_or(0);
    let share = d["anteilGeloescht"].as_f64().unwrap_or(0.0);
    if weg > 0 { p.push(format!("{weg} Objekte fehlen komplett (Grabsteine müssen erhalten bleiben)")); }
    if del > 200 && share > 0.05 { p.push(format!("{del} Objekte gelöscht ({} %) – das sieht nach einem Versehen aus", (share * 100.0).round())); }
    p
}

// gewinnt b gegen a? (neuere Änderung; bei Gleichstand der größere Autor)
pub fn newer(b: &Value, a: &Value) -> bool {
    let (tb, ta) = (b["ts"].as_f64().unwrap_or(0.0), a["ts"].as_f64().unwrap_or(0.0));
    tb > ta || (tb == ta && b["a"].as_str().unwrap_or("") > a["a"].as_str().unwrap_or(""))
}

pub fn merge(a: &[Value], b: &[Value]) -> Vec<Value> {
    let mut m: BTreeMap<String, Value> = a.iter().map(|e| (id_of(e).to_string(), e.clone())).collect();
    for e in b {
        let id = id_of(e).to_string();
        if m.get(&id).is_none_or(|c| newer(e, c)) { m.insert(id, e.clone()); }
    }
    m.into_values().collect()
}

// ---------------- Signatur ----------------
// Manifest-Datei: {"b": "<JSON-Text>", "s": "<Signatur über genau diesen Text>"}
pub fn verify_signed(raw: &str) -> Result<Value, String> {
    let m: Value = serde_json::from_str(raw).map_err(|_| "Manifest unlesbar")?;
    let (Some(b), Some(s)) = (m["b"].as_str(), m["s"].as_str()) else { return Err("Manifest ohne Signatur".into()) };
    let der = B64.decode(SIG_PUB).map_err(|_| "Schlüssel")?;
    let raw_key: [u8; 32] = der[der.len() - 32..].try_into().map_err(|_| "Schlüssel")?;
    let pk = ed25519_dalek::VerifyingKey::from_bytes(&raw_key).map_err(|_| "Schlüssel")?;
    let sig = B64.decode(s).ok().and_then(|s| ed25519_dalek::Signature::from_slice(&s).ok()).ok_or("Signatur unlesbar")?;
    pk.verify(b.as_bytes(), &sig).map_err(|_| "Signatur ungültig – stammt nicht von den Herausgebern".to_string())?;
    serde_json::from_str(b).map_err(|_| "Manifest-Inhalt unlesbar".into())
}

// Prüft Manifest + Inhalt einer Kartenversion. Ok = (Manifest-Inhalt, Objekte)
pub fn check_release(raw_man: &str, buf: &[u8]) -> Result<(Value, Vec<Value>), String> {
    let man = verify_signed(raw_man)?;
    let v = man["version"].as_u64().unwrap_or(0);
    if man["typ"] != "karte" || v == 0 { return Err("Manifest ungültig".into()); }
    if man["sha256"].as_str() != Some(&sha_hex(buf)) { return Err("Prüfsumme stimmt nicht – Datei beschädigt".into()); }
    if let Some(min) = man["minApp"].as_str() {
        if ver_newer(min, crate::APP_VER) { return Err(format!("Karte braucht Programmversion {min} (du hast {})", crate::APP_VER)); }
    }
    let ents = parse(std::str::from_utf8(buf).map_err(|_| "Karten-Datei nicht lesbar")?)?;
    let c = validate(&ents);
    if !c.ok { return Err(format!("Karte fehlerhaft: {}", c.errors.iter().take(3).cloned().collect::<Vec<_>>().join("; "))); }
    if ents.len() as u64 != man["objekte"].as_u64().unwrap_or(u64::MAX) { return Err("Anzahl der Objekte stimmt nicht".into()); }
    Ok((man, ents))
}

pub fn gunzip_bytes(b: &[u8], max: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    GzDecoder::new(b).take(max).read_to_end(&mut out).ok()?;
    Some(out)
}

// ---------------- Kartenversionen auf diesem PC ----------------
pub struct Version {
    pub version: u64,
    pub man: Value,
    pub ents: Arc<Vec<Value>>,
    pub by_id: HashMap<String, usize>,
}

impl Version {
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.by_id.get(id).map(|i| &self.ents[*i])
    }
}

pub struct Karten {
    dir: PathBuf,
    active: Mutex<Option<Arc<Version>>>,
}

impl Karten {
    pub fn new(root: &Path) -> Karten {
        let dir = root.join("Kartenversionen");
        let _ = std::fs::create_dir_all(&dir);
        let k = Karten { dir, active: Mutex::new(None) };
        let want = read_json(&k.dir.join("aktiv.json")).and_then(|a| a["version"].as_u64());
        let mut order: Vec<u64> = want.into_iter().collect();
        for v in k.versions().into_iter().rev() { if !order.contains(&v) { order.push(v); } }
        for v in order {
            if k.load(v) { break; }
            log(format!("Kartenversion {v} beschädigt – nehme die nächste"));
        }
        // eingebaute Startkarte (neue Installation, alles beschädigt, oder das Programm bringt eine neuere mit)
        if let Some(buf) = gunzip_bytes(START_MAP, 256 << 20) {
            let v = verify_signed(START_MAN).ok().and_then(|m| m["version"].as_u64()).unwrap_or(0);
            if v > k.active_version() {
                if let Err(e) = k.install(START_MAN, &buf, "Programm") { log(format!("Startkarte ungültig: {e}")); }
            }
        }
        if k.active().is_none() { log("WARNUNG: keine gültige Karte vorhanden"); }
        k
    }

    fn file(&self, v: u64) -> PathBuf {
        self.dir.join(format!("karte-v{v}.json"))
    }

    fn man_file(&self, v: u64) -> PathBuf {
        self.dir.join(format!("karte-v{v}.manifest.json"))
    }

    pub fn versions(&self) -> Vec<u64> {
        let mut out: Vec<u64> = std::fs::read_dir(&self.dir).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_prefix("karte-v")?.strip_suffix(".json")?.parse().ok())
            .collect();
        out.sort();
        out
    }

    pub fn active(&self) -> Option<Arc<Version>> {
        self.active.lock().unwrap().clone()
    }

    pub fn active_version(&self) -> u64 {
        self.active().map_or(0, |a| a.version)
    }

    fn read(&self, v: u64) -> Result<(Value, Vec<Value>), String> {
        let raw = std::fs::read_to_string(self.man_file(v)).map_err(|_| "Manifest fehlt")?;
        let buf = std::fs::read(self.file(v)).map_err(|_| "Datei fehlt")?;
        check_release(&raw, &buf)
    }

    fn load(&self, v: u64) -> bool {
        match self.read(v) {
            Ok((man, ents)) => {
                let by_id = ents.iter().enumerate().map(|(i, e)| (id_of(e).to_string(), i)).collect();
                *self.active.lock().unwrap() = Some(Arc::new(Version { version: v, man, ents: Arc::new(ents), by_id }));
                true
            }
            Err(e) => { log(format!("Karte v{v}: {e}")); false }
        }
    }

    // neue Version übernehmen: prüfen, schreibgeschützt ablegen, dann umschalten. Ok(true) = jetzt aktiv
    pub fn install(&self, raw_man: &str, buf: &[u8], source: &str) -> Result<bool, String> {
        let (man, _) = check_release(raw_man, buf)?;
        let v = man["version"].as_u64().unwrap_or(0);
        let f = self.file(v);
        let have = std::fs::read(&f).ok();
        if have.as_deref().map(sha_hex) != Some(sha_hex(buf)) {
            if have.is_some() {
                log(format!("Karte v{v} war beschädigt und wird repariert"));
                set_readonly(&f, false);
            }
            write_atomic(&f, buf).map_err(|e| e.to_string())?;
            write_atomic(&self.man_file(v), raw_man.as_bytes()).map_err(|e| e.to_string())?;
            set_readonly(&f, true);
        }
        let mut activated = false;
        if v >= self.active_version() {
            if !self.load(v) { return Err("Version konnte nicht geladen werden".into()); }
            write_json(&self.dir.join("aktiv.json"), &json!({"version": v, "seit": now_ms(), "quelle": source}));
            log(format!("Karte Version {v} aktiv ({} Objekte, aus {source})", man["objekte"]));
            activated = true;
        }
        self.prune();
        Ok(activated)
    }

    // auf eine ältere Version zurück (nur auf diesem PC)
    pub fn activate(&self, v: u64) -> Result<(), String> {
        if !self.load(v) { return Err(format!("Version {v} nicht vorhanden oder beschädigt")); }
        write_json(&self.dir.join("aktiv.json"), &json!({"version": v, "seit": now_ms(), "quelle": "von Hand"}));
        Ok(())
    }

    fn prune(&self) {
        let vs = self.versions();
        let cur = self.active_version();
        for v in vs.iter().take(vs.len().saturating_sub(KEEP)) {
            if *v == cur { continue; }
            for f in [self.file(*v), self.man_file(*v)] { set_readonly(&f, false); let _ = std::fs::remove_file(f); }
        }
    }

    // Objekte einer bestimmten (auch älteren) Version – für Sitzungen, die auf dieser Version aufbauen
    pub fn ents(&self, v: u64) -> Option<Arc<Vec<Value>>> {
        if let Some(a) = self.active() { if a.version == v { return Some(a.ents.clone()); } }
        self.read(v).ok().map(|(_, e)| Arc::new(e))
    }

    // Release-Dateien einer Version (für Sitzungs-Teilnehmer, denen sie fehlt)
    pub fn raw(&self, v: u64) -> Option<(String, Vec<u8>)> {
        Some((std::fs::read_to_string(self.man_file(v)).ok()?, std::fs::read(self.file(v)).ok()?))
    }

    pub fn has(&self, v: u64) -> bool {
        self.file(v).exists() && self.man_file(v).exists()
    }

    pub fn info(&self) -> Value {
        match self.active() {
            Some(a) => json!({"version": a.version, "objekte": a.ents.len(), "erstellt": a.man["erstellt"], "notiz": a.man["notiz"],
                "signiert": true, "basisOk": a.man["basis"].as_str().is_none_or(|b| b == base_hash()), "versionen": self.versions()}),
            None => json!({"version": 0, "objekte": 0, "versionen": []}),
        }
    }
}

fn set_readonly(f: &Path, ro: bool) {
    if let Ok(m) = std::fs::metadata(f) {
        let mut p = m.permissions();
        p.set_readonly(ro);
        let _ = std::fs::set_permissions(f, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canon_rounds_and_orders() {
        let e = json!({"z": 1, "p": [1.04, 2.25, -3.05, 4.0], "id": "r1", "ts": 5, "k": "r", "a": "x"});
        assert_eq!(canon_ent(&e).to_string(), r#"{"id":"r1","k":"r","p":[1,2.3,-3,4],"z":1,"ts":5,"a":"x"}"#);
    }

    #[test]
    fn checks() {
        assert!(check_ent(&json!({"id": "r1", "k": "r", "p": [0, 0, 1, 1]})).is_none());
        assert!(check_ent(&json!({"id": "r1", "k": "r", "p": [0, 0]})).is_some());
        assert!(check_ent(&json!({"id": "r1", "k": "r", "d": 1})).is_none());
        assert!(check_ent(&json!({"id": "cfg:mapfit", "k": "c", "T": [1, 2, 3]})).is_some());
        assert!(check_ent(&json!({"id": "e1", "k": "e", "x": 1, "z": 2, "r": 600})).is_some());
    }

    #[test]
    fn guard_blocks_mass_delete() {
        let old: Vec<Value> = (0..1000).map(|i| json!({"id": format!("r{i}"), "k": "r", "p": [0, 0, 1, 1]})).collect();
        let mut new = old.clone();
        for e in new.iter_mut().take(300) { *e = json!({"id": e["id"], "k": "r", "d": 1}); }
        assert_eq!(guard(&diff(&old, &new)).len(), 1);
        assert_eq!(guard(&diff(&old, &old[..999])).len(), 1);
        assert!(guard(&diff(&old, &new[..0].iter().chain(old.iter()).cloned().collect::<Vec<_>>())).is_empty());
    }

    #[test]
    fn startkarte_ist_gueltig() {
        let buf = gunzip_bytes(START_MAP, 256 << 20).unwrap();
        let (man, ents) = check_release(START_MAN, &buf).unwrap();
        assert_eq!(man["version"], 1);
        assert_eq!(man["basis"].as_str().unwrap(), base_hash());
        assert!(ents.len() > 10000);
    }
}
