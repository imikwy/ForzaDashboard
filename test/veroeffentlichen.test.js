"use strict";
// Veröffentlichen gegen ein lokales Git-Repo (statt GitHub): Zusammenführen, Wettlauf zweier Herausgeber, Schutzregel
const fs = require("fs"), os = require("os"), path = require("path"), {execFileSync} = require("child_process");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "fd-pub-test-")), origin = path.join(tmp, "origin.git"), work = path.join(tmp, "work"), other = path.join(tmp, "other");
process.env.FD_ROOT = work;
const git = (cwd, ...a) => execFileSync("git", a, {cwd, encoding:"utf8", stdio:["ignore", "pipe", "pipe"]});
const K = require("../app/lib/kartenpruefung");
const base = [...Array(50).keys()].map(i => ({id:"r" + i, k:"r", t:"road", lv:0, sm:1, p:[i, 0, i + 1, 1], ts:100, a:"x"}));
fs.mkdirSync(origin); git(origin, "init", "--bare", "-q", "-b", "main");
git(tmp, "clone", "-q", origin, work);
for (const d of [work]) { git(d, "config", "user.email", "t@t"); git(d, "config", "user.name", "T"); }
fs.mkdirSync(path.join(work, "karte")); fs.writeFileSync(path.join(work, "karte", "karte.json"), K.serialize({}, base));
git(work, "add", "."); git(work, "commit", "-q", "-m", "start"); git(work, "push", "-q", "origin", "main");

require("./helfer");
const test = require("node:test"), assert = require("node:assert");
const pub = require("../app/lib/veroeffentlichen");
const sess = (ents, code) => ({state:new Map(ents.map(e => [e.id, e])), pending:new Map(), info:{code:code || "AAABBBCCC", base:1}});
const remote = () => K.parse(git(work, "show", "origin/main:karte/karte.json")).ents;

test("Veröffentlichen: Commit auf main, Arbeitsordner bleibt unberührt", async () => {
  const mine = base.map(e => e.id === "r1" ? Object.assign({}, e, {t:"gravel", ts:200}) : e).concat([{id:"neu1", k:"o", kind:"fuel", x:1, z:2, ts:201, a:"me"}]);
  const before = fs.readFileSync(path.join(work, "karte", "karte.json"), "utf8");
  const r = await pub.publish(sess(mine), null, {name:"Tester", notiz:"Tanke"});
  assert.ok(r.ok, r.why);
  assert.strictEqual(r.diff.neu, 1); assert.strictEqual(r.diff.geaendert, 1);
  git(work, "fetch", "-q");
  const ents = remote();
  assert.strictEqual(ents.find(e => e.id === "r1").t, "gravel"); assert.ok(ents.find(e => e.id === "neu1"));
  assert.match(git(work, "log", "-1", "--pretty=%B", "origin/main"), /Sitzung AAABBBCCC, Tester[\s\S]*Tanke/);
  assert.strictEqual(fs.readFileSync(path.join(work, "karte", "karte.json"), "utf8"), before, "Arbeitsordner unverändert");
});

test("Wettlauf: Änderung eines anderen Herausgebers bleibt erhalten", async () => {
  // jemand anderes veröffentlicht inzwischen (neuer als unser Stand)
  git(tmp, "clone", "-q", origin, other); git(other, "config", "user.email", "o@o"); git(other, "config", "user.name", "O");
  const theirs = K.parse(fs.readFileSync(path.join(other, "karte", "karte.json"), "utf8")).ents.map(e => e.id === "r2" ? Object.assign({}, e, {t:"off", ts:300}) : e);
  fs.writeFileSync(path.join(other, "karte", "karte.json"), K.serialize({}, theirs));
  git(other, "commit", "-qam", "andere"); git(other, "push", "-q", "origin", "main");
  // unsere Sitzung kennt r2 noch im alten Zustand, ändert aber r3
  const mine = base.map(e => e.id === "r3" ? Object.assign({}, e, {lv:1, ts:301}) : e);
  const r = await pub.publish(sess(mine, "DDDEEEFFF"), null, {name:"Ich"});
  assert.ok(r.ok, r.why);
  git(work, "fetch", "-q");
  const ents = remote();
  assert.strictEqual(ents.find(e => e.id === "r2").t, "off", "fremde, neuere Änderung erhalten");
  assert.strictEqual(ents.find(e => e.id === "r3").lv, 1, "eigene Änderung drin");
  assert.strictEqual(ents.find(e => e.id === "r1").t, "gravel", "frühere Veröffentlichung erhalten");
});

test("Schutzregel: Massenlöschung nur mit ausdrücklicher Freigabe", async () => {
  const many = [...Array(400).keys()].map(i => ({id:"m" + i, k:"e", x:i, z:0, r:2, ts:400, a:"x"}));
  assert.ok((await pub.publish(sess(remote().concat(many), "GGGHHHJJJ"), null, {name:"Ich"})).ok);
  const del = remote().map(e => e.id.startsWith("m") ? {id:e.id, k:"e", d:1, ts:500, a:"x"} : e);
  const r = await pub.publish(sess(del, "GGGHHHJJJ"), null, {name:"Ich"});
  assert.ok(!r.ok && r.guard && r.guard.length, "blockiert");
  const r2 = await pub.publish(sess(del, "GGGHHHJJJ"), null, {name:"Ich", massenloeschung:true});
  assert.ok(r2.ok, r2.why);
  assert.match(git(work, "log", "-1", "--pretty=%B", "origin/main"), /\[massenloeschung-ok\]/);
});

test("Nicht veröffentlichen, solange Änderungen ausstehen", async () => {
  const s = sess(remote()); s.pending.set("x", {});
  const r = await pub.publish(s, null, {name:"Ich"});
  assert.ok(!r.ok); assert.match(r.why, /noch nicht vom Host bestätigt/);
});
