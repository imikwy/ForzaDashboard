#!/usr/bin/env node
// Schickt simulierte Forza-Horizon-Telemetrie an das Dashboard (zum Testen ohne Spiel).
//   node tools/telemetrie-simulator.js [port=5400] [x=-1042] [z=-4608]
// Das Auto fährt einen großen Kreis um die angegebene Stelle.
"use strict";
const dgram = require("dgram");
const port = +process.argv[2] || 5400, cx = +process.argv[3] || -1042, cz = +process.argv[4] || -4608;
const sock = dgram.createSocket("udp4"), t0 = Date.now();
setInterval(() => {
  const t = (Date.now() - t0) / 1000, a = t * 0.08, r = 180, v = r * 0.08;   // ~14 m/s
  const b = Buffer.alloc(324);
  b.writeInt32LE(1, 0); b.writeUInt32LE(Date.now() >>> 0, 4);
  b.writeFloatLE(8000, 8); b.writeFloatLE(900, 12); b.writeFloatLE(3500 + 1500 * Math.sin(t), 16);
  const yaw = -a + Math.PI;   // Fahrtrichtung entlang des Kreises
  b.writeFloatLE(yaw, 56);
  b.writeInt32LE(3412, 212); b.writeInt32LE(5, 216); b.writeInt32LE(800, 220); b.writeInt32LE(1, 224); b.writeInt32LE(6, 228);
  const o = 244;
  b.writeFloatLE(cx + r * Math.cos(a), o); b.writeFloatLE(10, o + 4); b.writeFloatLE(cz + r * Math.sin(a), o + 8);
  b.writeFloatLE(v, o + 12); b.writeFloatLE(90000, o + 16); b.writeFloatLE(300, o + 20);
  for (let i = 0; i < 4; i++) b.writeFloatLE(85, o + 24 + i * 4);
  b[o + 71] = 180; b[o + 75] = 3;
  sock.send(b, port, "127.0.0.1");
}, 1000 / 30);
console.log("Simuliere Telemetrie an Port " + port + " (Strg+C beendet)");
