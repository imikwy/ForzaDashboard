"use strict";
require("./helfer");
const test = require("node:test"), assert = require("node:assert");
const fs = require("fs"), os = require("os"), path = require("path"), {spawnSync} = require("child_process");

// Mini-Installation: app/server.js gibt nur seine Kennung aus
function setup(newServer){
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "fd-start-")), data = path.join(root, "daten");
  fs.copyFileSync(path.join(__dirname, "..", "starten.js"), path.join(root, "starten.js"));
  const mk = (dir, code, ver) => { fs.mkdirSync(path.join(dir, "app"), {recursive:true}); fs.writeFileSync(path.join(dir, "app", "server.js"), code); fs.writeFileSync(path.join(dir, "app", "package.json"), JSON.stringify({version:ver})); };
  mk(root, 'console.log("ALT"); process.exit(0);', "1.0.0");
  const staged = path.join(data, "updates", "app-2.0.0"); mk(staged, newServer, "2.0.0");
  fs.writeFileSync(path.join(data, "updates", "bereit.json"), JSON.stringify({version:"2.0.0", dir:staged}));
  return {root, data};
}
const run = r => spawnSync(process.execPath, [path.join(r.root, "starten.js")], {env:Object.assign({}, process.env, {FD_DATA:r.data, FD_ONCE:"1"}), encoding:"utf8", timeout:30000});

test("Starter installiert ein bereitliegendes Update", () => {
  const r = setup('console.log("NEU"); process.exit(0);'), out = run(r).stdout;
  assert.match(out, /Update 2\.0\.0 installiert/); assert.match(out, /NEU/);
  assert.match(fs.readFileSync(path.join(r.root, "app", "server.js"), "utf8"), /NEU/);
  assert.ok(fs.readdirSync(path.join(r.data, "updates")).some(d => d.startsWith("vorher-1.0.0")), "Sicherung der alten Version");
});

test("Stürzt die neue Version ab, wird die alte automatisch wiederhergestellt", () => {
  const r = setup('process.exit(3);'), out = run(r).stdout;
  assert.match(out, /abgestürzt/); assert.match(out, /wiederhergestellt/); assert.match(out, /ALT/);
  assert.match(fs.readFileSync(path.join(r.root, "app", "server.js"), "utf8"), /ALT/);
});
