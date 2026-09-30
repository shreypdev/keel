/* Keel site — behaviour shared by every page: theme toggle, tabs, copy buttons,
   a small syntax highlighter (no dependencies), and scroll reveals.
   Everything here is progressive enhancement: with JS off the content is readable, dark, and complete. */
(function () {
  "use strict";
  var doc = document;
  var root = doc.documentElement;
  var reduce = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
  var $$ = function (sel, ctx) { return Array.prototype.slice.call((ctx || doc).querySelectorAll(sel)); };

  /* ---------- theme ---------- */
  function setTheme(t, persist) {
    root.dataset.theme = t;
    var meta = doc.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute("content", t === "light" ? "#f7f6f2" : "#0a0a0a");
    if (persist) { try { localStorage.setItem("keel-theme", t); } catch (e) { /* private mode */ } }
    $$("[data-theme-toggle]").forEach(function (b) {
      b.setAttribute("aria-label", t === "light" ? "Switch to dark theme" : "Switch to light theme");
    });
  }
  setTheme(root.dataset.theme === "light" ? "light" : "dark", false);
  $$("[data-theme-toggle]").forEach(function (b) {
    b.addEventListener("click", function () { setTheme(root.dataset.theme === "light" ? "dark" : "light", true); });
  });

  /* ---------- syntax highlighting ---------- */
  var KW = {
    rust: "as async await break const continue crate dyn else enum fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait type unsafe use where while true false",
    swift: "actor as async await break case catch class continue default defer do else enum extension false for func guard if import in init internal is let nil private public return self static struct switch throw throws try true typealias var where while some any final override protocol",
    kotlin: "as break by class companion continue data do else enum false for fun if import in interface is null object override package private return sealed super suspend this throw true try typealias val var when while open abstract",
    ts: "as async await break case catch class const continue default delete do else enum export extends false finally for from function if implements import in instanceof interface let new null of private protected public readonly return static super switch this throw true try type typeof undefined var void while yield declare abstract",
    bash: "if then else fi for do done case esac in export",
    toml: "true false",
    c: "void const struct typedef return static enum extern uint8_t uint16_t uint32_t uint64_t int char"
  };
  KW.tsx = KW.ts; KW.typescript = KW.ts; KW.kt = KW.kotlin; KW.sh = KW.bash; KW.shell = KW.bash; KW.json = "true false null";
  var RX = {
    rust: [["com", "\\/\\/[^\\n]*|\\/\\*[\\s\\S]*?\\*\\/"], ["str", "\"(?:\\\\.|[^\"\\\\])*\"|'(?:\\\\.|[^'\\\\\\n])'"], ["attr", "#!?\\[[^\\]\\n]*\\]"]],
    swift: [["com", "\\/\\/[^\\n]*|\\/\\*[\\s\\S]*?\\*\\/"], ["str", "\"(?:\\\\.|[^\"\\\\\\n])*\""], ["attr", "@\\w+"]],
    kotlin: [["com", "\\/\\/[^\\n]*|\\/\\*[\\s\\S]*?\\*\\/"], ["str", "\"(?:\\\\.|[^\"\\\\\\n])*\"|'(?:\\\\.|[^'\\\\\\n])'"], ["attr", "@\\w+"]],
    ts: [["com", "\\/\\/[^\\n]*|\\/\\*[\\s\\S]*?\\*\\/"], ["str", "\"(?:\\\\.|[^\"\\\\\\n])*\"|'(?:\\\\.|[^'\\\\\\n])*'|`(?:\\\\.|[^`\\\\])*`"], ["attr", "@\\w+"]],
    bash: [["com", "(?:^|\\s)#[^\\n]*"], ["str", "\"(?:\\\\.|[^\"\\\\])*\"|'[^'\\n]*'"], ["attr", "\\s--?[A-Za-z][\\w-]*"]],
    toml: [["com", "#[^\\n]*"], ["str", "\"(?:\\\\.|[^\"\\\\\\n])*\""], ["ty", "^\\[[^\\]\\n]+\\]"]],
    json: [["str", "\"(?:\\\\.|[^\"\\\\\\n])*\""]],
    c: [["com", "\\/\\/[^\\n]*|\\/\\*[\\s\\S]*?\\*\\/"], ["str", "\"(?:\\\\.|[^\"\\\\\\n])*\""]]
  };
  RX.tsx = RX.ts; RX.typescript = RX.ts; RX.kt = RX.kotlin; RX.sh = RX.bash; RX.shell = RX.bash;
  var BASH_CMDS = "keel cargo npm git cd rustup brew source curl swift bash gradlew xcodebuild";

  function esc(s) { return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
  function highlight(code) {
    var lang = code.getAttribute("data-lang");
    if (!lang || !RX[lang] || code.getAttribute("data-hl")) return;
    var src = code.textContent, parts = [], names = [];
    RX[lang].forEach(function (r) { names.push(r[0]); parts.push("(" + r[1] + ")"); });
    var isShell = lang === "bash" || lang === "sh" || lang === "shell";
    if (isShell) { names.push("fn"); parts.push("(\\b(?:" + BASH_CMDS.split(" ").join("|") + ")\\b)"); }
    names.push("num"); parts.push("(\\b0x[0-9a-fA-F_]+\\b|\\b\\d[\\d_]*(?:\\.\\d+)?(?:[eE][+-]?\\d+)?[A-Za-z]?\\w*\\b)");
    if (KW[lang]) { names.push("kw"); parts.push("(\\b(?:" + KW[lang].split(" ").join("|") + ")\\b)"); }
    if (!isShell && lang !== "toml" && lang !== "json") {
      names.push("ty"); parts.push("(\\b[A-Z][A-Za-z0-9_]*\\b)");
      names.push("fn"); parts.push("(\\b[a-z_][A-Za-z0-9_]*!?(?=\\())");
    }
    var re = new RegExp(parts.join("|"), "gm"), out = "", last = 0, m;
    while ((m = re.exec(src))) {
      if (m[0] === "") { re.lastIndex++; continue; }
      var i = 1; while (i < m.length && m[i] === undefined) i++;
      out += esc(src.slice(last, m.index)) + '<span class="tok-' + names[i - 1] + '">' + esc(m[0]) + "</span>";
      last = m.index + m[0].length;
    }
    code.innerHTML = out + esc(src.slice(last));
    code.setAttribute("data-hl", "1");
  }
  $$("pre code[data-lang]").forEach(highlight);

  /* ---------- copy buttons ---------- */
  var COPY_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="5.5" y="5.5" width="8" height="8" rx="1.8"/><path d="M10.5 3.5v-.2A1.8 1.8 0 0 0 8.7 1.5H3.8A1.8 1.8 0 0 0 2 3.3v4.9a1.8 1.8 0 0 0 1.8 1.8h.2"/></svg>';
  function copyText(text, btn) {
    function done() {
      var label = btn.querySelector("span");
      btn.classList.add("done");
      if (label) label.textContent = "Copied";
      setTimeout(function () { btn.classList.remove("done"); if (label) label.textContent = "Copy"; }, 1600);
    }
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text).then(done, function () { fallback(); });
    } else { fallback(); }
    function fallback() {
      var ta = doc.createElement("textarea");
      ta.value = text; ta.setAttribute("readonly", ""); ta.style.cssText = "position:fixed;opacity:0;top:0";
      doc.body.appendChild(ta); ta.select();
      try { doc.execCommand("copy"); done(); } catch (e) { /* nothing more to try */ }
      doc.body.removeChild(ta);
    }
  }
  function makeCopy(getText) {
    var b = doc.createElement("button");
    b.type = "button"; b.className = "copy"; b.setAttribute("aria-label", "Copy code");
    b.innerHTML = COPY_ICON + "<span>Copy</span>";
    b.addEventListener("click", function () { copyText(getText(), b); });
    return b;
  }
  $$(".code").forEach(function (box) {
    var pre = box.querySelector("pre"); if (!pre || box.querySelector(".copy")) return;
    var btn = makeCopy(function () { return pre.textContent.replace(/\n$/, ""); });
    var bar = box.querySelector(".code-bar");
    (bar || box).appendChild(btn);
  });
  $$("[data-copy]").forEach(function (b) {
    b.addEventListener("click", function () { copyText(b.getAttribute("data-copy"), b); });
  });

  /* ---------- tabs ---------- */
  var tabSeq = 0;
  $$("[data-tabs]").forEach(function (box) {
    var panels = $$(":scope > .tab-panel", box); if (!panels.length) return;
    var id = "tabs" + (++tabSeq), list = doc.createElement("div"), tabs = [];
    list.className = "tablist"; list.setAttribute("role", "tablist");
    if (box.getAttribute("data-label")) list.setAttribute("aria-label", box.getAttribute("data-label"));
    panels.forEach(function (p, i) {
      var t = doc.createElement("button");
      t.type = "button"; t.className = "tab"; t.setAttribute("role", "tab");
      t.id = id + "-t" + i; p.id = p.id || id + "-p" + i;
      t.setAttribute("aria-controls", p.id); t.textContent = p.getAttribute("data-label") || "Tab " + (i + 1);
      p.setAttribute("role", "tabpanel"); p.setAttribute("aria-labelledby", t.id);
      list.appendChild(t); tabs.push(t);
      t.addEventListener("click", function () { select(i, false); });
      t.addEventListener("keydown", function (e) {
        var n = -1;
        if (e.key === "ArrowRight") n = (i + 1) % tabs.length;
        else if (e.key === "ArrowLeft") n = (i - 1 + tabs.length) % tabs.length;
        else if (e.key === "Home") n = 0; else if (e.key === "End") n = tabs.length - 1;
        if (n >= 0) { e.preventDefault(); select(n, true); }
      });
    });
    function select(n, focus) {
      tabs.forEach(function (t, i) {
        var on = i === n;
        t.setAttribute("aria-selected", on ? "true" : "false"); t.tabIndex = on ? 0 : -1;
        panels[i].hidden = !on;
      });
      if (focus) tabs[n].focus();
    }
    var lead = box.querySelector(":scope > .tabs-head");
    if (lead) lead.appendChild(list); else box.insertBefore(list, box.firstChild);
    select(0, false);
  });

  /* ---------- reveals, stat bars, count-up ---------- */
  function fmt(v, dec) { return dec ? v.toFixed(dec) : String(Math.round(v)); }
  function countUp(el) {
    var target = parseFloat(el.getAttribute("data-count")), dec = parseInt(el.getAttribute("data-dec") || "0", 10);
    if (reduce || isNaN(target)) { el.textContent = el.getAttribute("data-final") || fmt(target, dec); return; }
    var start = performance.now(), dur = 900;
    (function tick(now) {
      var k = Math.min(1, (now - start) / dur), e = 1 - Math.pow(1 - k, 3);
      el.textContent = k < 1 ? fmt(target * e, dec) : (el.getAttribute("data-final") || fmt(target, dec));
      if (k < 1) requestAnimationFrame(tick);
    })(start);
  }
  var targets = $$(".reveal, [data-bar], [data-count]");
  if ("IntersectionObserver" in window && targets.length) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) {
        if (!en.isIntersecting) return;
        var el = en.target; io.unobserve(el);
        el.classList.add("in");
        if (el.hasAttribute("data-count")) countUp(el);
        $$("[data-count]", el).forEach(countUp);
      });
    }, { rootMargin: "0px 0px -8% 0px", threshold: 0.12 });
    targets.forEach(function (el) { io.observe(el); });
  } else {
    targets.forEach(function (el) { el.classList.add("in"); });
  }

  /* ---------- docs: mark the current section in the on-this-page list ---------- */
  var toc = $$(".toc a");
  if (toc.length && "IntersectionObserver" in window) {
    var map = {};
    toc.forEach(function (a) { map[a.getAttribute("href").slice(1)] = a; });
    var so = new IntersectionObserver(function (entries) {
      entries.forEach(function (en) {
        var a = map[en.target.id]; if (!a) return;
        if (en.isIntersecting) { toc.forEach(function (x) { x.removeAttribute("aria-current"); }); a.setAttribute("aria-current", "true"); }
      });
    }, { rootMargin: "-80px 0px -70% 0px" });
    $$("article h2[id], article h3[id]").forEach(function (h) { so.observe(h); });
  }
})();
