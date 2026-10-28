// Umstieg: Brücke zum alten Update-Weg.
// Die bisherigen ForzaDashboard.exe-Versionen (bis 2026-10-28.10) holen Updates nur über den Vermittler, signiert mit
// herausgeber.key. Liegt dieser Schlüssel auf einem Herausgeber-PC im Datenordner, verteilt die neue exe sich dort
// einmal selbst über den alten Weg – so steigt jeder mit der alten exe automatisch auf die neue um.
// Danach kommen alle Updates nur noch über GitHub (updates.rs). Der alte Weg wird von der neuen exe nicht mehr gelesen.
use crate::mqtt::Net;
use crate::util::{gzip, log, now_ms, sha_hex};
use crate::APP_VER;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::Signer;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const UPD_CH: usize = 48000;
const EXE_NAME: &str = "ForzaDashboard.exe";

fn updx_topic() -> String {
    format!("fhdash/updx/{}", &sha_hex(crate::karte::SIG_PUB.as_bytes())[..20])
}

fn own_package() -> Option<&'static (Vec<u8>, String)> {
    static PKG: OnceLock<Option<(Vec<u8>, String)>> = OnceLock::new();
    PKG.get_or_init(|| {
        let exe = std::fs::read(std::env::current_exe().ok()?).ok()?;
        let x = sha_hex(&exe);
        Some((gzip(&exe), x))
    }).as_ref()
}

pub fn start(net: &Arc<Net>, root: &Path) {
    let Some(key) = std::fs::read_to_string(root.join("herausgeber.key")).ok().and_then(|pem| {
        use ed25519_dalek::pkcs8::DecodePrivateKey;
        ed25519_dalek::SigningKey::from_pkcs8_pem(&pem).ok()
    }) else { return };
    log("herausgeber.key gefunden – diese Version wird auch über den alten Update-Weg verteilt (Umstieg der alten exe)");
    let key = Arc::new(key);
    let state = root.join("herausgeber-stand-exe.json");
    let last = Arc::new(Mutex::new(0u64));
    let (n2, k2, s2, l2) = (net.clone(), key.clone(), state.clone(), last.clone());
    // alte exe fragt nach fehlenden Teilen -> erneut verteilen
    net.on_message(Arc::new(move |t: &str, _p: &[u8]| {
        if t != format!("{}/req", updx_topic()) { return; }
        let (n, k, s, l) = (n2.clone(), k2.clone(), s2.clone(), l2.clone());
        std::thread::spawn(move || publish(&n, &k, &s, &l, false));
    }));
    net.subscribe(&format!("{}/req", updx_topic()));
    let n = net.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            if !n.ok() { continue; }
            let (n2, k2, s2, l2) = (n.clone(), key.clone(), state.clone(), last.clone());
            let _ = tokio::task::spawn_blocking(move || publish(&n2, &k2, &s2, &l2, true)).await;
            tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        }
    });
}

fn publish(net: &Net, key: &ed25519_dalek::SigningKey, state_file: &PathBuf, last: &Mutex<u64>, force: bool) {
    {
        let mut l = last.lock().unwrap();
        if !force && now_ms().saturating_sub(*l) < 20000 { return; }
        *l = now_ms();
    }
    let Some((gz, x)) = own_package() else { return };
    let h = sha_hex(gz);
    let c = gz.len().div_ceil(UPD_CH);
    let b = json!({"ver": APP_VER, "ts": now_ms(), "files": [{"n": EXE_NAME, "h": h, "s": gz.len(), "c": c, "x": x}]}).to_string();
    let sig = B64.encode(key.sign(b.as_bytes()).to_bytes());
    let t = updx_topic();
    for (ci, part) in gz.chunks(UPD_CH).enumerate() { net.publish(&format!("{t}/d/{}/{ci}", &h[..16]), part, true); }
    net.publish(&format!("{t}/m"), json!({"b": b, "s": sig}).to_string().as_bytes(), true);
    // alte Teile beim Vermittler löschen
    let old: Vec<serde_json::Value> = std::fs::read_to_string(state_file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for o in old {
        let (Some(oh), Some(oc)) = (o["h"].as_str(), o["c"].as_u64()) else { continue };
        if oh == &h[..16] { continue; }
        for ci in 0..oc { net.publish(&format!("{t}/d/{oh}/{ci}"), &[], true); }
    }
    let _ = std::fs::write(state_file, json!([{"h": &h[..16], "c": c}]).to_string());
    log(format!("Version {APP_VER} über den alten Update-Weg verteilt ({c} Teile)"));
}
