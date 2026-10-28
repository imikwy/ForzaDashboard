#!/usr/bin/env node
// Werkzeug für die Karten-Quelle (karte/karte.json) – lokal und in GitHub Actions.
//
//   node tools/karte.js import <datei>            alte karte-gemeinsam.json / Export in karte/karte.json übernehmen (zusammenführen)
//   node tools/karte.js pruefen [--vorher <datei>]   Karte prüfen (+ Schutz gegen Massenlöschung gegenüber der Vorversion)
//   node tools/karte.js release --version N --out <ordner> [--vorher <datei>] [--massenloeschung-ok]
//                                                  Release bauen: karte.json.gz + signiertes Manifest
//                                                  Schlüssel: Umgebungsvariable FD_SIGN_KEY (PEM) oder --schluessel <datei>
"use strict";
const fs = require("fs"), path = require("path"), zlib = require("zlib");
process.env.FD_QUIET = "1";
process.env.FD_DATA = process.env.FD_DATA || path.join(require("os").tmpdir(), "fd-werkzeug-daten");
const K = require("../app/lib/kartenpruefung");
const SIG = require("../app/lib/signatur");
const {buildManifest} = require("../app/lib/karte");

const ROOT = path.resolve(__dirname, "..");
const SRC = path.join(ROOT, "karte", "karte.json");
const args = process.argv.slice(2), cmd = args[0];
const opt = n => { const i = args.indexOf("--" + n); return i >= 0 ? (args[i + 1] && !args[i + 1].startsWith("--") ? args[i + 1] : true) : null; };
const die = m => { console.error("FEHLER: " + m); process.exit(1); };

function readEnts(file){
  let txt = fs.readFileSync(file);
  if (file.endsWith(".gz")) txt = zlib.gunzipSync(txt);
  const o = K.parse(txt.toString("utf8"));
  return o.ents;
}
function printStats(s){ console.log("  " + s.gesamt + " Objekte (" + s.strassen + " Straßen, " + s.orte + " Orte, " + s.radiert + " Radierungen, " + s.ausgeblendet + " ausgeblendet, " + s.einstellungen + " Einstellungen, " + s.geloescht + " gelöscht)"); }
function printDiff(d){ console.log("  Änderungen: +" + d.neu + " neu, " + d.geaendert + " geändert, −" + d.geloescht + " gelöscht" + (d.verschwunden ? ", " + d.verschwunden + " verschwunden" : "")); }

if (cmd === "import") {
  const f = args[1]; if (!f) die("Datei angeben");
  const add = readEnts(f), cur = fs.existsSync(SRC) ? readEnts(SRC) : [];
  const merged = K.merge(cur, add), v = K.validate(merged);
  if (!v.ok) die("Ergebnis ungültig:\n  " + v.errors.join("\n  "));
  const d = K.diff(cur, merged);
  fs.mkdirSync(path.dirname(SRC), {recursive:true});
  fs.writeFileSync(SRC, K.serialize({}, merged));
  console.log("karte/karte.json geschrieben"); printStats(v.stats); printDiff(d);
}
else if (cmd === "pruefen") {
  const ents = readEnts(opt("datei") || SRC), v = K.validate(ents);
  printStats(v.stats);
  if (!v.ok) die("Karte ungültig:\n  " + v.errors.join("\n  "));
  const norm = K.serialize({}, ents);
  if (!opt("datei") && fs.readFileSync(SRC, "utf8").replace(/\r\n/g, "\n") !== norm) die("karte/karte.json ist nicht im Standardformat – `node tools/karte.js import karte/karte.json` ausführen");
  if (opt("vorher")) {
    const d = K.diff(readEnts(opt("vorher")), ents); printDiff(d);
    const probs = K.guard(d);
    if (probs.length && !opt("massenloeschung-ok")) die("Schutzregel:\n  " + probs.join("\n  ") + "\n  (absichtlich? Commit-Nachricht mit [massenloeschung-ok])");
  }
  console.log("OK – Karte gültig");
}
else if (cmd === "release") {
  const version = parseInt(opt("version"), 10), out = opt("out");
  if (!(version > 0) || !out) die("--version und --out angeben");
  const ents = readEnts(opt("datei") || SRC), v = K.validate(ents);
  if (!v.ok) die("Karte ungültig:\n  " + v.errors.join("\n  "));
  let d = null;
  if (opt("vorher")) {
    d = K.diff(readEnts(opt("vorher")), ents); printDiff(d);
    const probs = K.guard(d);
    if (probs.length && !opt("massenloeschung-ok")) die("Schutzregel:\n  " + probs.join("\n  "));
  }
  const buf = Buffer.from(K.serialize({version}, ents), "utf8");
  // im Release steht die Version in der Datei; Prüfsumme über genau diese Bytes
  let man = buildManifest(version, buf, ents, {notiz:String(opt("notiz") || ""), quelle:process.env.GITHUB_SHA || "", aenderungen:d ? {neu:d.neu, geaendert:d.geaendert, geloescht:d.geloescht} : null});
  const pem = process.env.FD_SIGN_KEY || (opt("schluessel") ? fs.readFileSync(opt("schluessel"), "utf8") : "");
  if (!pem) die("Kein Signier-Schlüssel (FD_SIGN_KEY)");
  man = SIG.sign(man, pem);
  fs.mkdirSync(out, {recursive:true});
  fs.writeFileSync(path.join(out, "karte.json.gz"), zlib.gzipSync(buf, {level:9}));
  fs.writeFileSync(path.join(out, "karte.manifest.json"), JSON.stringify(man, null, 1));
  console.log("Release Karte v" + version + " gebaut in " + out); printStats(v.stats);
}
else {
  console.log(fs.readFileSync(__filename, "utf8").split("\n").slice(1, 9).map(l => l.replace(/^\/\/ ?/, "")).join("\n"));
  process.exit(cmd ? 1 : 0);
}
