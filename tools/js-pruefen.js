// Syntax-Prüfung der Oberfläche: zieht alle eingebetteten <script>-Blöcke aus src/dashboard.html und src/overlay.html
// und prüft sie mit `node --check`. Aufruf: node tools/js-pruefen.js   (Exit-Code 1 bei Fehlern)
"use strict";
const fs = require("fs"), path = require("path"), os = require("os"), { spawnSync } = require("child_process");
const files = ["dashboard.html", "overlay.html"].map(f => path.join(__dirname, "..", "src", f));
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fd-js-"));
let bad = 0, n = 0;
for (const f of files) {
  const html = fs.readFileSync(f, "utf8"), re = /<script(\s[^>]*)?>([\s\S]*?)<\/script>/gi;
  let m, i = 0;
  while ((m = re.exec(html))) {
    if (/\ssrc=/.test(m[1] || "") || !m[2].trim()) continue;
    const line = html.slice(0, m.index).split("\n").length, out = path.join(dir, path.basename(f) + "." + (++i) + ".js");
    fs.writeFileSync(out, m[2]); n++;
    const r = spawnSync(process.execPath, ["--check", out], {encoding: "utf8"});
    if (r.status !== 0) { bad++; console.log("FEHLER in " + path.basename(f) + " (Block ab Zeile " + line + "):\n" + (r.stderr || r.stdout)); }
  }
}
fs.rmSync(dir, {recursive: true, force: true});
console.log(n + " Skript-Blöcke geprüft, " + (bad ? bad + " mit Fehlern" : "alle in Ordnung"));
process.exit(bad ? 1 : 0);
