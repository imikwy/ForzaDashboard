#!/usr/bin/env node
// Programm-Release bauen: alle Programmdateien in ein Paket (app-bundle.json.gz) + signiertes Manifest.
//   node tools/app-release.js --out <ordner> [--schluessel <pem-datei>]   (oder Umgebungsvariable FD_SIGN_KEY)
// Die Version kommt aus app/package.json. Spieler installieren das Paket nur, wenn die Signatur stimmt.
"use strict";
const fs = require("fs"), path = require("path"), zlib = require("zlib"), crypto = require("crypto");
const SIG = require("../app/lib/signatur");

const ROOT = path.resolve(__dirname, "..");
const args = process.argv.slice(2), opt = n => { const i = args.indexOf("--" + n); return i >= 0 ? args[i + 1] : null; };
const die = m => { console.error("FEHLER: " + m); process.exit(1); };
const out = opt("out"); if (!out) die("--out angeben");
const version = JSON.parse(fs.readFileSync(path.join(ROOT, "app", "package.json"), "utf8")).version;
if (!/^\d+\.\d+\.\d+$/.test(version)) die("Version in app/package.json ungültig: " + version);

const files = {};
function add(rel){ files[rel.split(path.sep).join("/")] = fs.readFileSync(path.join(ROOT, rel)).toString("base64"); }
function walk(dir){
  for (const f of fs.readdirSync(path.join(ROOT, dir), {withFileTypes:true})) {
    const rel = path.join(dir, f.name);
    if (f.isDirectory()) { if (f.name !== "node_modules") walk(rel); }
    else if (!/\.(tmp|log)$/.test(f.name)) add(rel);
  }
}
walk("app");
for (const f of ["starten.js", "LIESMICH.txt", "ForzaDashboard-starten.bat", "GamePass-Fix.bat"]) if (fs.existsSync(path.join(ROOT, f))) add(f);
if (!files["app/server.js"] || !files["app/karte-start.json.gz"]) die("Programm unvollständig");

const gz = zlib.gzipSync(Buffer.from(JSON.stringify({version, files}), "utf8"), {level:9});
let man = {typ:"app", version, datei:"app-bundle.json.gz", sha256:crypto.createHash("sha256").update(gz).digest("hex"), groesse:gz.length, dateien:Object.keys(files).length, erstellt:new Date().toISOString(), quelle:process.env.GITHUB_SHA || ""};
const pem = process.env.FD_SIGN_KEY || (opt("schluessel") ? fs.readFileSync(opt("schluessel"), "utf8") : "");
if (!pem) die("Kein Signier-Schlüssel (FD_SIGN_KEY)");
man = SIG.sign(man, pem);
fs.mkdirSync(out, {recursive:true});
fs.writeFileSync(path.join(out, "app-bundle.json.gz"), gz);
fs.writeFileSync(path.join(out, "app.manifest.json"), JSON.stringify(man, null, 1));
console.log("Programm " + version + ": " + man.dateien + " Dateien, " + Math.round(gz.length / 1024) + " KB -> " + out);
