// Veröffentlichen: Stand einer Bearbeiten-Sitzung wird zur neuen Kartenversion (nur auf Herausgeber-PCs).
//
// Ablauf (sicher, ohne den Arbeitsordner anzufassen):
// 1. neuesten Stand von GitHub holen (origin/main:karte/karte.json) – das ist die aktuelle Wahrheit
// 2. Sitzungs-Stand damit zusammenführen (pro Objekt gewinnt die neuere Änderung -> nichts von anderen geht verloren)
// 3. prüfen (gültig? Massenlöschung?) und als Commit direkt auf main schieben
// 4. GitHub Actions prüft erneut, signiert und erstellt das Release "karte-vN" -> alle Spieler laden es automatisch
// Schreibrechte auf das Repo haben nur die Herausgeber – sonst kann niemand eine Kartenversion veröffentlichen.
"use strict";
const fs = require("fs"), path = require("path"), os = require("os"), {execFile, execFileSync} = require("child_process");
const {P} = require("./basis");
const {log} = require("./util");
const K = require("./kartenpruefung");

const FILE = "karte/karte.json";
let cap = null;
function git(args, opts){
  return new Promise((resolve, reject) => execFile("git", ["-C", P.ROOT, ...args], Object.assign({windowsHide:true, timeout:120000, maxBuffer:256 << 20}, opts || {}), (err, out, errOut) => err ? reject(new Error((errOut || err.message).trim())) : resolve(String(out))));
}
function capability(){
  if (cap && Date.now() - cap.t < 60000) return cap.v;
  let v;
  try {
    if (!fs.existsSync(path.join(P.ROOT, ".git"))) v = {ok:false, why:"Dieser Ordner ist keine Repo-Kopie – Veröffentlichen geht nur auf den Herausgeber-PCs"};
    else {
      const url = execFileSync("git", ["-C", P.ROOT, "remote", "get-url", "origin"], {encoding:"utf8", windowsHide:true, timeout:5000}).trim();
      v = url ? {ok:true, remote:url.replace(/\/\/[^@/]+@/, "//")} : {ok:false, why:"Kein GitHub-Repo (origin) eingerichtet"};
    }
  } catch (e) { v = {ok:false, why:"Git nicht gefunden oder kein origin eingerichtet"}; }
  cap = {t:Date.now(), v}; return v;
}
function sessionEnts(s){ return [...s.state.values()].map(e => { const o = Object.assign({}, e); delete o._op; return o; }); }

async function remoteEnts(){
  await git(["fetch", "--quiet", "origin", "main"]);
  try { return K.parse(await git(["show", "origin/main:" + FILE])).ents; }
  catch (e) { return []; }
}
function preview(s, karten){
  const base = karten.ents(s.info.base) || [], ents = sessionEnts(s), d = K.diff(base, ents), v = K.validate(ents);
  return {ok:v.ok, errors:v.errors, stats:v.stats, diff:d, guard:K.guard(d), pending:s.pending.size, base:s.info.base, herausgeber:capability()};
}
async function publish(s, karten, o){
  const c = capability(); if (!c.ok) return {ok:false, why:c.why};
  if (s.pending.size) return {ok:false, why:s.pending.size + " Änderung(en) sind noch nicht vom Host bestätigt – kurz warten und erneut versuchen"};
  const mine = sessionEnts(s);
  for (let attempt = 1; attempt <= 3; attempt++) {
    let remote;
    try { remote = await remoteEnts(); } catch (e) { return {ok:false, why:"GitHub nicht erreichbar: " + e.message}; }
    const merged = K.merge(remote, mine), v = K.validate(merged);
    if (!v.ok) return {ok:false, why:"Karte ungültig: " + v.errors.slice(0, 3).join("; ")};
    const d = K.diff(remote, merged), probs = K.guard(d);
    if (!d.neu && !d.geaendert && !d.geloescht) return {ok:false, why:"Keine Änderungen gegenüber der veröffentlichten Karte"};
    if (probs.length && !o.massenloeschung) return {ok:false, guard:probs, diff:d, why:"Schutzregel: " + probs.join("; ")};
    const sum = [d.neu && d.neu + " neu", d.geaendert && d.geaendert + " geändert", d.geloescht && d.geloescht + " gelöscht"].filter(Boolean).join(", ");
    const msg = "Karte: " + sum + " (Sitzung " + s.info.code + ", " + (o.name || "?") + ")" + (o.notiz ? "\n\n" + String(o.notiz).slice(0, 500) : "") + (o.massenloeschung ? "\n\n[massenloeschung-ok]" : "");
    // Commit bauen, ohne Arbeitsordner oder aktuellen Branch zu verändern (eigener, temporärer Index)
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "fd-pub-")), idx = path.join(tmp, "index"), f = path.join(tmp, "karte.json");
    try {
      fs.writeFileSync(f, K.serialize({}, merged));
      const env = Object.assign({}, process.env, {GIT_INDEX_FILE:idx});
      const blob = (await git(["hash-object", "-w", f])).trim();
      await git(["read-tree", "origin/main"], {env});
      await git(["update-index", "--add", "--cacheinfo", "100644," + blob + "," + FILE], {env});
      const tree = (await git(["write-tree"], {env})).trim();
      const commit = (await git(["commit-tree", tree, "-p", "origin/main", "-m", msg])).trim();
      try { await git(["push", "origin", commit + ":refs/heads/main"]); }
      catch (e) { if (attempt < 3 && /rejected|fetch first|non-fast-forward/i.test(e.message)) { log("Veröffentlichen: jemand war schneller – neuer Versuch"); continue; } throw e; }
      log("Karte veröffentlicht: " + sum + " (Commit " + commit.slice(0, 8) + ")");
      return {ok:true, commit:commit.slice(0, 8), diff:d, version:"nächste", text:"Veröffentlicht (" + sum + "). GitHub prüft und erstellt die neue Version – in 1–2 Minuten bekommen sie alle automatisch."};
    } catch (e) { return {ok:false, why:"Veröffentlichen fehlgeschlagen: " + e.message}; }
    finally { fs.rmSync(tmp, {recursive:true, force:true}); }
  }
  return {ok:false, why:"Veröffentlichen fehlgeschlagen – bitte erneut versuchen"};
}
module.exports = {capability, preview, publish};
