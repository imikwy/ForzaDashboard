"use strict";
const {PEM} = require("./helfer");
const test = require("node:test"), assert = require("node:assert");
const fs = require("fs"), path = require("path");
const K = require("../app/lib/kartenpruefung");
const SIG = require("../app/lib/signatur");
const {KartenSpeicher, buildManifest} = require("../app/lib/karte");
const {P} = require("../app/lib/basis");

const road = (id, ts, extra) => Object.assign({id, k:"r", t:"road", lv:0, sm:1, p:[0, 0, 10, 10, 20, 0], ts, a:"x"}, extra || {});
function release(v, ents){
  const buf = Buffer.from(K.serialize({version:v}, ents), "utf8");
  return {man:SIG.sign(buildManifest(v, buf, ents), PEM), buf};
}

test("Prüfung erkennt kaputte Objekte", () => {
  assert.ok(K.validate([road("a", 1), {id:"e1", k:"e", x:1, z:2, r:5}, {id:"x:a", k:"x"}, {id:"cfg:mapfit", k:"c", T:[1, 0, 0, 1, 0, 0]}]).ok);
  assert.ok(!K.validate([{id:"b", k:"r", p:[1, 2]}]).ok, "Straße mit nur einem Punkt");
  assert.ok(!K.validate([{id:"b", k:"r", p:[0, 0, NaN, 1]}]).ok, "NaN");
  assert.ok(!K.validate([{id:"b", k:"q"}]).ok, "unbekannte Art");
  assert.ok(!K.validate([road("a", 1), road("a", 2)]).ok, "doppelte id");
  assert.ok(K.validate([{id:"a", k:"r", d:1}]).ok, "Grabstein ist gültig");
});

test("Einheitliches Format ist stabil (gleiche Karte = gleiche Bytes)", () => {
  const a = [road("b", 2), road("a", 1, {p:[0.04, 0, 10.06, 10, 20, 0]})];
  const b = [Object.assign({}, a[1], {}), a[0]].map(e => { const o = {}; for (const k of Object.keys(e).reverse()) o[k] = e[k]; return o; });
  assert.strictEqual(K.serialize({}, a), K.serialize({}, b));
  assert.match(K.serialize({}, a), /"p":\[0,0,10.1,10,20,0\]/);
});

test("Schutz gegen Massenlöschung", () => {
  const old = [...Array(1000).keys()].map(i => road("r" + i, 1));
  const fewer = old.map((e, i) => i < 300 ? {id:e.id, k:"r", d:1, ts:2} : e);
  assert.ok(K.guard(K.diff(old, fewer)).length, "300 von 1000 gelöscht -> blockiert");
  assert.deepStrictEqual(K.guard(K.diff(old, old.map((e, i) => i < 10 ? {id:e.id, k:"r", d:1, ts:2} : e))), [], "10 gelöscht ist ok");
  assert.ok(K.guard(K.diff(old, old.slice(1))).length, "verschwundenes Objekt -> blockiert");
});

test("Zusammenführen: neuere Änderung gewinnt, Löschungen bleiben", () => {
  const m = K.merge([road("a", 5), road("b", 1)], [road("a", 3, {t:"gravel"}), {id:"b", k:"r", d:1, ts:4}, road("c", 1)]);
  const by = id => m.find(e => e.id === id);
  assert.strictEqual(by("a").t, "road"); assert.strictEqual(by("b").d, 1); assert.ok(by("c"));
});

test("Kartenspeicher: nur signierte Versionen, Manipulation wird erkannt und repariert", () => {
  const ks = new KartenSpeicher(); ks.bundled = () => null; ks.init();
  const r1 = release(1, [road("a", 1)]);
  assert.strictEqual(ks.install(r1.man, r1.buf), null);
  assert.strictEqual(ks.active.version, 1);
  // falsche Signatur
  const r2 = release(2, [road("a", 1), road("b", 2)]), bad = Object.assign({}, r2.man, {objekte:2, notiz:"geändert"});
  assert.match(ks.install(bad, r2.buf), /Signatur/);
  // verändertes Paket
  assert.match(ks.install(r2.man, Buffer.concat([r2.buf, Buffer.from(" ")])), /Prüfsumme/);
  assert.strictEqual(ks.install(r2.man, r2.buf), null);
  assert.strictEqual(ks.active.version, 2);
  // Datei auf der Platte beschädigt -> beim Start zurück auf die letzte gute Version
  const f = path.join(P.KARTEN, "karte-v2.json"); fs.chmodSync(f, 0o644); fs.appendFileSync(f, "x");
  const ks2 = new KartenSpeicher(); ks2.bundled = () => null; ks2.init();
  assert.strictEqual(ks2.active.version, 1);
  assert.strictEqual(ks2.install(r2.man, r2.buf), null, "beschädigte Datei wird beim erneuten Laden repariert");
  assert.strictEqual(ks2.active.version, 2);
  // ältere Versionen bleiben abrufbar
  assert.strictEqual(ks2.ents(1).length, 1);
});
