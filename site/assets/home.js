/* Keel site, landing page only: the typed terminal and the live demo.
   Both are progressive enhancement. With JS off, or with prefers-reduced-motion, the terminal shows its
   final state and the demo links to the playground. */
(function () {
  "use strict";
  var doc = document;
  var reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
  var $ = function (sel, ctx) { return (ctx || doc).querySelector(sel); };
  var $$ = function (sel, ctx) { return Array.prototype.slice.call((ctx || doc).querySelectorAll(sel)); };
  var canObserve = "IntersectionObserver" in window;

  /* ---------- the terminal: real commands, typed once when it scrolls into view ---------- */
  var term = $("[data-term]");
  if (term) {
    var body = $(".term-body code", term), lines = $$(".ln", term), replay = $("[data-term-replay]", term);
    var timers = [], running = false;
    var later = function (fn, ms) { timers.push(setTimeout(fn, ms)); };
    var showAll = function () {
      timers.forEach(clearTimeout); timers = []; running = false;
      lines.forEach(function (l) { l.classList.remove("p"); if (l.dataset.html !== undefined) l.innerHTML = l.dataset.html; });
    };
    var play = function () {
      showAll();
      if (reduce) return;
      running = true;
      var pre = body.parentNode; pre.style.minHeight = pre.offsetHeight + "px"; // no layout shift while it types
      lines.forEach(function (l) { l.dataset.html = l.innerHTML; l.classList.add("p"); });
      var i = 0;
      (function next() {
        if (!running || i >= lines.length) { running = false; return; }
        var l = lines[i++];
        if (!l.classList.contains("c")) { l.classList.remove("p"); later(next, 70); return; }
        var text = l.textContent, n = 0;
        l.classList.remove("p"); l.textContent = "";
        var typed = doc.createTextNode(""), caret = doc.createElement("span");
        caret.className = "caret"; l.appendChild(typed); l.appendChild(caret);
        (function type() {
          if (!running) return;
          n += 1; typed.nodeValue = text.slice(0, n);
          if (n < text.length) { later(type, 32 + Math.random() * 28); return; }
          later(function () { l.innerHTML = l.dataset.html; later(next, 380); }, 260);
        })();
      })();
    };
    if (replay) { replay.hidden = false; replay.addEventListener("click", play); }
    if (canObserve && !reduce) {
      var to = new IntersectionObserver(function (es) {
        if (es[0].isIntersecting) { to.disconnect(); play(); }
      }, { threshold: 0.35 });
      to.observe(term);
    }
  }

  /* ---------- the live demo: the real playground, mounted when scrolled into view ---------- */
  var live = $("[data-live]");
  if (live) {
    var mount = $("[data-live-mount]", live), note = $("[data-live-note]", live), src = mount && mount.getAttribute("data-src");
    var stat = { rate: $('[data-stat="rate"]', live), p50: $('[data-stat="p50"]', live), p99: $('[data-stat="p99"]', live) };
    var pushBtn = $("[data-push-it]", live), frame = null, gotStats = false, giveUp = 0, step = 0;
    var theme = function () { return doc.documentElement.dataset.theme === "light" ? "light" : "dark"; };
    var fmt = function (us) {
      if (!isFinite(us) || us < 0) return "–";
      if (step > 0 && us <= step) return "< " + (step >= 1000 ? (step / 1000) + " ms" : step + " µs");
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
      if (!frame || e.source !== frame.contentWindow || e.origin !== location.origin || !d || d.type !== "keel-stats") return;
      var r = Number(d.changeSetsPerSec), a = Number(d.applyP50Us), b = Number(d.applyP99Us);
      if (!isFinite(r) || !isFinite(a) || !isFinite(b)) return;
      if (!gotStats) { gotStats = true; clearTimeout(giveUp); if (note) note.hidden = true; }
      if (isFinite(Number(d.timerResolutionUs))) step = Number(d.timerResolutionUs);
      stat.rate.textContent = r.toFixed(1); stat.p50.textContent = fmt(a); stat.p99.textContent = fmt(b);
    });
    doc.addEventListener("keel:theme", function (e) {
      if (frame && frame.contentWindow) frame.contentWindow.postMessage({ type: "keel-theme", theme: e.detail }, location.origin);
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
