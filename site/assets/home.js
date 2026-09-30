/* Keel site, landing page only: the live demo. Progressive enhancement: with JS off the demo links to the playground. */
(function () {
  "use strict";
  var doc = document;
  var $ = function (sel, ctx) { return (ctx || doc).querySelector(sel); };
  var $$ = function (sel, ctx) { return Array.prototype.slice.call((ctx || doc).querySelectorAll(sel)); };
  var canObserve = "IntersectionObserver" in window;

  /* ---------- the live demo: the real playground, mounted when scrolled into view ---------- */
  var live = $("[data-live]");
  if (live) {
    var mount = $("[data-live-mount]", live), note = $("[data-live-note]", live), src = mount && mount.getAttribute("data-src");
    var stat = { rate: $('[data-stat="rate"]', live), p50: $('[data-stat="p50"]', live), p99: $('[data-stat="p99"]', live) };
    var pushBtn = $("[data-push-it]", live), frame = null, gotStats = false, giveUp = 0, step = 0;
    var theme = function () { return doc.documentElement.dataset.theme === "light" ? "light" : "dark"; };
    var fmt = function (us) {
      if (!isFinite(us) || us < 0) return "–";
      if (step > 0 ? us <= step : us < 1) return "< " + (step >= 1000 ? (step / 1000) + " ms" : (step || 1) + " µs"); // below the clock's resolution
      return us >= 1000 ? (us / 1000).toFixed(us >= 10000 ? 1 : 2) + " ms" : Math.round(us) + " µs";
    };
    var url = function (screen) { return src.replace(/screen=[^&]*/, "screen=" + screen) + "&theme=" + theme(); };
    var open = function (screen) {
      if (frame) frame.remove();
      gotStats = false; clearTimeout(giveUp);
      frame = doc.createElement("iframe");
      frame.loading = "lazy"; frame.src = url(screen);
      frame.title = "Keel playground: the real Rust core, running in this page as WebAssembly";
      mount.appendChild(frame);
      if (note) note.hidden = true;
      giveUp = setTimeout(function () {
        if (gotStats) return;
        if (note) { note.textContent = "The live counters did not start. Your browser may block WebAssembly or embedded frames; "; var a = doc.createElement("a"); a.href = "playground/?screen=list&stream=1"; a.textContent = "open the playground in its own tab"; note.appendChild(a); note.appendChild(doc.createTextNode(".")); note.hidden = false; }
      }, 5000);
    };
    window.addEventListener("message", function (e) {
      var d = e.data;
      // "undra-stats" is the name; the pre-rename spelling is accepted too so a half-merged tree keeps working.
      if (!frame || e.source !== frame.contentWindow || e.origin !== location.origin || !d || (d.type !== "undra-stats" && d.type !== "keel-stats")) return;
      var r = Number(d.changeSetsPerSec), a = Number(d.applyP50Us), b = Number(d.applyP99Us);
      if (!isFinite(r) || !isFinite(a) || !isFinite(b)) return;
      if (!gotStats) { gotStats = true; clearTimeout(giveUp); if (note) note.hidden = true; }
      if (isFinite(Number(d.timerResolutionUs))) step = Number(d.timerResolutionUs);
      stat.rate.textContent = r.toFixed(1); stat.p50.textContent = fmt(a); stat.p99.textContent = fmt(b);
    });
    doc.addEventListener("keel:theme", function (e) {
      if (!frame || !frame.contentWindow) return;
      ["undra-theme", "keel-theme"].forEach(function (type) { frame.contentWindow.postMessage({ type: type, theme: e.detail }, location.origin); });
    });
    if (pushBtn) pushBtn.addEventListener("click", function () { open("stress"); pushBtn.disabled = true; });
    if (src && canObserve) {
      var lo = new IntersectionObserver(function (es) {
        if (es[0].isIntersecting) { lo.disconnect(); open("list"); }
      }, { rootMargin: "200px 0px" });
      lo.observe(live);
    }
  }
})();
