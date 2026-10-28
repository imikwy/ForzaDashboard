#!/usr/bin/env node
// Werkzeug für die Karten-Quelle (karte/karte.json) – vor allem für GitHub Actions. Spieler brauchen es nicht.
//
//   node tools/karte.js import <datei>               karte-gemeinsam.json / Export in karte/karte.json übernehmen (zusammenführen)
//   node tools/karte.js pruefen [--vorher <datei>]   Karte prüfen (+ Schutz gegen Massenlöschung gegenüber der Vorversion)
//   node tools/karte.js release --version N --out <ordner> [--vorher <datei>] [--massenloeschung-ok] [--notiz <text>]
//                                                   Release bauen: karte.json.gz + signiertes Manifest
//                                                   Schlüssel: Umgebungsvariable FD_SIGN_KEY (PEM) oder --schluessel <datei>
"use strict";
const fs = require("fs"), path = require("path"), zlib = require("zlib"), crypto = require("crypto");
const K = require("./lib/kartenpruefung");
const SIG = require("./lib/signatur");

const ROOT = path.resolve(__dirname, "..");
const SRC = path.join(ROOT, "karte", "karte.json");
const args = process.argv.slice(2), cmd = args[0];
const opt = n => { const i = args.indexOf("--" + n); return i >= 0 ? (args[i + 1] && !args[i + 1].startsWith("--") ? args[i + 1] : true) : null; };
const die = m => { console.error("FEHLER: " + m); process.exit(1); };
const sha256 = b => crypto.createHash("sha256").update(b).digest("hex");
// Prüfsumme der eingebauten Forza-Straßen (src/fh6-karte.js): eine Karte passt nur zu genau diesem Straßennetz
const BASIS = sha256(fs.readFileSync(path.join(ROOT, "src", "fh6-karte.js"))).slice(0, 16);

function readEnts(file){
  let txt = fs.readFileSync(file);
  if (file.endsWith(".gz")) txt = zlib.gunzipSync(txt);
  return K.parse(txt.toString("utf8")).ents;
}
function printStats(s){ console.log("  " + s.gesamt + " Objekte (" + s.strassen + " Straßen, " + s.orte + " Orte, " + s.radiert + " Radierungen, " + s.ausgeblendet + " ausgeblendet, " + s.einstellungen + " Einstellungen, " + s.geloescht + " gelöscht)"); }
function printDiff(d){ console.log("  Änderungen: +" + d.neu + " neu, " + d.geaendert + " geändert, −" + d.geloescht + " gelöscht" + (d.verschwunden ? ", " + d.verschwunden + " verschwunden" : "")); }
function checkGuard(d){
  printDiff(d);
  const probs = K.guard(d);
  if (probs.length && !opt("massenloeschung-ok")) die("Schutzregel:\n  " + probs.join("\n  ") + "\n  (absichtlich? Commit-Nachricht mit [massenloeschung-ok])");
}

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
  if (opt("vorher")) checkGuard(K.diff(readEnts(opt("vorher")), ents));
  console.log("OK – Karte gültig");
}
else if (cmd === "release") {
  const version = parseInt(opt("version"), 10), out = opt("out");
  if (!(version > 0) || !out) die("--version und --out angeben");
  const ents = readEnts(opt("datei") || SRC), v = K.validate(ents);
  if (!v.ok) die("Karte ungültig:\n  " + v.errors.join("\n  "));
  const d = opt("vorher") ? K.diff(readEnts(opt("vorher")), ents) : null;
  if (d) checkGuard(d);
  const pem = process.env.FD_SIGN_KEY || (opt("schluessel") ? fs.readFileSync(opt("schluessel"), "utf8") : "");
  if (!pem) die("Kein Signier-Schlüssel (FD_SIGN_KEY)");
  if (!SIG.matches(pem)) die("Der Signier-Schlüssel passt nicht zum Programm (herausgeber.key verwenden)");
  // im Release ist die Datei immer im Standardformat; die Prüfsumme gilt für genau diese Bytes
  const buf = Buffer.from(K.serialize({version}, ents), "utf8");
  const man = {typ:"karte", version, sha256:sha256(buf), groesse:buf.length, objekte:ents.length, basis:BASIS,
    erstellt:new Date().toISOString(), notiz:String(opt("notiz") === true ? "" : opt("notiz") || "").slice(0, 300), quelle:process.env.GITHUB_SHA || "",
    aenderungen:d ? {neu:d.neu, geaendert:d.geaendert, geloescht:d.geloescht} : null};
  fs.mkdirSync(out, {recursive:true});
  fs.writeFileSync(path.join(out, "karte.json.gz"), zlib.gzipSync(buf, {level:9}));
  fs.writeFileSync(path.join(out, "karte.manifest.json"), SIG.sign(man, pem));
  console.log("Release Karte v" + version + " gebaut in " + out); printStats(v.stats);
}
else {
  console.log(fs.readFileSync(__filename, "utf8").split("\n").slice(1, 9).map(l => l.replace(/^\/\/ ?/, "")).join("\n"));
  process.exit(cmd ? 1 : 0);
}
