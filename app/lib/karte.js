// Kartenversionen: die "Quelle der Wahrheit" auf diesem PC.
// - Jede Version liegt als eigene, schreibgeschützte Datei in daten/karten/ (karte-v12.json + Manifest).
// - Eine Version wird nur übernommen, wenn Signatur, Prüfsumme und Inhalt stimmen. Erst dann wird umgeschaltet.
// - Nichts überschreibt eine Version: neue Versionen kommen dazu, die letzten 10 bleiben für ein "Zurück".
// - Beim Start wird die aktive Version geprüft; ist sie beschädigt, wird automatisch die letzte gute benutzt.
"use strict";
const fs = require("fs"), path = require("path"), zlib = require("zlib");
const {P, CONFIG, VERSION} = require("./basis");
const {sha256, writeAtomic, writeJson, readJson, log} = require("./util");
const K = require("./kartenpruefung");
const SIG = require("./signatur");

const KEEP = 10;
const fileOf = v => path.join(P.KARTEN, "karte-v" + v + ".json");
const manOf = v => path.join(P.KARTEN, "karte-v" + v + ".manifest.json");
const AKTIV = path.join(P.KARTEN, "aktiv.json");

// Prüfsumme der eingebauten Forza-Straßen: eine Karte passt nur zu genau diesem Straßennetz
let BASE_HASH = "";
try { BASE_HASH = sha256(fs.readFileSync(path.join(P.PUBLIC, "fh6-karte.js"))).slice(0, 16); } catch (e) {}

function verNum(a, b){ const A = String(a).split(".").map(Number), B = String(b).split(".").map(Number); for (let i = 0; i < 3; i++) if ((A[i] || 0) !== (B[i] || 0)) return (A[i] || 0) - (B[i] || 0); return 0; }

// Prüft Manifest + Inhalt. Gibt eine Fehlermeldung zurück oder null.
function checkRelease(man, buf, opts){
  opts = opts || {};
  if (!man || man.typ !== "karte" || !(Number.isInteger(man.version) && man.version > 0)) return "Manifest ungültig";
  if (!opts.noSig && !SIG.verify(man, CONFIG.signaturSchluessel)) return "Signatur ungültig – Karte stammt nicht von den Herausgebern";
  if (sha256(buf) !== man.sha256) return "Prüfsumme stimmt nicht – Datei beschädigt";
  if (man.minApp && verNum(VERSION, man.minApp) < 0) return "Karte braucht Programmversion " + man.minApp + " (du hast " + VERSION + ")";
  let o; try { o = K.parse(buf.toString("utf8")); } catch (e) { return "Karten-Datei nicht lesbar"; }
  const v = K.validate(o.ents);
  if (!v.ok) return "Karte fehlerhaft: " + v.errors.slice(0, 3).join("; ");
  if (o.ents.length !== man.objekte) return "Anzahl der Objekte stimmt nicht";
  return null;
}

class KartenSpeicher {
  constructor(){ this.active = null; this.listeners = new Set(); }
  init(){
    fs.mkdirSync(P.KARTEN, {recursive:true});
    const want = readJson(AKTIV, {}).version;
    const order = [...new Set([want, ...this.versions().reverse()].filter(v => v))];
    for (const v of order) { if (this.load(v)) break; log("Kartenversion " + v + " beschädigt – nehme die nächste"); }
    // Startkarte aus dem Programm (neue Installation oder alles beschädigt)
    const st = this.bundled();
    if (st && (!this.active || st.man.version > this.active.version)) {
      const err = this.install(st.man, st.buf, {source:"Programm"});
      if (err && !this.active) log("Startkarte ungültig:", err);
    }
    if (!this.active) log("WARNUNG: keine gültige Karte vorhanden");
    return this;
  }
  versions(){
    try { return fs.readdirSync(P.KARTEN).map(f => /^karte-v(\d+)\.json$/.exec(f)).filter(Boolean).map(m => +m[1]).sort((a, b) => a - b); } catch (e) { return []; }
  }
  bundled(){
    try {
      const man = readJson(P.BUNDLED_MAP.replace(/\.json\.gz$/, ".manifest.json"), null);
      if (!man) return null;
      return {man, buf:zlib.gunzipSync(fs.readFileSync(P.BUNDLED_MAP))};
    } catch (e) { return null; }
  }
  load(v){
    try {
      const man = readJson(manOf(v), null), buf = fs.readFileSync(fileOf(v));
      // gespeicherte Versionen wurden beim Herunterladen schon geprüft; hier zählt, dass sie unverändert sind
      const err = checkRelease(man, buf, {noSig:!man.sig});
      if (err) { log("Karte v" + v + ":", err); return false; }
      const o = K.parse(buf.toString("utf8"));
      this.active = {version:v, man, ents:o.ents, map:new Map(o.ents.map(e => [e.id, e]))};
      return true;
    } catch (e) { return false; }
  }
  // neue Version übernehmen (geprüft, schreibgeschützt abgelegt, dann umgeschaltet)
  install(man, buf, info){
    const err = checkRelease(man, buf);
    if (err) return err;
    const v = man.version;
    let have = null; try { have = fs.readFileSync(fileOf(v)); } catch (e) {}
    if (!have || sha256(have) !== man.sha256) {   // fehlt oder beschädigt -> (neu) ablegen
      if (have) { log("Karte v" + v + " war beschädigt und wird repariert"); try { fs.chmodSync(fileOf(v), 0o644); } catch (e) {} }
      writeAtomic(fileOf(v), buf); writeJson(manOf(v), man, true);
      try { fs.chmodSync(fileOf(v), 0o444); } catch (e) {}
    }
    if (!this.active || v >= this.active.version) {
      if (!this.load(v)) return "Version konnte nicht geladen werden";
      writeJson(AKTIV, {version:v, seit:new Date().toISOString(), quelle:(info && info.source) || ""}, true);
      log("Karte Version " + v + " aktiv (" + man.objekte + " Objekte" + (info && info.source ? ", aus " + info.source : "") + ")");
      for (const f of this.listeners) try { f(this.active); } catch (e) {}
    }
    this.prune();
    return null;
  }
  // auf eine ältere Version zurück (nur lokal – z. B. wenn eine neue Version Probleme macht)
  activate(v){
    if (!this.load(v)) return "Version " + v + " nicht vorhanden oder beschädigt";
    writeJson(AKTIV, {version:v, seit:new Date().toISOString(), quelle:"von Hand"}, true);
    for (const f of this.listeners) try { f(this.active); } catch (e) {}
    return null;
  }
  prune(){
    const vs = this.versions(), cur = this.active && this.active.version;
    for (const v of vs.slice(0, Math.max(0, vs.length - KEEP))) {
      if (v === cur) continue;
      for (const f of [fileOf(v), manOf(v)]) { try { fs.chmodSync(f, 0o644); fs.unlinkSync(f); } catch (e) {} }
    }
  }
  // Objekte einer bestimmten (auch älteren) Version – für Sitzungen, die auf dieser Version aufbauen
  ents(v){
    if (this.active && this.active.version === v) return this.active.ents;
    try {
      const man = readJson(manOf(v), null), buf = fs.readFileSync(fileOf(v));
      if (!man || sha256(buf) !== man.sha256) return null;
      return K.parse(buf.toString("utf8")).ents;
    } catch (e) { return null; }
  }
  // Release-Dateien einer Version (zum Weitergeben an Sitzungs-Teilnehmer, denen sie fehlt)
  raw(v){
    try { return {man:readJson(manOf(v), null), buf:fs.readFileSync(fileOf(v))}; } catch (e) { return null; }
  }
  has(v){ return fs.existsSync(fileOf(v)) && fs.existsSync(manOf(v)); }
  onChange(f){ this.listeners.add(f); }
  info(){
    const a = this.active;
    return a ? {version:a.version, objekte:a.ents.length, erstellt:a.man.erstellt || "", notiz:a.man.notiz || "", signiert:!!a.man.sig, basisOk:!a.man.basis || a.man.basis === BASE_HASH, versionen:this.versions()} : {version:0, objekte:0, versionen:[]};
  }
}

// Manifest für eine Karten-Datei bauen (Werkzeuge + Veröffentlichen)
function buildManifest(version, buf, ents, extra){
  return Object.assign({typ:"karte", version, datei:"karte.json.gz", sha256:sha256(buf), groesse:buf.length, objekte:ents.length,
    basis:BASE_HASH, minApp:"3.0.0", erstellt:new Date().toISOString()}, extra || {});
}

module.exports = {KartenSpeicher, checkRelease, buildManifest, BASE_HASH};
