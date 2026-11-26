// ForzaDashboard: YouTube im Dashboard flüssig halten (läuft in der eingebetteten YouTube-Seite vor deren Skripten).
// Neben Forza ist die Grafikkarte voll beschäftigt. 4K/1440p-Videos und AV1 (auf vielen Grafikkarten nur per
// Prozessor dekodierbar) liefen dann oft nur mit ~20 Bildern/s. Deshalb meldet die Seite YouTube:
//   - höchstens 1080p (bzw. 1920 px an der langen Seite – Hochkant-Videos/Shorts bleiben bei 1080×1920)
//   - kein AV1 -> YouTube nimmt VP9 oder H.264, die praktisch jede Grafikkarte selbst dekodiert
// 60 Bilder/s bleiben erlaubt. YouTube wählt die Qualität weiterhin selbst (Auto), nur eben bis 1080p.
(() => {
  if (!/(^|\.)youtube\.com$/.test(location.hostname) || window.__fdYt) return;
  window.__fdYt = 1;
  const KURZ = 1080, LANG = 1920;
  const zahl = (s, k) => { const m = new RegExp("(?:^|[;,\\s])" + k + "\\s*=\\s*\"?(\\d+)", "i").exec(s); return m ? +m[1] : 0; };
  const zuViel = (typ, w, h) => {
    const s = String(typ || "");
    if (/\bav01\b|\bav1\b/i.test(s)) return true;
    w = w || zahl(s, "width"); h = h || zahl(s, "height");
    return !!(w && h && (Math.min(w, h) > KURZ || Math.max(w, h) > LANG));
  };
  for (const name of ["MediaSource", "ManagedMediaSource", "WebKitMediaSource"]) {
    const MS = window[name];
    if (!MS || typeof MS.isTypeSupported !== "function") continue;
    const orig = MS.isTypeSupported.bind(MS);
    try { MS.isTypeSupported = t => zuViel(t) ? false : orig(t); } catch (e) {}
  }
  try {
    const cpt = HTMLMediaElement.prototype.canPlayType;
    HTMLMediaElement.prototype.canPlayType = function (t) { return zuViel(t) ? "" : cpt.call(this, t); };
  } catch (e) {}
  try {
    const mc = navigator.mediaCapabilities;
    if (mc && typeof mc.decodingInfo === "function") {
      const orig = mc.decodingInfo.bind(mc);
      mc.decodingInfo = cfg => {
        const v = cfg && cfg.video;
        if (v && zuViel(v.contentType, v.width, v.height)) return Promise.resolve({supported: false, smooth: false, powerEfficient: false});
        return orig(cfg);
      };
    }
  } catch (e) {}
})();
