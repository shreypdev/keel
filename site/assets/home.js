/* Undra site, landing page only: the live demo. Progressive enhancement: with JS off the demo links to the playground. */
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
    var stat = { rate: $('[data-stat="rate"]', live), p50: $('[data-stat="p50"]', live), p99: $('[data-stat="p99"]', live), gen: $('[data-stat="gen"]', live), merge: $('[data-stat="merge"]', live), dropped: $('[data-stat="dropped"]', live) };
    var stressCells = $$("[data-stress]", live);
    // Browsers clamp performance.now() to a step of at least 100 µs unless the page is cross-origin isolated (Chrome:
    // 5 µs then), and GitHub Pages cannot send the isolating headers. The playground measures its own step, but a
    // frozen or unmeasurable clock reports 0, so the floor below is what is known and the counters never claim finer.
    var floorUs = window.crossOriginIsolated ? 5 : 100;
    var pushBtn = $("[data-push-it]", live), bar = $(".frame-bar span", live), frame = null, gotStats = false, giveUp = 0, step = floorUs;
    var theme = function () { return doc.documentElement.dataset.theme === "light" ? "light" : "dark"; };
    var fmt = function (us) {
      if (!isFinite(us) || us < 0) return "–";
      if (step > 0 ? us <= step : us < 1) return "< " + (step >= 100 ? (step / 1000) + " ms" : (step || 1) + " µs"); // below the clock's resolution, written as the playground writes it (0.1 ms)
      return us >= 1000 ? (us / 1000).toFixed(us >= 10000 ? 1 : 2) + " ms" : Math.round(us) + " µs";
    };
    var count = function (n) { return isFinite(n) ? Math.round(n).toLocaleString("en-US") : "–"; };
    var listUrl = function () { return src + "&theme=" + theme(); };
    // "Push it": the playground's stress screen. The core generates 10,000 updates a second on its own timer.
    var stressUrl = function () { return src.split("?")[0] + "?screen=stress&embed=1&rate=10000&mode=firehose&autostart=1&theme=" + theme(); };
    var open = function (url, label) {
      if (frame) frame.remove();
      gotStats = false; clearTimeout(giveUp);
      stressCells.forEach(function (el) { el.hidden = true; });
      [stat.rate, stat.p50, stat.p99, stat.gen, stat.dropped].forEach(function (el) { el.textContent = "–"; });
      stat.merge.textContent = "";
      if (bar) bar.textContent = label;
      frame = doc.createElement("iframe");
      frame.loading = "lazy"; frame.src = url;
      frame.title = "Undra playground: the real Rust core, running in this page as WebAssembly";
      mount.appendChild(frame);
      if (note) note.hidden = true;
      giveUp = setTimeout(function () {
        if (gotStats) return;
        if (note) { note.textContent = "The live counters did not start. Your browser may block WebAssembly or embedded frames; "; var a = doc.createElement("a"); a.href = "playground/?screen=list&stream=1"; a.textContent = "open the playground in its own tab"; note.appendChild(a); note.appendChild(doc.createTextNode(".")); note.hidden = false; }
      }, 5000);
    };
    window.addEventListener("message", function (e) {
      var d = e.data;
      if (!frame || e.source !== frame.contentWindow || e.origin !== location.origin || !d || d.type !== "undra-stats") return;
      var r = Number(d.changeSetsPerSec), a = Number(d.applyP50Us), b = Number(d.applyP99Us);
      if (!isFinite(r) || !isFinite(a) || !isFinite(b)) return;
      if (!gotStats) { gotStats = true; clearTimeout(giveUp); if (note) note.hidden = true; }
      if (isFinite(Number(d.timerResolutionUs))) step = Math.max(floorUs, Number(d.timerResolutionUs));
      stat.p50.textContent = fmt(a); stat.p99.textContent = fmt(b);
      // The stress screen adds the core's own counter and what the mirror did with it (all measured in this page).
      var g = Number(d.generatedPerSec), applied = Number(d.entriesAppliedPerSec), received = Number(d.entriesReceivedPerSec), ratio = Number(d.mergeRatio), dropped = Number(d.droppedFrames);
      var stress = d.generatedPerSec !== undefined && isFinite(g) && isFinite(applied);
      stressCells.forEach(function (el) { el.hidden = !stress; });
      if (stress) {
        stat.gen.textContent = count(g);
        stat.rate.textContent = count(applied);
        // Say what happened to the rest: 10,000 received and 120 applied is the merge, not a loss.
        stat.merge.textContent = isFinite(ratio) && isFinite(received) && received > 0 ? count(received) + " received, merged into " + (ratio * 100).toFixed(ratio < 0.01 ? 2 : 1) + " %" : "";
        stat.dropped.textContent = isFinite(dropped) ? count(dropped) : "–";
      } else {
        stat.rate.textContent = r.toFixed(1);
        stat.merge.textContent = "";
      }
    });
    doc.addEventListener("undra:theme", function (e) {
      if (!frame || !frame.contentWindow) return;
      frame.contentWindow.postMessage({ type: "undra-theme", theme: e.detail }, location.origin);
    });
    if (pushBtn) pushBtn.addEventListener("click", function () { open(stressUrl(), "playground · stress"); pushBtn.disabled = true; });
    if (src && canObserve) {
      var lo = new IntersectionObserver(function (es) {
        if (es[0].isIntersecting) { lo.disconnect(); if (!frame) open(listUrl(), "playground · 10k list"); }
      }, { rootMargin: "200px 0px" });
      lo.observe(live);
    }
  }
})();
