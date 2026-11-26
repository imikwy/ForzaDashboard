// Regel-Simulation für den Police-Chase-Code aus src/dashboard.html (mit Testpositionen)
const fs = require("fs"), vm = require("vm"), assert = require("assert");
const html = fs.readFileSync(process.argv[2] || require("path").join(__dirname, "../src/dashboard.html"), "utf8");
const a = html.indexOf("// ---- Police Chase: Polizei gegen Flüchtige"), b = html.indexOf("// ---- Handbremse:");
const code = html.slice(a, b);
let T = 1_000_000;
function mk(me, leader){
  const toasts = [], posts = [], pill = {hidden:true, dataset:{}, className:"", innerHTML:""};
  class FDate extends Date { static now(){ return T; } }
  const ctx = {
    Date:FDate, performance:{now:() => T}, setInterval(){}, Math, String, Set, Map, Object, isFinite, Promise, console,
    toast:t => toasts.push(t), sessPost:(p, body) => { posts.push([p, body]); return Promise.resolve({ok:true}); },
    $:() => pill, esc:t => String(t), mirror:() => 1, CAR:{ok:false}, D:{on:true, dash:true, px:1, pz:0, car:1, spd:0},
    kolType:n => n === "Police Chase" ? ["police", "Police Chase"] : null,
    FR:{lobbys:[{code:"AAA-BBB-CCC", an:true, name:"Police Chase"}], m:new Map(), kc:null},
  };
  ctx.kolInfo = l => ({kk:ctx.FR.kc.kol[0]});
  // letzte Telemetrie: kommt laufend an, außer der Test schaltet sie ab (Spiel zu / Verbindung weg)
  let quiet = false; Object.defineProperty(ctx, "lastMsg", {get: () => quiet ? T - 5000 : T});
  ctx.__quiet = v => { quiet = v; };
  vm.createContext(ctx);
  vm.runInContext(code, ctx);
  const kk = {code:"AAA-BBB-CCC", me, leader, mem:[], ev:{}, pc:null};
  ctx.FR.kc = {kol:[kk]};
  return {ctx, kk, toasts, posts, pill, run:s => vm.runInContext(s, ctx)};
}
const START = T, H = mk("X", "X").run("PCR.head");   // Vorsprung aus dem Dashboard
const round = (o) => Object.assign({rid:"AB12CD", start:START, dur:20 * 60e3, cops:["C1", "C2"], runs:["R1", "R2"], nm:{C1:"Cop1", C2:"Cop2", R1:"Run1", R2:"Run2"}, end:0, res:""}, o || {});
// Zeit vorspulen und jede 100 ms einen Schritt rechnen; f(t) kann Positionen setzen
function adv(w, ms, f){ const end = T + ms; while (T < end) { T += 100; if (f) f(T); w.run("pcStep()"); } }
let ok = 0; const t = (name, fn) => { fn(); ok++; console.log("✓", name); };

t("Polizisten je Spielerzahl", () => {
  const w = mk("X", "X");
  const got = [4, 5, 6, 7, 12, 13, 18, 19].map(n => w.run("pcCops(" + n + ")"));
  assert.deepStrictEqual(got, [2, 2, 2, 4, 4, 6, 6, 8]);
  assert.deepStrictEqual([0, 2, 3, 5, 6].map(s => w.run("pcNeed(" + s + ")")), [10, 10, 8, 8, 6]);
  assert.deepStrictEqual([0, 3, 4, 5, 6].map(s => w.run("pcEvery(" + s + ")")), [30e3, 30e3, 20e3, 20e3, 15e3]);
});

t("Flüchtiger: Vorsprung zählt nicht, Sterne beim Eintritt, erst nach >400 m und 15 s neu", () => {
  T = START; const w = mk("R1", "C1"); w.kk.mem = [{d:"C1"}, {d:"C2"}, {d:"R2"}]; w.kk.pc = round();
  const cop = {tx:100, tz:0, g:1, n:"Cop1"}; w.ctx.FR.m.set("C1", cop);
  adv(w, H - 2e3);                                  // Vorsprung: Polizist 100 m entfernt
  assert.strictEqual(w.run("PC.st"), 0, "kein Stern im Vorsprung");
  assert.ok(w.toasts[0].includes(H / 1000 + " s Vorsprung"), w.toasts[0]);
  adv(w, 3e3);                                      // Jagd läuft
  assert.strictEqual(w.run("PC.st"), 1, "erster Stern");
  adv(w, 20e3);                                     // drin bleiben: kein weiterer Stern
  assert.strictEqual(w.run("PC.st"), 1);
  cop.tx = 380; adv(w, 2e3); cop.tx = 200; adv(w, 20e3);   // nur 380 m raus (nicht >400) – zählt nicht neu
  assert.strictEqual(w.run("PC.st"), 1, "380 m reicht nicht");
  cop.tx = 450; adv(w, 3e3); cop.tx = 200; adv(w, 1e3);    // >400 m raus und wieder rein
  assert.strictEqual(w.run("PC.st"), 2, "zweiter Stern");
  cop.tx = 450; adv(w, 2e3); cop.tx = 200; adv(w, 1e3);    // sofort wieder: < 15 s seit letztem Stern
  assert.strictEqual(w.run("PC.st"), 2, "15-s-Sperre");
  adv(w, 15e3);                                     // drin geblieben, Sperre abgelaufen -> zählt (war ja > 400 m draußen)
  assert.strictEqual(w.run("PC.st"), 3);
  const sent = w.posts.filter(p => p[0] === "/lobby/ev").map(p => p[1].pc.st);
  assert.deepStrictEqual(sent, [0, 1, 2, 3], "Sterne werden gemeldet");
  for (let i = 0; i < 6; i++) { cop.tx = 450; adv(w, 2e3); cop.tx = 200; adv(w, 16e3); }
  assert.strictEqual(w.run("PC.st"), 6, "höchstens 6 Sterne");
});

t("Flüchtiger: 10 s im Fangkreis = gefasst, draußen läuft der Zähler doppelt zurück", () => {
  T = START; const w = mk("R1", "C1"); w.kk.mem = [{d:"C1"}, {d:"C2"}, {d:"R2"}]; w.kk.pc = round();
  const cop = {tx:30, tz:0, g:1, n:"Cop1"}; w.ctx.FR.m.set("C1", cop);
  adv(w, H + 1e3, () => { if (T < START + H) cop.tx = 1000; else cop.tx = 30; });
  assert.strictEqual(w.run("PC.st"), 1);             // Stern zählt -> Fangzeit noch 10 s (ab 3 Sternen 8 s)
  adv(w, 6e3); assert.ok(Math.abs(w.run("PC.ct") - 6.9) < 0.3, "≈6–7 s gesammelt: " + w.run("PC.ct"));
  cop.tx = 60; adv(w, 2e3);                          // 2 s draußen: -4 s
  assert.ok(Math.abs(w.run("PC.ct") - 2.9) < 0.3, "zurückgelaufen: " + w.run("PC.ct"));
  cop.g = 0; cop.tx = 20; adv(w, 3e3);               // Polizist im Menü: fängt nicht
  assert.ok(w.run("PC.ct") < 0.01);
  cop.g = 1; adv(w, 9.5e3); assert.strictEqual(w.run("PC.out"), false, "9,5 s reichen nicht");
  adv(w, 0.7e3); assert.strictEqual(w.run("PC.out"), true, "nach 10 s gefasst");
  assert.strictEqual(w.run("PC.by"), "C1");
  const last = w.posts.filter(p => p[0] === "/lobby/ev").pop()[1].pc;
  assert.deepStrictEqual([last.out, last.by], [true, "C1"]);
  assert.ok(w.toasts.some(x => x.includes("Gefasst von Cop1")));
});

t("Flüchtiger mit 3 Sternen: 8 s, mit 6 Sternen: 6 s", () => {
  for (const [st, need] of [[3, 8], [6, 6]]) {
    T = START + 120e3; const w = mk("R1", "C1"); w.kk.mem = [{d:"C1"}, {d:"C2"}, {d:"R2"}]; w.kk.pc = round();
    w.kk.ev.R1 = {pc:{rid:"AB12CD", st, out:false}};   // Stand nach Neustart des Dashboards wiederherstellen
    const cop = {tx:30, tz:0, g:1, n:"Cop1"}; w.ctx.FR.m.set("C1", cop);
    adv(w, (need - 0.5) * 1000); assert.strictEqual(w.run("PC.out"), false);
    adv(w, 700); assert.strictEqual(w.run("PC.out"), true, st + " Sterne -> " + need + " s");
  }
});

t("Polizist: Flüchtige nur im Suchkreis sichtbar, Ping alle 30 s (ab 4 Sternen 20 s)", () => {
  T = START; const w = mk("C1", "C1"); w.kk.mem = [{d:"C2"}, {d:"R1"}, {d:"R2"}]; w.kk.pc = round();
  w.ctx.D.px = 1; w.ctx.D.pz = 0;
  const list = () => [{id:"C2", n:"Cop2", x:2000, z:0}, {id:"R1", n:"Run1", x:R1x, z:0}, {id:"R2", n:"Run2", x:2300, z:0}];
  let R1x = 200;
  const vis = () => w.ctx.pcFilter(list(), w.ctx.FR.kc).map(f => f.id).join(",");
  assert.strictEqual(vis(), "C2", "im Vorsprung niemand sichtbar");
  T = START + H + 1e3;
  assert.strictEqual(vis(), "C2,R1,R2", "R1 in meinem, R2 im Kreis von Cop2 (300 m)");
  R1x = 500; assert.strictEqual(vis(), "C2,R2", "R1 draußen = verborgen");
  assert.strictEqual(w.run('pcHid("R1")'), true);
  // Pings: Positionen zu den gemeinsamen Zeitpunkten (Start + Vorsprung + k·30 s)
  T = START + H; w.run("PC.lastT = 0; PC.rid = ''"); R1x = 500;
  const pingAt = [];
  adv(w, 95e3, () => { R1x = 500 + (T - START) / 100; w.ctx.pcFilter(list(), w.ctx.FR.kc); const g = w.run("PC.pings.get(\"R1\")"); if (g && !pingAt.includes(g.at)) pingAt.push(g.at); });
  assert.deepStrictEqual(pingAt.map(x => Math.round((x - START) / 1000)), [0, 30, 60, 90].map(x => x + H / 1000), "Pings alle 30 s ab Ende des Vorsprungs");
  w.kk.ev.R1 = {pc:{rid:"AB12CD", st:4, out:false}};
  const n0 = pingAt.length; adv(w, 41e3, () => { w.ctx.pcFilter(list(), w.ctx.FR.kc); const g = w.run("PC.pings.get(\"R1\")"); if (g && !pingAt.includes(g.at)) pingAt.push(g.at); });
  assert.deepStrictEqual(pingAt.slice(n0).map(x => Math.round((x - START) / 1000)), [95, 100, 120].map(x => x + H / 1000), "Wechsel auf 20-s-Raster (sofort neuer Ping, dann 160, 180)");
  assert.ok(w.pill.innerHTML.includes("Ping") && w.pill.className.includes("cop"), w.pill.innerHTML);
});

t("Kolonnenführer: alle gefasst -> nach 8 s Polizei gewinnt; Zeit um -> Flüchtige gewinnen", () => {
  T = START + 100e3; let w = mk("C1", "C1"); w.kk.mem = [{d:"C2"}, {d:"R1"}, {d:"R2"}]; w.kk.pc = round();
  w.kk.ev.R1 = {pc:{rid:"AB12CD", st:2, out:true, by:"C1"}}; w.kk.ev.R2 = {pc:{rid:"AB12CD", st:1, out:false}};
  adv(w, 10e3); assert.strictEqual(w.posts.filter(p => p[0] === "/lobby/pc").length, 0, "R2 noch frei");
  assert.ok(w.toasts.some(x => x.includes("Run1 wurde gefasst") ) === false, "schon beim Laden gefasst: keine Meldung");
  w.kk.ev.R2.pc.out = true; w.kk.ev.R2.pc.by = "C2";
  adv(w, 5e3); assert.strictEqual(w.posts.filter(p => p[0] === "/lobby/pc").length, 0, "erst nach 8 s");
  assert.ok(w.toasts.some(x => x.includes("Run2 wurde gefasst – von Cop2")));
  adv(w, 4e3); const e = w.posts.filter(p => p[0] === "/lobby/pc"); assert.strictEqual(e.length, 1); assert.strictEqual(e[0][1].res, "cop");
  // Zeit um
  T = START + 20 * 60e3 - 2e3; w = mk("C1", "C1"); w.kk.mem = [{d:"C2"}, {d:"R1"}, {d:"R2"}]; w.kk.pc = round();
  w.kk.ev.R1 = {pc:{rid:"AB12CD", st:5, out:false}};
  adv(w, 3e3); const e2 = w.posts.filter(p => p[0] === "/lobby/pc"); assert.strictEqual(e2.length, 1); assert.strictEqual(e2[0][1].res, "run");
  assert.ok(w.pill.innerHTML.includes("Flüchtige gewinnen"), w.pill.innerHTML);
  // kein Führer: sendet nichts
  T = START + 20 * 60e3 + 1e3; w = mk("C2", "C1"); w.kk.mem = [{d:"C1"}, {d:"R1"}, {d:"R2"}]; w.kk.pc = round(); adv(w, 6e3);
  assert.strictEqual(w.posts.filter(p => p[0] === "/lobby/pc").length, 0);
});

t("Pille: Flüchtiger mit Sternen und Fang-Balken, nach dem Ende verschwindet sie", () => {
  T = START + 100e3; const w = mk("R1", "C1"); w.kk.mem = [{d:"C1"}, {d:"C2"}, {d:"R2"}]; w.kk.pc = round();
  w.kk.ev.R1 = {pc:{rid:"AB12CD", st:2, out:false}};
  const cop = {tx:30, tz:0, g:1, n:"Cop1"}; w.ctx.FR.m.set("C1", cop);
  adv(w, 2e3);
  assert.ok(w.pill.innerHTML.includes("<i>★</i><i>★</i>") && w.pill.innerHTML.includes('class="bar"') && w.pill.className.includes("hot"), w.pill.innerHTML);
  assert.ok(w.pill.innerHTML.includes("18:"), "Restzeit");
  w.kk.pc = round({end:T, res:"cop"}); adv(w, 1e3); assert.ok(w.pill.innerHTML.includes("Polizei gewinnt"));
  adv(w, 46e3); assert.strictEqual(w.pill.hidden, true);
});
console.log(ok + " Prüfungen bestanden");
t("Kolonnen-Menü rendert (ohne Runde, laufend, Ergebnis)", () => {
  T = START + 100e3; const w = mk("C1", "C1"); w.kk.mem = [{d:"C2", n:"Cop2"}, {d:"R1", n:"Run1"}, {d:"R2", n:"Run2"}];
  const l = w.ctx.FR.lobbys[0], mem = [{d:"C1", n:"Ich", self:true}].concat(w.kk.mem);
  let h = w.ctx.kolPolice(l, mem, {}, {role:"cop"}); assert.ok(h.includes("2 Polizei · 2 Flüchtige") && h.includes('data-a="pcstart"') && !h.includes("disabled"), h);
  h = w.ctx.kolPolice(l, mem.slice(0, 3), {}, {}); assert.ok(h.includes("Noch 1 Fahrer fehlen") && h.includes("disabled"));
  w.kk.pc = round(); w.kk.ev = {R1:{pc:{rid:"AB12CD", st:3, out:true, by:"C1"}}};
  h = w.ctx.kolPolice(l, mem, w.kk.ev, {}); assert.ok(h.includes("Läuft · noch") && h.includes("gefasst von dir") && !h.includes("Du <span") && h.includes("1 Festnahme") && h.includes('data-a="pcend"'), h);
  w.kk.pc = round({end:T, res:"cop"}); h = w.ctx.kolPolice(l, mem, w.kk.ev, {}); assert.ok(h.includes("Polizei hat gewonnen") && h.includes("1 von 2 gefasst"), h);
});
t("Zuschauen: Sicht des Polizisten, aktive Polizisten nicht bei Flüchtigen", () => {
  T = START + 100e3; const w = mk("X9", "C1"); w.kk.mem = [{d:"C1"}, {d:"C2"}, {d:"R1"}, {d:"R2"}]; w.kk.pc = round();
  w.ctx.FR.m.set("C1", {id:"C1", n:"Cop1", tx:0, tz:0, x:0, z:0}); w.ctx.FR.m.set("R1", {id:"R1", n:"Run1"});
  const list = [{id:"C1", n:"Cop1", x:0, z:0}, {id:"C2", n:"Cop2", x:5000, z:0}, {id:"R1", n:"Run1", x:200, z:0}, {id:"R2", n:"Run2", x:900, z:0}];
  assert.strictEqual(w.ctx.pcFilter(list, w.ctx.FR.kc).length, 4, "Zuschauer ohne Zuschauen: alle");
  w.run("WATCH.id = \"C1\"");
  assert.deepStrictEqual(w.ctx.pcFilter(list, w.ctx.FR.kc).map(f => f.id), ["C1", "C2", "R1"], "Sicht von Cop1: Run2 verborgen");
  assert.strictEqual(w.run("pcHid(\"R2\")"), true);
  w.run("WATCH.id = \"R1\"");
  assert.strictEqual(w.ctx.pcFilter(list, w.ctx.FR.kc).length, 4, "Sicht eines Flüchtigen: alle");
  const c = mk("C2", "C1"); c.kk.mem = [{d:"C1"}, {d:"R1"}, {d:"R2"}]; c.kk.pc = round();
  assert.strictEqual(c.run("pcCanWatch(\"R1\")"), false, "Polizist darf keinem Flüchtigen zuschauen");
  assert.strictEqual(c.run("pcCanWatch(\"C1\")"), true);
  c.kk.pc = round({end:T, res:"cop"}); assert.strictEqual(c.run("pcCanWatch(\"R1\")"), true, "nach der Runde alles erlaubt");
});
console.log("fertig");

// ---------------- Pause-Schutz ----------------
// Flüchtiger geht ins Pausemenü (Forza: on = 0) oder es kommt nichts mehr: 10 s Zeit, zurückzukommen und loszufahren
const pauseWorld = (st) => {
  T = START + 60e3; const w = mk("R1", "C1"); w.kk.mem = [{d:"C1"}, {d:"C2"}, {d:"R2"}]; w.kk.pc = round();
  w.kk.ev.R1 = {pc:{rid:"AB12CD", st:st || 1, out:false}};
  w.ctx.FR.m.set("C1", {tx:2000, tz:0, g:1, n:"Cop1"});
  w.ctx.D.spd = 20; adv(w, 1e3); return w;
};
const evs = w => w.posts.filter(p => p[0] === "/lobby/ev").map(p => p[1].pc);
t("Pause: 9,5 s im Menü, dann zurück und losfahren -> bleibt im Spiel", () => {
  const w = pauseWorld();
  w.ctx.D.on = false; adv(w, 9.5e3);
  assert.strictEqual(w.run("PC.out"), false, "noch nicht raus");
  assert.ok(w.toasts.some(x => x.includes("Pause erkannt")), w.toasts.join(" | "));
  assert.ok(w.pill.innerHTML.includes("Zurück ins Spiel") && w.pill.className.includes("hot"), w.pill.innerHTML);
  const dl = evs(w).filter(e => e.dl).pop(); assert.ok(dl && Math.abs(dl.dl - (START + 61e3 + 10e3)) < 1500, "Frist an die anderen: " + (dl && dl.dl - START));
  w.ctx.D.on = true; w.ctx.D.spd = 15; adv(w, 0.3e3);
  assert.strictEqual(w.run("PC.pz"), false, "Pause vorbei");
  assert.strictEqual(evs(w).pop().dl, 0, "Frist aufgehoben gemeldet");
  adv(w, 60e3); assert.strictEqual(w.run("PC.out"), false);
  assert.ok(w.run("PC.pt") < 0.01, "Konto wieder voll: " + w.run("PC.pt"));
});
t("Pause: 10 s im Menü -> raus (wie gefasst, Grund Pause, keine Festnahme für die Polizei)", () => {
  const w = pauseWorld(3);
  w.ctx.D.on = false; adv(w, 9.8e3); assert.strictEqual(w.run("PC.out"), false);
  adv(w, 0.4e3); assert.strictEqual(w.run("PC.out"), true, "nach 10 s raus");
  const last = evs(w).pop(); assert.deepStrictEqual([last.out, last.by, last.why, last.dl, last.st], [true, "", "pause", 0, 3]);
  assert.ok(w.toasts.some(x => x.includes("Zu lange pausiert")));
  assert.ok(w.pill.innerHTML.includes("zu lange pausiert"), w.pill.innerHTML);
  w.ctx.D.on = true; adv(w, 5e3); assert.strictEqual(w.run("PC.out"), true, "bleibt raus");
});
t("Pause: zurück, aber stehen bleiben zählt weiter; Funkstille zählt als Pause", () => {
  let w = pauseWorld();
  w.ctx.D.on = false; adv(w, 4e3); w.ctx.D.on = true; w.ctx.D.spd = 0; adv(w, 5e3);
  assert.strictEqual(w.run("PC.out"), false); assert.strictEqual(w.run("PC.pz"), true, "steht noch -> Pause läuft");
  adv(w, 1.3e3); assert.strictEqual(w.run("PC.out"), true, "4 s Menü + 6 s Stehen = raus");
  w = pauseWorld(); w.ctx.__quiet(true); adv(w, 10.3e3); assert.strictEqual(w.run("PC.out"), true, "keine Daten mehr = Pause");
  w = pauseWorld(); w.ctx.D.px = 0; w.ctx.D.pz = 0; adv(w, 10.3e3); assert.strictEqual(w.run("PC.out"), true, "keine Position = Pause");
});
t("Pause: Konto füllt sich nur halb so schnell auf (mehrmals kurz pausieren hilft nicht)", () => {
  const w = pauseWorld();
  w.ctx.D.on = false; adv(w, 8e3); w.ctx.D.on = true; adv(w, 4e3);   // 8 s Pause, 4 s fahren -> Konto 6 s
  assert.ok(Math.abs(w.run("PC.pt") - 6) < 0.3, "Konto " + w.run("PC.pt"));
  w.ctx.D.on = false; adv(w, 3.7e3); assert.strictEqual(w.run("PC.out"), false);
  adv(w, 0.6e3); assert.strictEqual(w.run("PC.out"), true, "nach ~4 s statt 10 s raus");
});
t("Pause: Fangzähler bleibt stehen (wer im Fangkreis pausiert, ist danach sofort dran)", () => {
  const w = pauseWorld(); const cop = w.ctx.FR.m.get("C1"); cop.tx = 20;
  adv(w, 7e3); const ct = w.run("PC.ct"); assert.ok(ct > 6.5 && ct < 7.5, "ct " + ct);
  w.ctx.D.on = false; adv(w, 9e3); assert.ok(Math.abs(w.run("PC.ct") - ct) < 0.01, "steht still statt zurückzulaufen");
  w.ctx.D.on = true; adv(w, 3.5e3); assert.strictEqual(w.run("PC.out"), true); assert.strictEqual(w.run("PC.by"), "C1", "danach normal gefasst");
});
t("Pause im Vorsprung zählt nicht – erst ab Beginn der Jagd", () => {
  T = START; const w = mk("R1", "C1"); w.kk.mem = [{d:"C1"}, {d:"C2"}, {d:"R2"}]; w.kk.pc = round();
  w.ctx.FR.m.set("C1", {tx:2000, tz:0, g:1, n:"Cop1"}); w.ctx.D.on = false;
  adv(w, H - 1e3); assert.ok(w.run("PC.pt") === 0 && !w.run("PC.pz"));
  adv(w, 10.5e3); assert.strictEqual(w.run("PC.out"), false, "9,5 s nach Jagdbeginn noch drin");
  adv(w, 0.7e3); assert.strictEqual(w.run("PC.out"), true, "ab Jagdbeginn 10 s");
});
t("Polizei sieht den pausierten Flüchtigen kompakt, Meldung bei Disqualifikation, Führer wertet als Polizei-Sieg", () => {
  T = START + 100e3; const w = mk("C1", "C1"); w.kk.mem = [{d:"C2"}, {d:"R1", n:"Run1"}, {d:"R2", n:"Run2"}]; w.kk.pc = round();
  w.kk.ev.R1 = {pc:{rid:"AB12CD", st:2, out:false, dl:T + 7e3}}; w.kk.ev.R2 = {pc:{rid:"AB12CD", st:1, out:true, by:"C2"}};
  adv(w, 1e3);
  assert.ok(w.pill.innerHTML.includes("⏸") && w.pill.innerHTML.includes("Run1") && w.pill.innerHTML.includes("6 s"), w.pill.innerHTML);
  const l = w.ctx.FR.lobbys[0], mem = [{d:"C1", n:"Ich", self:true}].concat(w.kk.mem);
  let h = w.ctx.kolPolice(l, mem, w.kk.ev, {}); assert.ok(h.includes("pausiert – noch 6 s"), h);
  w.kk.ev.R1.pc = {rid:"AB12CD", st:2, out:true, by:"", why:"pause"}; adv(w, 1e3);
  assert.ok(w.toasts.some(x => x.includes("Run1 ist raus – zu lange pausiert")), w.toasts.join(" | "));
  h = w.ctx.kolPolice(l, mem, w.kk.ev, {}); assert.ok(h.includes("raus – zu lange pausiert"), h);
  assert.ok(!/Ich[^]*?1 Festnahme/.test(h), "keine Festnahme für die Polizei durch Pause");
  adv(w, 9e3); const e = w.posts.filter(p => p[0] === "/lobby/pc"); assert.strictEqual(e.length, 1); assert.strictEqual(e[0][1].res, "cop");
});
console.log("Pause-Schutz geprüft");
