/* ===========================================================================
   doc-charts.js  —  the chart runtime for doc-theme.css

   Extracted verbatim from
   template/example.html.

   No Chart.js, no D3, no CDN. Every chart is built from divs or inline SVG
   against the --series-* / --track-* tokens in doc-theme.css, so charts
   re-colour with the theme toggle for free and print correctly.

   Exposes window.docCharts = { hbar, stacked, donut, lineChart, ratio,
   meters, bindTip, fmt }. Each takes the id of an empty mount div.

   Mount a chart by giving it an empty div and calling the function:

     <div id="chart-rows"></div>

     docCharts.hbar("chart-rows", {
       data: [ { label: "Japan", value: 160, note: "All English" } ],
       max: 160, unit: "rows", ticks: [0, 80, 160]
     });

   Signatures, all from real calls in the source document:

     hbar(mountId, { data:[{label,value,note?,color?}], max?, unit?, ticks? })
     stacked(stackId, legendId, [{label,value,color}])
     donut(mountId, { data:[{label,value,color,display?,note?}],
                      valueLabel?, centerValue, centerLabel, aria })
     lineChart(mountId, { data:[{x,y}], max, yTicks, xTicks:[{i,label,anchor?}],
                          unit, aria })
     ratio(mountId, { data:[{label,value,display?,note?}] })
     meters(mountId, [{label,value,max,color,track}])

   label and note accept HTML, so <code> works inside them.
   =========================================================================== */

(function () {
  "use strict";


  /* ---------------- theme toggle ---------------- */
  var btn = document.getElementById("themeBtn");
  var root = document.documentElement;
  function prefersDark() {
    return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  }
  function currentIsDark() {
    var stamp = root.getAttribute("data-theme");
    if (stamp === "dark") return true;
    if (stamp === "light") return false;
    return prefersDark();
  }
  function syncBtn() { if (btn) btn.textContent = currentIsDark() ? "Light" : "Dark"; }
  // A doc without a theme button still renders; only the toggle goes away.
  if (btn) {
    btn.addEventListener("click", function () {
      root.setAttribute("data-theme", currentIsDark() ? "light" : "dark");
      syncBtn();
    });
    syncBtn();
  }

  /* ---------------- tooltip ---------------- */
  // Created on demand so a doc does not have to remember the #tip div.
  var tip = document.getElementById("tip");
  if (!tip) {
    tip = document.createElement("div");
    tip.id = "tip";
    tip.setAttribute("role", "status");
    tip.setAttribute("aria-live", "polite");
    document.body.appendChild(tip);
  }
  function showTip(e, html) {
    tip.innerHTML = html;
    tip.style.opacity = "1";
    moveTip(e);
  }
  function moveTip(e) {
    var pad = 14;
    var x = e.clientX + pad, y = e.clientY + pad;
    var r = tip.getBoundingClientRect();
    if (x + r.width > window.innerWidth - 8) x = e.clientX - r.width - pad;
    if (y + r.height > window.innerHeight - 8) y = e.clientY - r.height - pad;
    tip.style.left = x + "px";
    tip.style.top = y + "px";
  }
  function hideTip() { tip.style.opacity = "0"; }
  function bindTip(el, html) {
    el.addEventListener("mouseenter", function (e) { showTip(e, html); });
    el.addEventListener("mousemove", moveTip);
    el.addEventListener("mouseleave", hideTip);
    el.setAttribute("tabindex", "0");
    el.addEventListener("focus", function () {
      tip.innerHTML = html;
      tip.style.opacity = "1";
      var r = el.getBoundingClientRect();
      tip.style.left = r.left + "px";
      tip.style.top = (r.bottom + 8) + "px";
    });
    el.addEventListener("blur", hideTip);
  }

  var fmt = function (n) { return n.toLocaleString("en-US"); };

  /* ---------------- horizontal bar chart ----------------
     Single series unless a per-row color is given. Bars cap at 22px,
     4px rounded data-end, square at the baseline, value direct-labelled
     at the tip, axis ticks underneath. */
  function hbar(mountId, opts) {
    var mount = document.getElementById(mountId);
    if (!mount) return;
    var data = opts.data;
    var max = opts.max || Math.max.apply(null, data.map(function (d) { return d.value; }));
    var plot = document.createElement("div");
    plot.className = "hbar-plot";

    data.forEach(function (d) {
      var row = document.createElement("div");
      row.className = "hbar-row";

      var lab = document.createElement("div");
      lab.className = "hbar-label";
      lab.innerHTML = d.label;

      var track = document.createElement("div");
      track.className = "hbar-track";

      var fill = document.createElement("div");
      fill.className = "hbar-fill" + (d.value === 0 ? " zero" : "");
      fill.style.background = d.color || "var(--series-1)";
      fill.style.width = "0%";

      var val = document.createElement("span");
      val.className = "hbar-value";
      val.textContent = (opts.valueLabel ? opts.valueLabel(d) : fmt(d.value));

      track.appendChild(fill);
      track.appendChild(val);
      row.appendChild(lab);
      row.appendChild(track);
      plot.appendChild(row);

      bindTip(row, "<strong>" + d.label.replace(/<[^>]+>/g, "") + "</strong><br>" +
        fmt(d.value) + " " + (opts.unit || "") + (d.note ? "<br>" + d.note : ""));

      requestAnimationFrame(function () {
        fill.style.width = (d.value / max * 100) + "%";
      });
    });

    var axis = document.createElement("div");
    axis.className = "hbar-row";
    var spacer = document.createElement("div");
    var ticks = document.createElement("div");
    ticks.className = "hbar-axis";
    var steps = opts.ticks || [0, max / 2, max];
    steps.forEach(function (t) {
      var s = document.createElement("span");
      s.textContent = fmt(Math.round(t));
      ticks.appendChild(s);
    });
    axis.appendChild(spacer);
    axis.appendChild(ticks);
    plot.appendChild(axis);

    mount.appendChild(plot);
  }

  /* ---------------- stacked bar ---------------- */
  function stacked(stackId, legendId, segs) {
    var stack = document.getElementById(stackId);
    var legend = document.getElementById(legendId);
    if (!stack) return;
    var total = segs.reduce(function (a, s) { return a + s.value; }, 0);
    segs.forEach(function (s) {
      var pct = s.value / total * 100;
      var seg = document.createElement("div");
      seg.className = "stack-seg";
      seg.style.background = s.color;
      seg.style.width = pct + "%";
      /* only label inside the segment when it comfortably fits */
      if (pct >= 12) seg.textContent = fmt(s.value);
      bindTip(seg, "<strong>" + s.label + "</strong><br>" + fmt(s.value) +
        " (" + pct.toFixed(1) + "%)");
      stack.appendChild(seg);

      if (legend) {
        var item = document.createElement("span");
        item.className = "legend-item";
        var sw = document.createElement("span");
        sw.className = "legend-swatch";
        sw.style.background = s.color;
        item.appendChild(sw);
        item.appendChild(document.createTextNode(s.label + " · " + fmt(s.value)));
        legend.appendChild(item);
      }
    });
  }

  /* ---------------- donut ----------------
     Ring, not a full pie: the hole carries the total, and hovering a slice or a
     legend row swaps the centre readout and dims everything else. */
  var SVG_NS = "http://www.w3.org/2000/svg";
  function svgEl(name, attrs) {
    var el = document.createElementNS(SVG_NS, name);
    for (var k in attrs) if (attrs.hasOwnProperty(k)) el.setAttribute(k, attrs[k]);
    return el;
  }

  function donut(mountId, opts) {
    var mount = document.getElementById(mountId);
    if (!mount) return;
    var data = opts.data;
    var total = data.reduce(function (a, d) { return a + d.value; }, 0);
    if (!total) return;
    var cx = 90, cy = 90, R = 85, r = 55;
    var lab = function (d) { return opts.valueLabel ? opts.valueLabel(d) : fmt(d.value); };

    function arcPath(a0, a1) {
      /* a full circle cannot be drawn as one arc, so shave a hair off */
      if (a1 - a0 >= Math.PI * 2) a1 = a0 + Math.PI * 2 - 0.0001;
      var pt = function (rad, a) { return [cx + rad * Math.cos(a), cy + rad * Math.sin(a)]; };
      var big = (a1 - a0) > Math.PI ? 1 : 0;
      var o0 = pt(R, a0), o1 = pt(R, a1), i1 = pt(r, a1), i0 = pt(r, a0);
      return "M" + o0[0].toFixed(2) + " " + o0[1].toFixed(2) +
        " A" + R + " " + R + " 0 " + big + " 1 " + o1[0].toFixed(2) + " " + o1[1].toFixed(2) +
        " L" + i1[0].toFixed(2) + " " + i1[1].toFixed(2) +
        " A" + r + " " + r + " 0 " + big + " 0 " + i0[0].toFixed(2) + " " + i0[1].toFixed(2) + " Z";
    }

    var wrap = document.createElement("div");
    wrap.className = "donut-wrap";
    var fig = document.createElement("div");
    fig.className = "donut-figure";
    var svg = svgEl("svg", { viewBox: "0 0 180 180", width: "180", height: "180",
      role: "img", "aria-label": opts.aria || "Donut chart" });

    var center = document.createElement("div");
    center.className = "donut-center";
    var cVal = document.createElement("div");
    cVal.className = "donut-c-val";
    var cLab = document.createElement("div");
    cLab.className = "donut-c-lab";
    var resetCenter = function () {
      cVal.textContent = opts.centerValue || fmt(total);
      cLab.textContent = opts.centerLabel || "total";
    };
    resetCenter();
    center.appendChild(cVal);
    center.appendChild(cLab);

    var legend = document.createElement("div");
    legend.className = "donut-legend";

    var a = -Math.PI / 2;
    data.forEach(function (d) {
      var sweep = d.value / total * Math.PI * 2;
      var path = svgEl("path", { d: arcPath(a, a + sweep), fill: d.color });
      a += sweep;
      svg.appendChild(path);

      var item = document.createElement("div");
      item.className = "dl-item";
      var sw = document.createElement("span");
      sw.className = "dl-sw";
      sw.style.background = d.color;
      var nm = document.createElement("span");
      nm.className = "dl-name";
      nm.innerHTML = d.label;
      var vv = document.createElement("span");
      vv.className = "dl-val";
      vv.textContent = lab(d);
      var pc = document.createElement("span");
      pc.className = "dl-pct";
      pc.textContent = (d.value / total * 100).toFixed(1) + "%";
      item.appendChild(sw); item.appendChild(nm); item.appendChild(vv); item.appendChild(pc);
      legend.appendChild(item);

      var tipHtml = "<strong>" + d.label.replace(/<[^>]+>/g, "") + "</strong><br>" +
        lab(d) + " · " + (d.value / total * 100).toFixed(1) + "% of " +
        (opts.centerLabel || "total") + (d.note ? "<br>" + d.note : "");
      bindTip(path, tipHtml);
      bindTip(item, tipHtml);

      var on = function () {
        wrap.classList.add("dim");
        path.classList.add("on");
        item.classList.add("on");
        cVal.textContent = lab(d);
        cLab.textContent = d.label.replace(/<[^>]+>/g, "");
      };
      var off = function () {
        wrap.classList.remove("dim");
        path.classList.remove("on");
        item.classList.remove("on");
        resetCenter();
      };
      [path, item].forEach(function (el) {
        el.addEventListener("mouseenter", on);
        el.addEventListener("mouseleave", off);
        el.addEventListener("focus", on);
        el.addEventListener("blur", off);
      });
    });

    fig.appendChild(svg);
    fig.appendChild(center);
    wrap.appendChild(fig);
    wrap.appendChild(legend);
    mount.appendChild(wrap);
  }

  /* ---------------- line chart ----------------
     Daily series. Zeros are the point of the chart, so the line is drawn across
     every day and only non-zero days get a marker and a direct label. */
  function lineChart(mountId, opts) {
    var mount = document.getElementById(mountId);
    if (!mount) return;
    var data = opts.data;
    var W = 920, H = 236, mL = 40, mR = 14, mT = 26, mB = 30;
    var pw = W - mL - mR, ph = H - mT - mB;
    var max = opts.max || Math.max.apply(null, data.map(function (d) { return d.y; }));
    var X = function (i) { return mL + (data.length === 1 ? pw / 2 : i / (data.length - 1) * pw); };
    var Y = function (v) { return mT + ph - (v / max) * ph; };

    var wrap = document.createElement("div");
    wrap.className = "line-wrap";
    var svg = svgEl("svg", { viewBox: "0 0 " + W + " " + H, preserveAspectRatio: "xMidYMid meet",
      role: "img", "aria-label": opts.aria || "Line chart" });

    (opts.yTicks || [0, max / 2, max]).forEach(function (t) {
      var y = Y(t);
      svg.appendChild(svgEl("line", { x1: mL, y1: y, x2: W - mR, y2: y,
        "class": t === 0 ? "lc-axis" : "lc-grid" }));
      var tx = svgEl("text", { x: mL - 8, y: y + 3.5, "class": "lc-tick", "text-anchor": "end" });
      tx.textContent = fmt(Math.round(t));
      svg.appendChild(tx);
    });

    var line = "", area = "";
    data.forEach(function (d, i) {
      line += (i ? " L" : "M") + X(i).toFixed(2) + " " + Y(d.y).toFixed(2);
    });
    area = "M" + X(0).toFixed(2) + " " + Y(0).toFixed(2) + " " +
      line.replace(/^M/, "L") + " L" + X(data.length - 1).toFixed(2) + " " + Y(0).toFixed(2) + " Z";
    svg.appendChild(svgEl("path", { d: area, "class": "lc-area" }));
    var lpath = svgEl("path", { d: line, "class": "lc-line" });
    svg.appendChild(lpath);

    /* x ticks: only where the caller asked for one */
    (opts.xTicks || []).forEach(function (t) {
      var tx = svgEl("text", { x: X(t.i), y: H - mB + 17, "class": "lc-tick",
        "text-anchor": t.anchor || "middle" });
      tx.textContent = t.label;
      svg.appendChild(tx);
    });

    /* hover bands across every day, so the zeros are inspectable too */
    var bw = pw / (data.length - 1);
    data.forEach(function (d, i) {
      var hot = svgEl("rect", { x: (X(i) - bw / 2).toFixed(2), y: mT,
        width: bw.toFixed(2), height: ph, "class": "lc-hot" });
      bindTip(hot, "<strong>" + d.x + "</strong><br>" +
        (d.y ? fmt(d.y) + " " + (opts.unit || "rows") + " written" : "no writes"));
      svg.appendChild(hot);
    });

    data.forEach(function (d, i) {
      if (!d.y) return;
      var peak = d.y >= max * 0.5;
      svg.appendChild(svgEl("circle", { cx: X(i).toFixed(2), cy: Y(d.y).toFixed(2), r: 4,
        "class": "lc-dot" + (peak ? " peak" : "") }));
      var t = svgEl("text", { x: X(i).toFixed(2), y: (Y(d.y) - 10).toFixed(2),
        "class": "lc-vlab", "text-anchor": i > data.length - 6 ? "end" : "middle" });
      t.textContent = fmt(d.y);
      svg.appendChild(t);
    });

    wrap.appendChild(svg);
    mount.appendChild(wrap);
  }

  /* ---------------- ratio blocks ----------------
     Two quantities that differ by orders of magnitude. Heights are square-rooted
     so the smaller box stays visible; the multiple is stated in words. */
  function ratio(mountId, opts) {
    var mount = document.getElementById(mountId);
    if (!mount) return;
    var max = Math.max.apply(null, opts.data.map(function (d) { return d.value; }));
    var row = document.createElement("div");
    row.className = "ratio";
    opts.data.forEach(function (d) {
      var item = document.createElement("div");
      item.className = "ratio-item";
      var box = document.createElement("div");
      box.className = "ratio-box";
      box.style.background = d.color || "var(--series-1)";
      box.style.height = "4px";
      var cap = document.createElement("div");
      cap.className = "ratio-cap";
      cap.innerHTML = "<strong>" + d.display + "</strong>" + d.label;
      item.appendChild(box);
      item.appendChild(cap);
      row.appendChild(item);
      bindTip(item, "<strong>" + d.display + "</strong><br>" + d.label.replace(/<[^>]+>/g, "") +
        (d.note ? "<br>" + d.note : ""));
      requestAnimationFrame(function () {
        var h = Math.sqrt(d.value / max) * 150;
        box.style.height = Math.max(6, h) + "px";
      });
    });
    mount.appendChild(row);
  }

  /* ---------------- meters ---------------- */
  function meters(mountId, rows) {
    var mount = document.getElementById(mountId);
    if (!mount) return;
    rows.forEach(function (r) {
      var block = document.createElement("div");
      block.className = "meter-block";
      var head = document.createElement("div");
      head.className = "meter-head";
      var name = document.createElement("span");
      name.className = "meter-name";
      name.innerHTML = r.label;
      var val = document.createElement("span");
      val.className = "meter-val";
      val.textContent = fmt(r.value) + " of " + fmt(r.max);
      head.appendChild(name);
      head.appendChild(val);
      var track = document.createElement("div");
      track.className = "meter-track";
      if (r.track) track.style.background = r.track;
      var fill = document.createElement("div");
      fill.className = "meter-fill";
      fill.style.background = r.color || "var(--series-1)";
      fill.style.width = "0%";
      track.appendChild(fill);
      block.appendChild(head);
      block.appendChild(track);
      mount.appendChild(block);
      bindTip(block, "<strong>" + r.label.replace(/<[^>]+>/g, "") + "</strong><br>" +
        fmt(r.value) + " of " + fmt(r.max) +
        " (" + (r.value / r.max * 100).toFixed(1) + "%)");
      requestAnimationFrame(function () {
        fill.style.width = Math.max(r.value / r.max * 100, r.value > 0 ? 1.5 : 0) + "%";
      });
    });
  }


  /* ---------------- exports ---------------- */
  window.docCharts = {
    hbar: hbar,
    stacked: stacked,
    donut: donut,
    lineChart: lineChart,
    ratio: ratio,
    meters: meters,
    bindTip: bindTip,
    fmt: fmt
  };
})();
