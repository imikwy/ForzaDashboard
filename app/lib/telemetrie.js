// Forza "Data Out" (UDP) lesen – Forza Horizon und Forza Motorsport.
"use strict";
const r4 = v => (Number.isFinite(v) ? Math.round(v * 10000) / 10000 : 0);
function parse(b){
  const F = o => b.readFloatLE(o), I = o => b.readInt32LE(o), A4 = o => [r4(F(o)), r4(F(o + 4)), r4(F(o + 8)), r4(F(o + 12))];
  const d = {len:b.length, on:I(0), ts:b.readUInt32LE(4), rpmMax:r4(F(8)), rpmIdle:r4(F(12)), rpm:r4(F(16)),
    ax:r4(F(20)), ay:r4(F(24)), az:r4(F(28)), vx:r4(F(32)), vy:r4(F(36)), vz:r4(F(40)),
    yaw:r4(F(56)), pitch:r4(F(60)), roll:r4(F(64)),
    susp:A4(68), slipR:A4(84), slipA:A4(164), slip:A4(180), wrs:A4(100), surf:A4(148), suspM:A4(196),
    avx:r4(F(44)), avy:r4(F(48)), avz:r4(F(52)), rumble:[I(116), I(120), I(124), I(128)],
    car:I(212), cls:I(216), pi:I(220), drive:I(224), cyl:I(228)};
  const fh = b.length >= 323 && b.length !== 331;
  if (fh) { d.puddle = [I(132), I(136), I(140), I(144)]; d.grp = b.readUInt32LE(232); d.smV = r4(F(236)); d.smM = r4(F(240)); }
  else d.puddle = A4(132);
  let o = -1;
  if (fh) o = 244; else if (b.length >= 311) o = 232;
  if (o > 0) {
    Object.assign(d, {px:r4(F(o)), py:r4(F(o + 4)), pz:r4(F(o + 8)), spd:r4(F(o + 12)), pow:r4(F(o + 16)), tq:r4(F(o + 20)),
      tt:A4(o + 24), boost:r4(F(o + 40)), fuel:r4(F(o + 44)), dist:r4(F(o + 48)), best:r4(F(o + 52)), last:r4(F(o + 56)),
      cur:r4(F(o + 60)), rt:r4(F(o + 64)), lap:b.readUInt16LE(o + 68), pos:b[o + 70], acc:b[o + 71], brk:b[o + 72],
      clu:b[o + 73], hb:b[o + 74], gear:b[o + 75], steer:b.readInt8(o + 76), dash:1});
    if (b.length > o + 78) { d.line = b.readInt8(o + 77); d.aiBrake = b.readInt8(o + 78); }
    if (o === 232 && b.length >= 327) d.wear = A4(311);    // Forza Motorsport (2023): Reifenverschleiß
  } else d.dash = 0;
  return d;
}
module.exports = {parse};
