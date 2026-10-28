// Selbst gelernte Straßen (gehören nur dir, nicht zur gemeinsamen Karte) und die gelernten Straßen der Freunde.
"use strict";
const fs = require("fs");
const {P} = require("./basis");
const {readJson, writeAtomic, writeJson, sha256, log} = require("./util");

const learned = {v:0, lease:null, leaseT:0};
try { learned.v = parseInt(fs.readFileSync(P.LEARNED_VER, "utf8"), 10) || 0; } catch (e) {}

function getLearned(){
  let data = "null"; try { data = fs.readFileSync(P.LEARNED, "utf8").trim() || "null"; } catch (e) {}
  return '{"v":' + learned.v + ',"data":' + data + '}';
}
// Speichern nur, wenn der Browser auf dem aktuellen Stand war (sonst würde ein zweites Gerät Änderungen überschreiben)
function postLearned(base, text){
  if (base !== learned.v || !text.trimStart().startsWith("{")) return {ok:false, v:learned.v};
  try { JSON.parse(text); } catch (e) { return {ok:false, v:learned.v}; }
  try {
    if (fs.existsSync(P.LEARNED)) fs.copyFileSync(P.LEARNED, P.LEARNED + ".bak");
    writeAtomic(P.LEARNED, text);
    learned.v++; writeAtomic(P.LEARNED_VER, String(learned.v));
    return {ok:true, v:learned.v};
  } catch (e) { log("Gelernte Straßen nicht gespeichert:", e.message); return {ok:false, v:learned.v}; }
}
// nur ein Browser-Fenster lernt gleichzeitig (sonst zählt jede Fahrt doppelt)
function lease(id){
  const now = Date.now(); if (!id) return {learner:false};
  if (!learned.lease || learned.lease === id || now - learned.leaseT > 15000) { learned.lease = id; learned.leaseT = now; }
  return {learner:learned.lease === id};
}

// ---- Freunde ----
const flr = {list:new Map(), v:0};
try { const o = readJson(P.FRIENDS_LR, {}); for (const k in o) flr.list.set(k, o[k]); flr.v = 1; } catch (e) {}
let saveT = null;
function flrSave(){ clearTimeout(saveT); saveT = setTimeout(() => { const o = {}; for (const [k, v] of flr.list) o[k] = v; try { writeJson(P.FRIENDS_LR, o); } catch (e) {} }, 1500); }
function onFriendRoads(id, o){
  const prev = flr.list.get(id);
  if (prev && prev.ts >= o.ts) return;
  flr.list.set(id, {n:String(o.n || "Freund").slice(0, 24), ts:o.ts || Date.now(), roads:(o.roads || []).slice(0, 5000), cands:(o.cands || []).slice(0, 8000), erase:[], pois:[]});
  flr.v++; flrSave();
}
function importFriend(o){
  const n = String(o.n || "Freund").slice(0, 24), id = "imp-" + sha256(n).slice(0, 8);
  flr.list.set(id, {n, ts:Date.now(), roads:Array.isArray(o.roads) ? o.roads.slice(0, 5000) : [], cands:Array.isArray(o.cands) ? o.cands.slice(0, 8000) : [], erase:[], pois:[]});
  flr.v++; flrSave(); return {ok:true};
}
function friendList(on){ const list = []; if (on) for (const [id, o] of flr.list) list.push(Object.assign({id}, o)); return {v:flr.v, list}; }

module.exports = {learned, getLearned, postLearned, lease, flr, onFriendRoads, importFriend, friendList};
