"use strict";
const {Bus, FakeNet, until} = require("./helfer");
const test = require("node:test"), assert = require("node:assert");
const {channel, seal, open} = require("../app/lib/netz");
const {Kanal} = require("../app/lib/kanal");
const {snappy} = require("../app/lib/umzug");

test("Verschlüsselung: nur mit dem richtigen Code lesbar, Veränderung wird erkannt", () => {
  const a = channel("play", "ABC-DEF-GHJ"), b = channel("play", "abcdefghj"), c = channel("play", "ABC-DEF-GHK");
  assert.strictEqual(a.topic, b.topic, "Schreibweise des Codes egal");
  assert.notStrictEqual(a.topic, c.topic);
  const msg = {y:"pos", n:"Max", x:1.5, big:"x".repeat(5000)};
  const buf = seal(a, msg);
  assert.deepStrictEqual(open(b, buf), msg);
  assert.strictEqual(open(c, buf), null, "falscher Code");
  const t = Buffer.from(buf); t[40] ^= 1;
  assert.strictEqual(open(a, t), null, "verändert");
  assert.ok(!buf.toString("latin1").includes("Max"), "Klartext nicht sichtbar");
});

test("Kanal teilt große Nachrichten und setzt sie wieder zusammen", async () => {
  const bus = new Bus(), got = [];
  const k1 = new Kanal(new FakeNet(bus), "edit", "QQQ-WWW-EEE", () => {});
  const k2 = new Kanal(new FakeNet(bus), "edit", "QQQ-WWW-EEE", o => got.push(o));
  const big = {y:"ops", list:[...Array(3000).keys()].map(i => ({s:i, txt:"Straße " + i + " ".repeat(40)}))};
  k1.send(big);
  await until(() => got.length === 1, 5000);
  assert.deepStrictEqual(got[0], big);
  k1.close(); k2.close();
});

test("Snappy-Entpacken (für den Speicher der alten exe)", () => {
  // "abcabcabcabc": Literal "abc" + Kopie (Länge 9, Abstand 3)
  const comp = Buffer.from([12, 0x08, 0x61, 0x62, 0x63, ((9 - 4) << 2) | 1, 3]);
  assert.strictEqual(snappy(comp).toString(), "abcabcabcabc");
});
