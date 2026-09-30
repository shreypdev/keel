/* Keel site: Cmd/Ctrl-K search over search-index.json (built by site/scripts/build-search-index.mjs).
   Loaded on first use by site.js. No dependencies. Entries: { u: url, p: page title, h: heading, x: text }. */
(function () {
  "use strict";
  var doc = document, root = null, input, list, status, index = null, loading = null, base = "", shown = [], sel = 0, returnTo = null;

  function esc(s) { return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
  function words(q) { return q.toLowerCase().split(/[^a-z0-9_#+.:-]+/).filter(Boolean); }

  function build() {
    root = doc.createElement("div");
    root.className = "kbar"; root.hidden = true;
    root.innerHTML = '<div class="kbar-panel" role="dialog" aria-modal="true" aria-label="Search Keel">' +
      '<div class="kbar-in"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3 3"/></svg>' +
      '<input type="search" role="combobox" aria-expanded="true" aria-controls="kbar-list" aria-autocomplete="list" placeholder="Search the docs, the roadmap and the blog" autocomplete="off" autocapitalize="off" spellcheck="false" aria-label="Search">' +
      '<kbd>Esc</kbd></div><ul id="kbar-list" role="listbox" aria-label="Results"></ul><p class="kbar-status" role="status" aria-live="polite"></p></div>';
    doc.body.appendChild(root);
    input = root.querySelector("input"); list = root.querySelector("ul"); status = root.querySelector(".kbar-status");
    root.addEventListener("mousedown", function (e) { if (e.target === root) close(); });
    input.addEventListener("input", render);
    input.addEventListener("keydown", function (e) {
      if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
      else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
      else if (e.key === "Enter") { e.preventDefault(); go(sel); }
      else if (e.key === "Escape") { e.preventDefault(); close(); }
      else if (e.key === "Tab") { e.preventDefault(); }
    });
    list.addEventListener("mousemove", function (e) { var li = e.target.closest("li"); if (li) { mark(+li.getAttribute("data-i")); } });
    list.addEventListener("click", function (e) { var li = e.target.closest("li"); if (li) go(+li.getAttribute("data-i")); });
  }

  function load() {
    if (index) return Promise.resolve();
    if (!loading) loading = fetch(base + "search-index.json").then(function (r) { if (!r.ok) throw new Error(r.status); return r.json(); }).then(function (j) { index = j; }, function () { loading = null; index = null; throw 0; });
    return loading;
  }

  function score(e, ws) {
    var head = e.h.toLowerCase(), page = e.p.toLowerCase(), text = e.x.toLowerCase(), s = 0;
    for (var i = 0; i < ws.length; i++) {
      var w = ws[i], h = head.indexOf(w), t = text.indexOf(w), p = page.indexOf(w);
      if (h < 0 && t < 0 && p < 0) return 0;
      if (h >= 0) s += (h === 0 ? 14 : 10);
      if (p >= 0) s += 5;
      if (t >= 0) s += 2 + (text.indexOf(w, t + 1) >= 0 ? 1 : 0);
    }
    if (!e.h) s += 2;
    return s;
  }

  function snippet(e, ws) {
    var text = e.x, low = text.toLowerCase(), at = -1;
    for (var i = 0; i < ws.length && at < 0; i++) at = low.indexOf(ws[i]);
    var from = Math.max(0, at < 0 ? 0 : at - 40), out = text.slice(from, from + 130);
    out = esc(out);
    ws.forEach(function (w) { out = out.replace(new RegExp("(" + w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + ")", "ig"), "<mark>$1</mark>"); });
    return (from > 0 ? "… " : "") + out + (from + 130 < text.length ? " …" : "");
  }

  function render() {
    var q = input.value.trim(), ws = words(q);
    if (!index) { list.innerHTML = ""; status.textContent = "Loading the index…"; return; }
    var rows;
    if (!ws.length) {
      rows = index.filter(function (e) { return !e.h; }).slice(0, 8).map(function (e) { return { e: e, sn: e.x.slice(0, 120) + "…" }; });
      status.textContent = rows.length + " pages. Type to search.";
    } else {
      rows = index.map(function (e) { return { e: e, s: score(e, ws) }; }).filter(function (r) { return r.s > 0; })
        .sort(function (a, b) { return b.s - a.s; }).slice(0, 8).map(function (r) { return { e: r.e, sn: snippet(r.e, ws) }; });
      status.textContent = rows.length ? rows.length + " results" : 'Nothing found for "' + q + '".';
    }
    shown = rows; sel = 0;
    list.innerHTML = rows.map(function (r, i) {
      var e = r.e;
      return '<li role="option" id="kbar-' + i + '" data-i="' + i + '" aria-selected="' + (i === 0) + '"><span class="kbar-t">' + esc(e.p) + (e.h ? ' <i aria-hidden="true">›</i> ' + esc(e.h) : "") + '</span><span class="kbar-s">' + r.sn + "</span></li>";
    }).join("");
    input.setAttribute("aria-activedescendant", rows.length ? "kbar-0" : "");
  }

  function mark(i) {
    var items = list.children; if (!items[i]) return;
    if (items[sel]) items[sel].setAttribute("aria-selected", "false");
    sel = i; items[i].setAttribute("aria-selected", "true"); input.setAttribute("aria-activedescendant", "kbar-" + i);
    items[i].scrollIntoView({ block: "nearest" });
  }
  function move(d) { if (shown.length) mark((sel + d + shown.length) % shown.length); }
  function go(i) { var r = shown[i]; if (!r) return; close(); location.href = new URL(r.e.u, new URL(base, location.href)).href; }

  function open(siteBase) {
    base = new URL(siteBase, location.href).href;
    if (!root) build();
    returnTo = doc.activeElement;
    root.hidden = false; doc.documentElement.style.overflow = "hidden";
    input.value = ""; input.focus(); render();
    load().then(render, function () { status.textContent = "Search is unavailable right now."; });
  }
  function close() {
    if (!root || root.hidden) return;
    root.hidden = true; doc.documentElement.style.overflow = "";
    if (returnTo && returnTo.focus) returnTo.focus();
  }
  function toggle(siteBase) { if (root && !root.hidden) close(); else open(siteBase); }

  window.KeelSearch = { open: open, close: close, toggle: toggle };
})();
