// Forza "Data Out"-Pakete lesen (Forza Horizon und Forza Motorsport)
use serde_json::{json, Map, Value};

fn r4(v: f32) -> f64 {
    let v = v as f64;
    if v.is_finite() { (v * 10000.0).round() / 10000.0 } else { 0.0 }
}

pub struct Packet {
    pub json: Value,
    pub on: bool,
    pub dash: bool,
    pub px: f64,
    pub pz: f64,
    pub yaw: f64,
    pub spd: f64,
    pub car: i64,
}

pub fn parse(b: &[u8]) -> Packet {
    let raw4 = |o: usize| -> [u8; 4] { b.get(o..o + 4).and_then(|s| s.try_into().ok()).unwrap_or([0; 4]) };
    let f = |o: usize| r4(f32::from_le_bytes(raw4(o)));
    let i = |o: usize| i32::from_le_bytes(raw4(o)) as i64;
    let u = |o: usize| u32::from_le_bytes(raw4(o)) as u64;
    let byte = |o: usize| b.get(o).copied().unwrap_or(0);
    let a4 = |o: usize| json!([f(o), f(o + 4), f(o + 8), f(o + 12)]);
    let i4 = |o: usize| json!([i(o), i(o + 4), i(o + 8), i(o + 12)]);
    let len = b.len();

    let mut d = Map::new();
    let mut put = |k: &str, v: Value| { d.insert(k.into(), v); };
    put("len", json!(len)); put("on", json!(i(0))); put("ts", json!(u(4)));
    put("rpmMax", json!(f(8))); put("rpmIdle", json!(f(12))); put("rpm", json!(f(16)));
    put("ax", json!(f(20))); put("ay", json!(f(24))); put("az", json!(f(28)));
    put("vx", json!(f(32))); put("vy", json!(f(36))); put("vz", json!(f(40)));
    put("yaw", json!(f(56))); put("pitch", json!(f(60))); put("roll", json!(f(64)));
    put("susp", a4(68)); put("slipR", a4(84)); put("slipA", a4(164)); put("slip", a4(180));
    put("wrs", a4(100)); put("surf", a4(148)); put("suspM", a4(196));
    put("avx", json!(f(44))); put("avy", json!(f(48))); put("avz", json!(f(52)));
    put("rumble", i4(116));
    put("car", json!(i(212))); put("cls", json!(i(216))); put("pi", json!(i(220)));
    put("drive", json!(i(224))); put("cyl", json!(i(228)));

    let fh = len >= 323 && len != 331;
    if fh {
        put("puddle", i4(132)); put("grp", json!(u(232))); put("smV", json!(f(236))); put("smM", json!(f(240)));
    } else {
        put("puddle", a4(132));
    }
    let o = if fh { Some(244) } else if len >= 311 { Some(232) } else { None };
    let (mut px, mut pz, mut spd) = (0.0, 0.0, 0.0);
    if let Some(o) = o {
        px = f(o); pz = f(o + 8); spd = f(o + 12);
        put("px", json!(px)); put("py", json!(f(o + 4))); put("pz", json!(pz)); put("spd", json!(spd));
        put("pow", json!(f(o + 16))); put("tq", json!(f(o + 20))); put("tt", a4(o + 24));
        put("boost", json!(f(o + 40))); put("fuel", json!(f(o + 44))); put("dist", json!(f(o + 48)));
        put("best", json!(f(o + 52))); put("last", json!(f(o + 56))); put("cur", json!(f(o + 60))); put("rt", json!(f(o + 64)));
        put("lap", json!(u16::from_le_bytes([byte(o + 68), byte(o + 69)])));
        put("pos", json!(byte(o + 70))); put("acc", json!(byte(o + 71))); put("brk", json!(byte(o + 72)));
        put("clu", json!(byte(o + 73))); put("hb", json!(byte(o + 74))); put("gear", json!(byte(o + 75)));
        put("steer", json!(byte(o + 76) as i8)); put("dash", json!(1));
        if len > o + 78 { put("line", json!(byte(o + 77) as i8)); put("aiBrake", json!(byte(o + 78) as i8)); }
        if o == 232 && len >= 327 { put("wear", a4(311)); } // Forza Motorsport (2023): Reifenverschleiß
    } else {
        put("dash", json!(0));
    }
    Packet { on: i(0) != 0, dash: o.is_some(), px, pz, yaw: f(56), spd, car: i(212), json: Value::Object(d) }
}
