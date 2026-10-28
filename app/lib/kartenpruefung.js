// Prüfung und einheitliches Format der Karte. Wird im Programm UND beim Veröffentlichen auf GitHub benutzt –
// so kann keine kaputte oder versehentlich geleerte Karte zur neuen "Wahrheit" werden.
//
// Objekte: {id, k, …, ts, a}
//   r = Straße {t, lv, sm, p:[x,z,x,z,…]}      o = Ort {kind, n, x, z}      e = Radier-Kreis {x, z, r}
//   x = ausgeblendet ("x:" + Ziel)             c = Einstellung ("cfg:…": Kartenausrichtung, Straßenkorrekturen, Tacho)
//   d:1 = gelöscht (bleibt als Grabstein stehen, damit das Löschen beim Zusammenführen nicht verloren geht)
"use strict";

const KINDS = new Set(["r", "o", "e", "x", "c"]);
const ROAD_TYPES = new Set(["motor", "road", "gravel", "off", "tunnel"]);
const LIM = 30000;   // Spielwelt-Koordinaten liegen weit darunter
const fin = v => typeof v === "number" && Number.isFinite(v) && Math.abs(v) < LIM;

function checkEnt(e){
  if (!e || typeof e !== "object" || Array.isArray(e)) return "kein Objekt";
  if (typeof e.id !== "string" || !e.id || e.id.length > 100) return "ungültige id";
  if (!KINDS.has(e.k)) return "unbekannte Art " + e.k;
  if (e.ts !== undefined && !(typeof e.ts === "number" && e.ts >= 0)) return "ungültige Zeit";
  if (e.d === 1) return null;   // Grabstein
  if (e.k === "r") {
    if (!Array.isArray(e.p) || e.p.length < 4 || e.p.length % 2 || e.p.length > 40000 || !e.p.every(fin)) return "Straße ohne gültige Punkte";
    if (e.t !== undefined && !ROAD_TYPES.has(e.t)) return "unbekannter Straßentyp " + e.t;
    if (e.lv !== undefined && ![-1, 0, 1, 2].includes(e.lv)) return "ungültige Ebene";
  } else if (e.k === "o") {
    if (!fin(e.x) || !fin(e.z)) return "Ort ohne Position";
    if (e.n !== undefined && (typeof e.n !== "string" || e.n.length > 60)) return "Ortsname zu lang";
  } else if (e.k === "e") {
    if (!fin(e.x) || !fin(e.z) || !(typeof e.r === "number" && e.r > 0 && e.r <= 500)) return "ungültiger Radier-Kreis";
  } else if (e.k === "x") {
    if (!e.id.startsWith("x:") || e.id.length < 3) return "Ausblendung ohne Ziel";
  } else if (e.k === "c") {
    if (!e.id.startsWith("cfg:")) return "Einstellung ohne cfg:-Kennung";
    if (e.id === "cfg:mapfit" && !(Array.isArray(e.T) && e.T.length === 6 && e.T.every(v => typeof v === "number" && Number.isFinite(v)))) return "ungültige Kartenausrichtung";
    if (e.id === "cfg:roadfix" && (typeof e.lines !== "object" || !e.lines || Array.isArray(e.lines))) return "ungültige Straßenkorrekturen";
  }
  return null;
}

// Einheitliche Schreibweise: feste Reihenfolge der Felder, Zahlen gerundet -> gleiche Karte = gleiche Bytes
const r1 = v => Math.round(v * 10) / 10;
function canonEnt(e){
  const o = {id:e.id, k:e.k};
  const keys = Object.keys(e).filter(k => !["id", "k", "ts", "a"].includes(k)).sort();
  for (const k of keys) { if (e[k] === undefined) continue; o[k] = k === "p" && Array.isArray(e.p) ? e.p.map(r1) : e[k]; }
  if (e.ts !== undefined) o.ts = e.ts;
  if (e.a !== undefined) o.a = e.a;
  return o;
}
function sortEnts(list){ return list.slice().sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0); }
// Datei: eine Zeile pro Objekt – gut lesbar und Git zeigt genau, was sich geändert hat
function serialize(meta, ents){
  const head = JSON.stringify(Object.assign({format:"fd-karte", schema:1}, meta));
  const lines = sortEnts(ents).map(e => JSON.stringify(canonEnt(e)));
  return head.slice(0, -1) + ',"ents":[\n' + lines.join(",\n") + "\n]}\n";
}
function parse(text){
  const o = JSON.parse(text);
  if (!o || !Array.isArray(o.ents)) throw new Error("keine Karten-Datei");
  return o;
}

function validate(ents){
  const errors = [], ids = new Set();
  const stats = {gesamt:0, aktiv:0, strassen:0, orte:0, radiert:0, ausgeblendet:0, einstellungen:0, geloescht:0};
  for (const e of ents) {
    const err = checkEnt(e);
    if (err) { if (errors.length < 50) errors.push((e && e.id || "?") + ": " + err); continue; }
    if (ids.has(e.id)) { if (errors.length < 50) errors.push(e.id + ": doppelt"); continue; }
    ids.add(e.id); stats.gesamt++;
    if (e.d === 1) { stats.geloescht++; continue; }
    stats.aktiv++;
    stats[{r:"strassen", o:"orte", e:"radiert", x:"ausgeblendet", c:"einstellungen"}[e.k]]++;
  }
  return {ok:!errors.length, errors, stats};
}

// Was ändert sich von alt -> neu? (für die Zusammenfassung vor dem Veröffentlichen und den Schutz vor Massenlöschung)
function diff(oldEnts, newEnts){
  const A = new Map(oldEnts.map(e => [e.id, e])), B = new Map(newEnts.map(e => [e.id, e]));
  const res = {neu:0, geaendert:0, geloescht:0, verschwunden:0, nachArt:{}};
  const bump = (k, f) => { const o = res.nachArt[k] || (res.nachArt[k] = {neu:0, geaendert:0, geloescht:0}); o[f]++; };
  const same = (a, b) => JSON.stringify(canonEnt(Object.assign({}, a, {ts:0, a:""}))) === JSON.stringify(canonEnt(Object.assign({}, b, {ts:0, a:""})));
  for (const [id, b] of B) {
    const a = A.get(id), aLive = a && a.d !== 1, bLive = b.d !== 1;
    if (!aLive && bLive) { res.neu++; bump(b.k, "neu"); }
    else if (aLive && !bLive) { res.geloescht++; bump(b.k, "geloescht"); }
    else if (aLive && bLive && !same(a, b)) { res.geaendert++; bump(b.k, "geaendert"); }
  }
  for (const [id, a] of A) if (!B.has(id)) { res.verschwunden++; if (a.d !== 1) { res.geloescht++; bump(a.k, "geloescht"); } }
  const liveOld = oldEnts.filter(e => e.d !== 1).length;
  res.anteilGeloescht = liveOld ? res.geloescht / liveOld : 0;
  return res;
}
// Schutzregel: mehr als 5 % (mind. 200 Objekte) auf einmal gelöscht, oder Objekte ganz verschwunden -> nur mit ausdrücklicher Freigabe
function guard(d){
  const probs = [];
  if (d.verschwunden) probs.push(d.verschwunden + " Objekte fehlen komplett (Grabsteine müssen erhalten bleiben)");
  if (d.geloescht > 200 && d.anteilGeloescht > 0.05) probs.push(d.geloescht + " Objekte gelöscht (" + Math.round(d.anteilGeloescht * 100) + " %) – das sieht nach einem Versehen aus");
  return probs;
}

// Zusammenführen zweier Stände: pro Objekt gewinnt die neuere Änderung (Grabsteine zählen mit)
function newer(b, a){ const tb = b.ts || 0, ta = a.ts || 0; return tb > ta || (tb === ta && (b.a || "") > (a.a || "")); }
function merge(listA, listB){
  const m = new Map(listA.map(e => [e.id, e]));
  for (const e of listB) { const c = m.get(e.id); if (!c || newer(e, c)) m.set(e.id, e); }
  return [...m.values()];
}

module.exports = {checkEnt, canonEnt, sortEnts, serialize, parse, validate, diff, guard, merge, newer, ROAD_TYPES};
