#!/usr/bin/env node
// Programm-Release bauen: signiertes Manifest für eine fertig gebaute ForzaDashboard.exe (für GitHub Actions).
//   node tools/app-release.js --exe <ForzaDashboard.exe> --version JJJJ-MM-TT.N --out <ordner> [--schluessel <pem-datei>]
// Schlüssel: Umgebungsvariable FD_SIGN_KEY (Inhalt von herausgeber.key) oder --schluessel.
// Spieler installieren die exe nur, wenn Signatur und Prüfsumme stimmen.
"use strict";
const fs = require("fs"), path = require("path"), crypto = require("crypto");
const SIG = require("./lib/signatur");

const args = process.argv.slice(2), opt = n => { const i = args.indexOf("--" + n); return i >= 0 ? args[i + 1] : null; };
const die = m => { console.error("FEHLER: " + m); process.exit(1); };
const exe = opt("exe"), version = opt("version"), out = opt("out");
if (!exe || !version || !out) die("--exe, --version und --out angeben");
if (!/^\d{4}-\d{2}-\d{2}\.\d+$/.test(version)) die("Version muss JJJJ-MM-TT.N sein: " + version);
const pem = process.env.FD_SIGN_KEY || (opt("schluessel") ? fs.readFileSync(opt("schluessel"), "utf8") : "");
if (!pem) die("Kein Signier-Schlüssel (FD_SIGN_KEY)");
if (!SIG.matches(pem)) die("Der Signier-Schlüssel passt nicht zum Programm (herausgeber.key verwenden)");

const buf = fs.readFileSync(exe);
const man = {typ:"app", version, sha256:crypto.createHash("sha256").update(buf).digest("hex"), groesse:buf.length, erstellt:new Date().toISOString(), quelle:process.env.GITHUB_SHA || ""};
fs.mkdirSync(out, {recursive:true});
fs.copyFileSync(exe, path.join(out, "ForzaDashboard.exe"));
fs.writeFileSync(path.join(out, "app.manifest.json"), SIG.sign(man, pem));
console.log("Programm " + version + ": " + Math.round(buf.length / 1024) + " KB -> " + out);
