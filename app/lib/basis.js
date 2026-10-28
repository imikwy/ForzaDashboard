// Pfade, Einstellungen und Geräte-Kennung.
"use strict";
const fs = require("fs"), path = require("path"), os = require("os"), crypto = require("crypto"), {execFileSync} = require("child_process");
const {readJson, writeJson, writeAtomic, log} = require("./util");

// ---------------- Pfade ----------------
// Programm (wird bei Updates ersetzt) und Daten (werden nie von einem Update angefasst) sind getrennt.
const ROOT = process.env.FD_ROOT || path.resolve(__dirname, "..", "..");
const APP = path.join(ROOT, "app");
const DATA = process.env.FD_DATA || path.join(ROOT, "daten");
const P = {
  ROOT, APP, DATA,
  PUBLIC: path.join(APP, "public"),
  SETTINGS: path.join(DATA, "einstellungen.txt"),
  KARTEN: path.join(DATA, "karten"),            // geprüfte, unveränderliche Kartenversionen
  SITZUNGEN: path.join(DATA, "sitzungen"),      // Bearbeiten-Sitzungen (Journal)
  LOBBYS: path.join(DATA, "lobbys.json"),
  LOKAL: path.join(DATA, "karte-lokal.json"),   // nur dieser PC: Tacho-Zuordnung, eigene Ausrichtung ohne Karte
  LEARNED: path.join(DATA, "gelernte-strassen.json"),
  LEARNED_VER: path.join(DATA, "gelernte-strassen.version"),
  FRIENDS_LR: path.join(DATA, "freunde-strassen.json"),
  BACKUP: path.join(DATA, "sicherungen"),
  LOG: path.join(DATA, "logs", "forzadashboard.log"),
  UPDATES: path.join(DATA, "updates"),
  START_MAP: path.join(ROOT, "karte", "karte.json"),     // im Repo: Quelle der Wahrheit (nur Herausgeber-PCs)
  BUNDLED_MAP: path.join(APP, "karte-start.json.gz"),    // mit dem Programm ausgeliefert: Startkarte für neue Installationen
  CONFIG: path.join(APP, "config.json"),
  LOCAL_ROOT: process.env.LOCALAPPDATA ? path.join(process.env.LOCALAPPDATA, "ForzaDashboard") : path.join(os.homedir(), ".forzadashboard"),
};
fs.mkdirSync(DATA, {recursive:true});

const PKG = readJson(path.join(APP, "package.json"), {version:"0.0.0"});
const VERSION = PKG.version;
const CONFIG = Object.assign({repo:"", signaturSchluessel:"", brokers:[]}, readJson(P.CONFIG, {}));

// ---------------- Einstellungen (einstellungen.txt, von Hand lesbar) ----------------
const DEFAULTS = {udp_port:5400, web_port:8080, beim_start_oeffnen:true, name:"", freunde:true, spotify_lautstaerke:true, updates:true, update_repo:""};
const S = Object.assign({}, DEFAULTS);
const yes = v => !["nein", "0", "false", "aus", "off", "no"].includes(String(v).trim().toLowerCase());
function loadSettings(){
  if (!fs.existsSync(P.SETTINGS)) {
    writeAtomic(P.SETTINGS, "# ForzaDashboard Einstellungen\r\n# Im Spiel: Einstellungen > HUD und Gameplay > Data Out = An,\r\n# Data Out IP-Adresse = 127.0.0.1, Data Out IP-Port = gleicher Wert wie udp_port\r\nudp_port=5400\r\nweb_port=8080\r\nbeim_start_oeffnen=ja\r\n");
  }
  for (const raw of fs.readFileSync(P.SETTINGS, "utf8").split(/\r?\n/)) {
    const line = raw.trim(); if (!line || line.startsWith("#")) continue;
    const eq = line.indexOf("="); if (eq < 0) continue;
    const k = line.slice(0, eq).trim().toLowerCase(), v = line.slice(eq + 1).trim();
    if (k === "udp_port" || k === "web_port") { if (+v > 0 && +v < 65536) S[k] = +v; }
    else if (["beim_start_oeffnen", "freunde", "spotify_lautstaerke", "updates"].includes(k)) S[k] = yes(v);
    else if (k === "name") S.name = v.slice(0, 24);
    else if (k === "update_repo" && /^[\w.-]+\/[\w.-]+$/.test(v)) S.update_repo = v;
  }
  if (!S.name) { try { S.name = (os.userInfo().username || "").slice(0, 24); } catch (e) {} }
  if (!S.name) S.name = "Fahrer";
}
function setSetting(key, val){
  try {
    let t = fs.readFileSync(P.SETTINGS, "utf8");
    const re = new RegExp("^" + key + "\\s*=.*$", "m"), v = typeof val === "boolean" ? (val ? "ja" : "nein") : String(val);
    t = re.test(t) ? t.replace(re, key + "=" + v) : t.replace(/\s*$/, "") + "\r\n" + key + "=" + v + "\r\n";
    writeAtomic(P.SETTINGS, t);
    S[key] = val;
  } catch (e) { log("Einstellung konnte nicht gespeichert werden:", e.message); }
}
loadSettings();

// ---------------- Geräte-Kennung ----------------
// Früher stand die Kennung in einstellungen.txt – wer den Ordner kopiert hat, hatte dieselbe Kennung wie der
// Absender, und die Programme hielten die Änderungen des anderen für ihre eigenen. Jetzt liegt sie im
// Benutzerprofil dieses PCs (nicht im Ordner) und ist an den PC gebunden. Kopierte Ordner bekommen automatisch eine neue.
function machineFingerprint(){
  let m = "";
  if (process.platform === "win32") {
    try { m = /MachineGuid\s+REG_SZ\s+(\S+)/.exec(execFileSync("reg", ["query", "HKLM\\SOFTWARE\\Microsoft\\Cryptography", "/v", "MachineGuid"], {encoding:"utf8", windowsHide:true, timeout:3000}))[1]; } catch (e) {}
  }
  if (!m) m = os.hostname() + "|" + (os.userInfo().username || "");
  return crypto.createHash("sha256").update("fd-geraet:" + m).digest("hex").slice(0, 16);
}
function loadIdentity(){
  const file = path.join(P.LOCAL_ROOT, "geraet.json"), fp = machineFingerprint();
  let o = readJson(file, null);
  if (!o || !/^[0-9a-f]{10}$/.test(o.id) || o.machine !== fp) {
    o = {id:crypto.randomBytes(5).toString("hex"), machine:fp, created:new Date().toISOString()};
    try { writeJson(file, o, true); } catch (e) {}
    log("Neue Geräte-Kennung für diesen PC:", o.id);
  }
  return o.id;
}
const DEVICE_ID = process.env.FD_DEVICE_ID || loadIdentity();

function updateRepo(){ return S.update_repo || CONFIG.repo || ""; }

module.exports = {P, S, VERSION, CONFIG, DEVICE_ID, setSetting, updateRepo};
