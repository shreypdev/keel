/* Keel site — the hero diagram.
   A pure function of time drives the picture: frame(T) sets every pulse, glow and label.
   8-second loop: a write leaves one UI, crosses the boundary once, the core commits one
   change-set, and it fans out to all three local mirrors. The origin rotates each loop.
   Under prefers-reduced-motion the labelled still diagram stays put; the button starts it. */
(function () {
  "use strict";
  const stage = document.querySelector("[data-stage]");
  if (!stage) return;
  const NS = "http://www.w3.org/2000/svg";
  const LOOP = 8;
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const mqNarrow = window.matchMedia("(max-width: 819px)");
  const steps = Array.from(stage.querySelectorAll(".steps li"));
  const toggle = stage.querySelector("[data-dg-toggle]");

  const clamp = (v) => (v < 0 ? 0 : v > 1 ? 1 : v);
  const win = (t, a, b) => clamp((t - a) / (b - a));
  const ease = (x) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);
  const invEase = (y) => { let lo = 0, hi = 1; for (let i = 0; i < 24; i++) { const m = (lo + hi) / 2; if (ease(m) < y) lo = m; else hi = m; } return (lo + hi) / 2; };

  function el(name, attrs, parent) {
    const n = document.createElementNS(NS, name);
    for (const k in attrs) n.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(n);
    return n;
  }

  function buildRig(svg) {
    const q = (s, c) => (c || svg).querySelector(s);
    const k = svg.classList.contains("dg-narrow") ? 0.72 : 1;
    const gp = q("[data-role=pulses]");
    const by = parseFloat(svg.getAttribute("data-by"));
    const rig = { svg, k, nodes: [], chips: {}, cells: [], write: [], change: [], rings: [], commit: null, coreGlow: q("[data-role=core-glow]") };

    for (let i = 0; i < 3; i++) {
      const g = q('[data-node="' + i + '"]');
      rig.nodes.push({
        hot: q("[data-role=hot]", g), halo: q("[data-role=halo]", g), call: q("[data-role=call]", g), idle: q("[data-role=idle]", g),
        badge: q("[data-role=badge]", g), row: q("[data-row=new]", g), hl: q("[data-role=hl]", g),
        badgeBefore: q("[data-role=badge]", g).getAttribute("data-before"), badgeAfter: q("[data-role=badge]", g).getAttribute("data-after")
      });
    }
    ["todos", "filter", "visible", "remaining"].forEach((n) => { const g = q('[data-chip="' + n + '"]'); rig.chips[n] = q("[data-role=on]", g); });
    for (let i = 0; i < 4; i++) { const g = q('[data-cell="' + i + '"]'); rig.cells.push({ on: q("[data-role=on]", g), t: q("[data-role=t]", g) }); }
    const cg = q("[data-role=commit]");
    rig.commit = { on: q("[data-role=on]", cg), t: q("[data-role=t]", cg) };

    function mkPulse(path, bright) {
      const len = path.getTotalLength();
      const g = el("g", { class: "pulse", style: "display:none" }, gp);
      const d = path.getAttribute("d");
      const trails = [1, 0.55, 0.22].map((f, i) => ({ f, p: el("path", { d, "stroke-width": (3.4 - i * 0.5) * k, opacity: [0.2, 0.4, 0.7][i] }, g) }));
      const c1 = el("circle", { class: "g1", r: 12 * k, opacity: 0.18 }, g);
      const c2 = el("circle", { class: "g1", r: 6.5 * k, opacity: 0.4 }, g);
      const c3 = el("circle", { class: "g3", r: (bright ? 3.3 : 2.6) * k }, g);
      return { path, len, g, trails, circles: [c1, c2, c3], trail: 120 * k * (len > 500 ? 1.3 : 1) };
    }
    const beam = (n) => q('[data-beam="' + n + '"]');
    for (let i = 0; i < 3; i++) { rig.write.push(mkPulse(beam("w" + i), false)); rig.change.push(mkPulse(beam("c" + i), true)); }
    rig.write.forEach((p) => {
      const a = p.path.getPointAtLength(0), b = p.path.getPointAtLength(p.len);
      p.uc = clamp((by - a.y) / (b.y - a.y));
      p.tc = 0.7 + 1.5 * invEase(p.uc); // when the pulse crosses the boundary line
      p.ringCross = el("circle", { class: "ring", cx: a.x, cy: by, r: 6 * k }, gp);
      p.ringIn = el("circle", { class: "ring", cx: b.x, cy: b.y, r: 6 * k }, gp);
    });
    rig.change.forEach((p) => {
      const e = p.path.getPointAtLength(p.len);
      p.ringOut = el("circle", { class: "ring", cx: e.x, cy: e.y, r: 6 * k }, gp);
    });
    return rig;
  }

  function setPulse(p, h, d) {
    if (h <= 0 || d >= 1) { if (p.on !== false) { p.g.style.display = "none"; p.on = false; } return; }
    if (p.on !== true) { p.g.style.display = ""; p.on = true; }
    const L = p.len, head = h * L, tail = Math.max(0, Math.min(L, head - p.trail * (1 - d)));
    const pt = p.path.getPointAtLength(head);
    p.trails.forEach((tr) => {
      const a = Math.max(tail, head - p.trail * tr.f), dash = head - a;
      if (dash < 0.6) { tr.p.style.display = "none"; return; }
      tr.p.style.display = "";
      tr.p.setAttribute("stroke-dasharray", dash.toFixed(1) + " " + (L * 2).toFixed(1));
      tr.p.setAttribute("stroke-dashoffset", (-a).toFixed(1));
    });
    const fade = 1 - d;
    p.circles.forEach((c, i) => { c.setAttribute("cx", pt.x.toFixed(1)); c.setAttribute("cy", pt.y.toFixed(1)); c.style.opacity = (i === 2 ? 1 : 1) * fade; });
  }
  function setRing(c, t, t0, k, R) {
    const x = (t - t0) / 0.75;
    if (x < 0 || x > 1) { if (c.on) { c.style.opacity = 0; c.on = false; } return; }
    c.on = true; c.setAttribute("r", (6 + x * R) * k); c.style.opacity = Math.pow(1 - x, 1.6) * 0.95;
  }
  const setOp = (node, v) => { if (node) node.style.opacity = v.toFixed(3); };

  function frame(rig, T) {
    const loop = Math.floor(T / LOOP), t = T - loop * LOOP, origin = loop % 3, k = rig.k;

    // 1. the write
    const wH = ease(win(t, 0.7, 2.2)), wD = win(t, 2.2, 2.6);
    rig.write.forEach((p, i) => {
      if (i !== origin) { setPulse(p, 0, 1); setRing(p.ringCross, t, -9, k, 0); setRing(p.ringIn, t, -9, k, 0); return; }
      setPulse(p, wH, wD);
      setRing(p.ringCross, t, p.tc, k, 20);
      setRing(p.ringIn, t, 2.2, k, 26);
    });

    // 2. the core: signals go dirty, the transaction commits, the change-set is built
    const fadeDirty = 1 - win(t, 5.5, 6.0);
    [["todos", 2.25], ["visible", 2.55], ["remaining", 2.85]].forEach((a) => setOp(rig.chips[a[0]], win(t, a[1], a[1] + 0.2) * fadeDirty));
    setOp(rig.chips.filter, 0);
    const commit = win(t, 3.05, 3.25) * (1 - win(t, 4.0, 4.4));
    setOp(rig.commit.on, commit);
    rig.commit.t.style.opacity = commit.toFixed(3);
    rig.cells.forEach((c, i) => {
      const a = 3.2 + i * 0.1, v = win(t, a, a + 0.15) * (1 - win(t, 4.3, 4.8));
      setOp(c.on, v * 0.9);
      c.t.style.fill = v > 0.5 ? "var(--accent-ink)" : "";
    });
    setOp(rig.coreGlow, 0.3 + 0.7 * Math.max(commit, win(t, 2.2, 2.5) * (1 - win(t, 2.5, 3.2)) * 0.6));

    // 3. the change-set fans out; every mirror applies it locally
    const cH = ease(win(t, 3.6, 5.0)), cD = win(t, 5.0, 5.45);
    rig.change.forEach((p, i) => { setPulse(p, cH, cD); setRing(p.ringOut, t, 5.0, k, 26); });
    const sending = win(t, 0.15, 0.5) * (1 - win(t, 2.3, 2.8));
    const applied = win(t, 5.0, 5.2) * (1 - win(t, 5.2, 6.6));
    rig.nodes.forEach((n, i) => {
      const own = i === origin ? sending : 0;
      setOp(n.hot, Math.max(own, applied * 0.85));
      setOp(n.halo, Math.max(own * 0.55, applied * 0.8));
      const callOp = i === origin ? win(t, 0.3, 0.65) * (1 - win(t, 2.4, 2.8)) : 0;
      setOp(n.call, callOp);
      setOp(n.idle, clamp(1 - callOp * 2.2));
      const rowIn = win(t, 5.0, 5.4), rowOut = win(t, 7.3, 7.7), rv = rowIn * (1 - rowOut);
      setOp(n.row, rv);
      n.row.setAttribute("transform", "translate(0," + ((1 - rowIn) * -8).toFixed(1) + ")");
      setOp(n.hl, 0.3 * win(t, 5.0, 5.15) * (1 - win(t, 5.15, 6.4)));
      const txt = t >= 5.0 && t < 7.5 ? n.badgeAfter : n.badgeBefore;
      if (n.badge.textContent !== txt) n.badge.textContent = txt;
    });

    // steps under the stage
    const on = t >= 0.3 && t < 2.4 ? 0 : t >= 2.4 && t < 4.9 ? 1 : t >= 4.9 && t < 7.4 ? 2 : -1;
    steps.forEach((li, i) => li.classList.toggle("on", i === on));
  }

  // ---- runner ----
  let rig = null, rigSvg = null, T = 0, last = 0, playing = false, raf = 0, inView = true, started = false;
  function activeSvg() { return stage.querySelector(mqNarrow.matches ? ".dg-narrow" : ".dg-wide"); }
  function ensureRig() {
    const svg = activeSvg();
    if (!svg) return null;
    if (svg !== rigSvg) { rigSvg = svg; rig = buildRig(svg); }
    return rig;
  }
  function tick(now) {
    raf = 0;
    if (!playing || !inView) return;
    const dt = Math.min(0.05, (now - last) / 1000); last = now;
    T += dt;
    frame(ensureRig(), T);
    raf = requestAnimationFrame(tick);
  }
  function play() {
    const r = ensureRig(); if (!r) return;
    if (!started) { started = true; stage.querySelectorAll("svg.dg").forEach((s) => s.classList.remove("is-static")); }
    playing = true; last = performance.now();
    if (toggle) { toggle.setAttribute("aria-pressed", "false"); toggle.querySelector("span").textContent = "Pause"; }
    if (!raf) raf = requestAnimationFrame(tick);
  }
  function pause() {
    playing = false;
    if (toggle) { toggle.setAttribute("aria-pressed", "true"); toggle.querySelector("span").textContent = started ? "Play" : "Play animation"; }
  }
  if (toggle) toggle.addEventListener("click", () => (playing ? pause() : play()));
  // Scrub to a moment of the loop (seconds). Used by the pause button's neighbours and for checking frames.
  stage.seek = (sec) => { pause(); if (!started) { started = true; stage.querySelectorAll("svg.dg").forEach((s) => s.classList.remove("is-static")); } T = sec; frame(ensureRig(), T); };
  mqNarrow.addEventListener("change", () => { if (started) { ensureRig(); frame(rig, T); } });
  if ("IntersectionObserver" in window) {
    new IntersectionObserver((es) => {
      inView = es[0].isIntersecting;
      if (inView && playing && !raf) { last = performance.now(); raf = requestAnimationFrame(tick); }
    }, { threshold: 0.05 }).observe(stage);
  }
  if (reduce) pause(); else play();
})();
