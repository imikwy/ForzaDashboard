// Spotify: Titel anzeigen, steuern, Lautstärke nur für Spotify (Windows-Lautstärkemixer).
"use strict";
const {execFile, spawn} = require("child_process");
const {S} = require("./basis");
function ps(cmd, cb){ execFile("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", cmd], {windowsHide:true, timeout:4000}, (err, out) => cb && cb(err ? "" : String(out))); }
let media = {running:false, playing:false, title:"", artist:""}, mediaWanted = 0;
// Spotify-Titel über "tasklist" lesen (leicht, startet kein PowerShell) – nur solange ein Dashboard die Musik anzeigt
function pollMedia(){
  if (process.platform !== "win32" || Date.now() - mediaWanted > 10000) return;
  execFile("tasklist", ["/v", "/fo", "csv", "/nh", "/fi", "imagename eq Spotify.exe"], {windowsHide:true, timeout:3000}, (err, out) => {
    if (err) return;
    let running = false, title = "";
    for (const line of String(out).split(/\r?\n/)) {
      const cols = line.match(/"([^"]*)"/g); if (!cols || cols.length < 9) continue;
      running = true; const t = cols[cols.length - 1].slice(1, -1);
      if (t && t !== "N/A" && t !== "Spotify" && t !== "Spotify Premium" && t !== "Spotify Free" && t.includes(" - ")) title = t;
    }
    const sep = title.indexOf(" - ");
    media = sep > 0 ? {running, playing:true, artist:title.slice(0, sep), title:title.slice(sep + 3)} : {running, playing:false, title:"", artist:""};
  });
}
setInterval(pollMedia, 3000).unref();
const KEYS = {playpause:179, next:176, prev:177, volup:175, voldown:174};
// Spotify-Lautstärke über den Windows-Lautstärkemixer (nur die Spotify-App, nicht der ganze PC).
// Läuft in einer dauerhaften PowerShell im Hintergrund; klappt das nicht, regeln die Knöpfe die PC-Lautstärke.
const SPV_PS = "$ErrorActionPreference = 'Stop'\ntry {\nAdd-Type -TypeDefinition @\"\nusing System;\nusing System.Diagnostics;\nusing System.Runtime.InteropServices;\n[ComImport, Guid(\"BCDE0395-E52F-467C-8E3D-C4579291692E\")] class FdMMDeviceEnumerator {}\n[Guid(\"A95664D2-9614-4F35-A746-DE8DB63617E6\"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\ninterface FdIMMDeviceEnumerator { int EnumAudioEndpoints(int a, int b, out IntPtr c); [PreserveSig] int GetDefaultAudioEndpoint(int dataFlow, int role, out FdIMMDevice dev); }\n[Guid(\"D666063F-1587-4E43-81F1-B948E807363F\"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\ninterface FdIMMDevice { [PreserveSig] int Activate(ref Guid iid, int ctx, IntPtr p, [MarshalAs(UnmanagedType.IUnknown)] out object o); }\n[Guid(\"77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F\"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\ninterface FdIAudioSessionManager2 { int a(); int b(); [PreserveSig] int GetSessionEnumerator(out FdIAudioSessionEnumerator e); }\n[Guid(\"E2F5BB11-0570-40CA-ACDD-3AA01277DEE8\"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\ninterface FdIAudioSessionEnumerator { [PreserveSig] int GetCount(out int n); [PreserveSig] int GetSession(int i, out FdIAudioSessionControl2 s); }\n[Guid(\"bfb7ff88-7239-4fc9-8fa2-07c950be9c6d\"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\ninterface FdIAudioSessionControl2 { int a(); int b(); int c(); int d(); int e(); int f(); int g(); int h(); int i(); int j(); int k(); [PreserveSig] int GetProcessId(out uint pid); }\n[Guid(\"87CE5498-68D6-44E5-9215-6DA47EF883D8\"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]\ninterface FdISimpleAudioVolume { [PreserveSig] int SetMasterVolume(float v, ref Guid ctx); [PreserveSig] int GetMasterVolume(out float v); }\npublic static class FdSpVol {\n  static FdISimpleAudioVolume[] Find() {\n    var list = new System.Collections.Generic.List<FdISimpleAudioVolume>();\n    var en = (FdIMMDeviceEnumerator)(new FdMMDeviceEnumerator());\n    FdIMMDevice dev; if (en.GetDefaultAudioEndpoint(0, 1, out dev) != 0) return list.ToArray();\n    Guid iid = typeof(FdIAudioSessionManager2).GUID; object o; dev.Activate(ref iid, 23, IntPtr.Zero, out o);\n    var mgr = (FdIAudioSessionManager2)o; FdIAudioSessionEnumerator se; mgr.GetSessionEnumerator(out se);\n    int n; se.GetCount(out n);\n    for (int i = 0; i < n; i++) {\n      FdIAudioSessionControl2 c; se.GetSession(i, out c); uint pid; c.GetProcessId(out pid);\n      try { if (Process.GetProcessById((int)pid).ProcessName.Equals(\"Spotify\", StringComparison.OrdinalIgnoreCase)) list.Add((FdISimpleAudioVolume)c); } catch {}\n    }\n    return list.ToArray();\n  }\n  public static string Run(string cmd) {\n    var vs = Find(); if (vs.Length == 0) return \"none\";\n    float v; vs[0].GetMasterVolume(out v);\n    if (cmd == \"up\") v = Math.Min(1f, v + 0.06f); else if (cmd == \"down\") v = Math.Max(0f, v - 0.06f);\n    else if (cmd.StartsWith(\"set \")) v = Math.Max(0f, Math.Min(1f, float.Parse(cmd.Substring(4), System.Globalization.CultureInfo.InvariantCulture)));\n    if (cmd != \"get\") { Guid g = Guid.Empty; foreach (var x in vs) x.SetMasterVolume(v, ref g); }\n    return v.ToString(\"0.000\", System.Globalization.CultureInfo.InvariantCulture);\n  }\n}\n\"@\n} catch { [Console]::Out.WriteLine(\"FAIL \" + $_.Exception.Message); exit 1 }\n[Console]::Out.WriteLine(\"READY\")\nwhile ($true) {\n  $l = [Console]::In.ReadLine(); if ($l -eq $null) { break }\n  try { $r = [FdSpVol]::Run($l.Trim()) } catch { $r = \"ERR\" }\n  [Console]::Out.WriteLine($r)\n}\n";
const spv = {proc:null, ready:false, failed:false, q:[], buf:"", vol:null, lastGet:0};
function spvStart(){
  if (process.platform !== "win32" || spv.proc || spv.failed || !S.spotify_lautstaerke) return;
  let p;
  try { p = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", Buffer.from(SPV_PS, "utf16le").toString("base64")], {windowsHide:true}); }
  catch (e) { spv.failed = true; return; }
  spv.proc = p; spv.buf = "";
  p.stdout.on("data", d => { spv.buf += d; let i; while ((i = spv.buf.indexOf("\n")) >= 0) { const line = spv.buf.slice(0, i).trim(); spv.buf = spv.buf.slice(i + 1); spvLine(line); } });
  p.on("error", () => { spv.failed = true; });
  p.on("exit", () => { spv.proc = null; spv.ready = false; const q = spv.q; spv.q = []; q.forEach(cb => cb(null)); });
  setTimeout(() => { if (spv.proc === p && !spv.ready) { spv.failed = true; try { p.kill(); } catch (e) {} } }, 25000);
}
function spvLine(l){
  if (!l) return;
  if (l === "READY") { spv.ready = true; spvCmd("get"); return; }
  if (l.startsWith("FAIL")) { spv.failed = true; console.log("  Hinweis: Spotify-Lautstärke nicht verfügbar – die Knöpfe regeln die PC-Lautstärke."); return; }
  const cb = spv.q.shift(), v = parseFloat(l);
  if (!isNaN(v)) spv.vol = v; else if (l === "none") spv.vol = null;
  if (cb) cb(l);
}
function spvCmd(c, cb){
  if (!spv.ready || !spv.proc) { if (cb) cb(null); return; }
  spv.q.push(cb || null); try { spv.proc.stdin.write(c + "\n"); } catch (e) {}
}
setInterval(() => { if (spv.ready && Date.now() - mediaWanted < 10000) spvCmd("get"); }, 3000).unref();
function mediaCmd(c){
  if ((c === "volup" || c === "voldown") && S.spotify_lautstaerke && process.platform === "win32" && !spv.failed) {
    if (spv.ready) { spvCmd(c === "volup" ? "up" : "down"); return; }
    spvStart(); return;
  }
  if (c === "open") { spawn("cmd", ["/c", "start", "", "spotify:"], {windowsHide:true, detached:true}); return; }
  const k = KEYS[c]; if (!k) return;
  const n = c.startsWith("vol") ? 2 : 1;
  ps("$w = New-Object -ComObject WScript.Shell; 1.." + n + " | ForEach-Object { $w.SendKeys([char]" + k + ") }", () => setTimeout(pollMedia, 300));
}
function mediaState(){
  if (Date.now() - mediaWanted > 10000) { mediaWanted = Date.now(); pollMedia(); }
  mediaWanted = Date.now(); spvStart();
  return Object.assign({vol:spv.ready ? spv.vol : null, volMode:spv.ready ? "spotify" : spv.failed || !S.spotify_lautstaerke ? "system" : "start"}, media);
}
module.exports = {mediaState, mediaCmd};
