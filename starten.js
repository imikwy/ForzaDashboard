// ForzaDashboard starten – startet den Server, installiert bereitliegende Updates und startet bei Bedarf neu.
// Doppelklick auf "ForzaDashboard starten" (Verknüpfung) oder: node starten.js
//
// Updates werden hier installiert (nie während der Server läuft). Startet die neue Version nicht sauber,
// wird automatisch die vorherige wiederhergestellt.
"use strict";
const {spawn} = require("child_process"), fs = require("fs"), path = require("path");
process.title = "ForzaDashboard";
const ROOT = __dirname, DATA = process.env.FD_DATA || path.join(ROOT, "daten"), UPD = path.join(DATA, "updates");
const READY = path.join(UPD, "bereit.json"), LAST = path.join(UPD, "installiert.json");
const ROOT_FILES = ["starten.js", "LIESMICH.txt", "ForzaDashboard-starten.bat", "GamePass-Fix.bat"];
const say = t => console.log("  " + t);
const readJson = f => { try { return JSON.parse(fs.readFileSync(f, "utf8")); } catch (e) { return null; } };
const appVer = dir => (readJson(path.join(dir, "package.json")) || {}).version || "?";

function install(){
  const r = readJson(READY); if (!r) return null;
  try { fs.unlinkSync(READY); } catch (e) {}
  const src = r.dir;
  if (!src || !fs.existsSync(path.join(src, "app", "server.js"))) { say("Update-Paket fehlt oder ist unvollständig – übersprungen"); return null; }
  const old = appVer(path.join(ROOT, "app")), backup = path.join(UPD, "vorher-" + old + "-" + Date.now());
  say("Installiere Update " + r.version + " (vorher " + old + ") ...");
  try {
    fs.mkdirSync(backup, {recursive:true});
    fs.cpSync(path.join(ROOT, "app"), path.join(backup, "app"), {recursive:true});
    for (const f of ROOT_FILES) if (fs.existsSync(path.join(ROOT, f))) fs.copyFileSync(path.join(ROOT, f), path.join(backup, f));
    fs.rmSync(path.join(ROOT, "app"), {recursive:true, force:true});
    fs.cpSync(path.join(src, "app"), path.join(ROOT, "app"), {recursive:true});
    for (const f of ROOT_FILES) if (fs.existsSync(path.join(src, f))) fs.copyFileSync(path.join(src, f), path.join(ROOT, f));
    fs.writeFileSync(LAST, JSON.stringify({version:r.version, vorher:old, sicherung:backup, am:new Date().toISOString()}, null, 1));
    fs.rmSync(src, {recursive:true, force:true});
    // nur die letzten 3 Sicherungen behalten
    const olds = fs.readdirSync(UPD).filter(d => d.startsWith("vorher-")).sort((a, b) => +a.split("-").pop() - +b.split("-").pop());
    for (const d of olds.slice(0, -3)) fs.rmSync(path.join(UPD, d), {recursive:true, force:true});
    say("Update " + r.version + " installiert.");
    return backup;
  } catch (e) {
    say("Update fehlgeschlagen (" + e.message + ") – vorherige Version wird wiederhergestellt");
    restore(backup); return null;
  }
}
function restore(backup){
  try {
    if (!fs.existsSync(path.join(backup, "app", "server.js"))) return;
    fs.rmSync(path.join(ROOT, "app"), {recursive:true, force:true});
    fs.cpSync(path.join(backup, "app"), path.join(ROOT, "app"), {recursive:true});
    for (const f of ROOT_FILES) if (fs.existsSync(path.join(backup, f))) fs.copyFileSync(path.join(backup, f), path.join(ROOT, f));
    say("Vorherige Version " + appVer(path.join(ROOT, "app")) + " wiederhergestellt.");
  } catch (e) { say("Wiederherstellen fehlgeschlagen: " + e.message); }
}

let restarted = !!process.env.FD_RESTARTED, crashes = [];
function run(){
  const backup = install();
  const env = Object.assign({}, process.env, {FD_LOOP:"1"}); if (restarted) env.FD_RESTARTED = "1"; else delete env.FD_RESTARTED;
  const t0 = Date.now();
  const c = spawn(process.execPath, [path.join(ROOT, "app", "server.js")], {stdio:"inherit", env, cwd:ROOT});
  c.on("exit", code => {
    if (code === 42) { restarted = true; say("Neustart ..."); setTimeout(run, 500); return; }
    if (code && backup && Date.now() - t0 < 30000) { say("Die neue Version ist beim Start abgestürzt."); restore(backup); restarted = true; setTimeout(run, 1000); return; }
    if (code) {
      crashes = crashes.filter(t => Date.now() - t < 120000); crashes.push(Date.now());
      if (crashes.length <= 4) { say("ForzaDashboard ist abgestürzt – startet neu ..."); restarted = true; setTimeout(run, 3000); return; }
      say("ForzaDashboard stürzt wiederholt ab. Bitte daten\\logs\\forzadashboard.log an die Herausgeber schicken.");
    }
    console.log("\n  ForzaDashboard wurde beendet. Dieses Fenster kann geschlossen werden.");
    if (process.env.FD_ONCE) process.exit(code || 0);   // für Tests
    setInterval(() => {}, 1 << 30);
  });
}
run();
