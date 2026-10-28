"use strict";
const {DATA, Bus, FakeNet, until, wait} = require("./helfer");
const test = require("node:test"), assert = require("node:assert");
const fs = require("fs"), path = require("path");
const {Sitzung} = require("../app/lib/sitzung");

const BASE = [{id:"a", k:"r", t:"road", lv:0, sm:1, p:[0, 0, 10, 10], ts:1, a:"x"}, {id:"e0", k:"e", x:5, z:5, r:3, ts:1, a:"x"}];
const karten = {ents:v => v === 1 ? BASE : null, has:v => v === 1, raw:() => null, install:() => "nein"};
let n = 0;
function editor(bus, dev, code, host){
  const dir = path.join(DATA, "pc-" + dev + "-" + (++n));
  const s = new Sitzung({code, title:"T", base:host ? 1 : 0, term:host ? 1 : 0}, {net:new FakeNet(bus), karten, dev, name:() => dev.toUpperCase(), dir});
  s.start(host); return s;
}
const stateOf = s => JSON.stringify([...s.state.values()].map(e => [e.id, e.d || 0, e.t || "", e.p || ""]).sort());
const road = (id, t) => ({id, k:"r", t:t || "road", lv:0, sm:1, p:[1, 1, 2, 2, 3, 1]});

test("Drei Editoren: alle sehen dieselben Schritte in derselben Reihenfolge", async () => {
  const bus = new Bus(), code = "AAABBBCCC";
  const h = editor(bus, "hhhhhhhhhh", code, true), a = editor(bus, "aaaaaaaaaa", code), b = editor(bus, "bbbbbbbbbb", code);
  await until(() => a.ready && b.ready && a.hostDev === h.dev && b.hostDev === h.dev, 5000, "beigetreten");
  for (let i = 0; i < 10; i++) { a.submit([road("ra" + i)]); b.submit([road("rb" + i, "gravel")]); h.submit([road("rh" + i)]); }
  await until(() => h.seq === 30 && a.seq === 30 && b.seq === 30 && !a.pending.size && !b.pending.size, 5000, "30 Schritte überall");
  assert.deepStrictEqual(a.ops.map(o => o.id), h.ops.map(o => o.id));
  assert.deepStrictEqual(b.ops.map(o => o.id), h.ops.map(o => o.id));
  assert.strictEqual(stateOf(a), stateOf(h)); assert.strictEqual(stateOf(b), stateOf(h));
  for (const s of [h, a, b]) s.stop();
});

test("Verlauf: Schritte des anderen rückgängig machen, Konflikt-Nachfrage, Wiederholen", async () => {
  const bus = new Bus(), code = "DDDEEEFFF";
  const h = editor(bus, "hhhhhhhhhh", code, true), a = editor(bus, "aaaaaaaaaa", code);
  await until(() => a.ready && a.hostDev === h.dev, 5000);
  a.submit([road("neu")]);
  await until(() => h.seq === 1);
  h.submit([Object.assign(road("neu"), {t:"gravel"})]);   // Host ändert danach dieselbe Straße
  await until(() => a.seq === 2);
  const g1 = a.history().find(g => g.lb.startsWith("Neue Straße")).g;
  const r = a.undo(g1, false);
  assert.ok(r.conflict && r.conflict.includes("HHHHHHHHHH"), "fragt nach, weil HHHHHHHHHH danach geändert hat");
  assert.ok(a.undo(g1, true).ok);
  await until(() => h.seq === 3 && a.seq === 3);
  assert.strictEqual(h.state.get("neu").d, 1, "Straße ist wieder weg");
  const hist = h.history();
  assert.ok(hist.find(g => g.g === g1).undone);
  // Wiederholen = das Rückgängig rückgängig machen
  const undoG = hist.find(g => g.isUndo).g;
  assert.ok(h.undo(undoG, true).ok);
  await until(() => a.seq === 4);
  assert.strictEqual(a.state.get("neu").t, "gravel", "Stand direkt vor dem Rückgängig ist zurück");
  assert.ok(!a.history().find(g => g.g === g1).undone);
  // Strg+Z nimmt den eigenen letzten Schritt
  assert.strictEqual(a.lastOwn(false), g1);
  for (const s of [h, a]) s.stop();
});

test("Radier-Wisch mit mehreren Teilen = ein Eintrag im Verlauf", async () => {
  const bus = new Bus(), code = "GGGHHHJJJ";
  const h = editor(bus, "hhhhhhhhhh", code, true);
  for (let i = 0; i < 5; i++) h.submit([{id:"w" + i, k:"e", x:i, z:0, r:2}], {grp:"wisch1"});
  await until(() => h.seq === 5);
  const hist = h.history();
  assert.strictEqual(hist.length, 1); assert.match(hist[0].lb, /Radiert · 5 Stellen/);
  assert.ok(h.undo(hist[0].g, false).ok);
  await until(() => h.seq === 6);
  for (let i = 0; i < 5; i++) assert.strictEqual(h.state.get("w" + i).d, 1);
  h.stop();
});

test("Host fällt aus: anderer übernimmt, ausstehende Änderungen gehen nicht verloren", async () => {
  const bus = new Bus(), code = "KKKMMMNNN";
  const h = editor(bus, "aaaaaaaaaa", code, true), b = editor(bus, "bbbbbbbbbb", code);
  await until(() => b.ready && b.hostDev === h.dev, 5000);
  b.submit([road("vorher")]); await until(() => b.seq === 1 && !b.pending.size);
  h.ctx.net.ok = false; h.stop();                          // Host weg (Absturz)
  b.submit([road("waehrend")]);                            // bleibt ausstehend
  assert.strictEqual(b.pending.size, 1);
  b.lastHost = Date.now() - 20000;                         // Wartezeit abkürzen
  b.tick();
  await until(() => b.role === "host" && b.seq === 2 && !b.pending.size, 5000, "B übernimmt");
  assert.ok(b.term >= 2);
  // ausstehende Änderungen überleben auch einen Neustart (pending.json)
  b.stop();
  const b2 = new Sitzung(Object.assign({}, b.info), {net:new FakeNet(bus), karten, dev:b.dev, name:() => "B", dir:path.dirname(b.dir)});
  assert.strictEqual(b2.seq, 2, "Journal wurde neu geladen");
  assert.ok(b2.state.get("waehrend"));
});

test("Zwei Hosts nach Netz-Trennung: Abgleich ohne Verlust", async () => {
  const bus = new Bus(), code = "PPPQQQRRR";
  const a = editor(bus, "aaaaaaaaaa", code, true), b = editor(bus, "bbbbbbbbbb", code);
  await until(() => b.ready && b.hostDev === a.dev, 5000);
  a.submit([road("gemeinsam")]); await until(() => b.seq === 1);
  // Netz getrennt: B wird selbst Host und arbeitet weiter, A auch
  bus.drop = (from, to) => from !== to;
  b.lastHost = Date.now() - 20000; b.tick();
  await until(() => b.role === "host");
  a.submit([road("nurA")]); b.submit([road("nurB")]);
  await until(() => a.seq === 2 && b.seq === 2);
  // Netz wieder da: der "bessere" Host gewinnt, der andere reicht seine Schritte neu ein
  bus.drop = () => false;
  a.hb(); b.hb();
  await until(() => a.seq === 3 && b.seq === 3 && !a.pending.size && !b.pending.size, 5000, "abgeglichen");
  assert.deepStrictEqual(a.ops.map(o => o.id), b.ops.map(o => o.id));
  for (const id of ["gemeinsam", "nurA", "nurB"]) { assert.ok(a.state.get(id), id + " bei A"); assert.ok(b.state.get(id), id + " bei B"); }
  assert.strictEqual((a.role === "host" ? 1 : 0) + (b.role === "host" ? 1 : 0), 1, "genau ein Host");
  for (const s of [a, b]) s.stop();
});

test("Ungültige Änderungen werden abgelehnt und nicht verteilt", async () => {
  const bus = new Bus(), code = "SSSTTTUUU";
  const h = editor(bus, "hhhhhhhhhh", code, true), a = editor(bus, "aaaaaaaaaa", code);
  await until(() => a.ready && a.hostDev === h.dev, 5000);
  a.submit([{id:"kaputt", k:"r", p:[1]}]);
  await until(() => !a.pending.size, 5000, "abgelehnt");
  await wait(50);
  assert.strictEqual(h.seq, 0); assert.strictEqual(a.seq, 0);
  for (const s of [h, a]) s.stop();
});
