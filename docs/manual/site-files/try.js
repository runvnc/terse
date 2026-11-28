// Tersenet in the browser (try.html): loads terse-web.wasm (terse-core
// compiled to WebAssembly, crates/terse-web) and the content bundle
// try-content.bin (the built-in shell and programs and the manual, written
// by `terse manual site`), runs a frame every 1/60 s and draws what the
// machines show on a canvas. Nothing is loaded until Start is pressed.
// No dependencies, no build step.
"use strict";

(() => {
  const FRAME_MS = 1000 / 60;
  const FAMILIES = [
    '"DejaVu Sans Mono", Menlo, Consolas, "Liberation Mono", monospace',
    'system-ui, -apple-system, "Segoe UI", Roboto, "Noto Sans", Arial, sans-serif',
    'Georgia, "Times New Roman", "Noto Serif", serif',
    '"DejaVu Sans Mono", Menlo, Consolas, "Liberation Mono", monospace',
  ];

  let wasm = null, mem = null;
  const bytes = () => new Uint8Array(mem.buffer);
  const enc = new TextEncoder(), dec = new TextDecoder();

  // Copies `data` (bytes) into the module's memory: [ptr, len].
  function put(data) {
    const p = wasm.tw_alloc(data.length);
    bytes().set(data, p);
    return [p, data.length];
  }
  function free([p, n]) { wasm.tw_free(p, n); }

  async function load(status) {
    status("Loading the machine…");
    const env = {
      tw_random(ptr, len) {
        for (let o = 0; o < len; o += 65536) {
          crypto.getRandomValues(new Uint8Array(mem.buffer, ptr + o, Math.min(65536, len - o)));
        }
      },
    };
    const [inst, bundle] = await Promise.all([
      (async () => {
        const r = await fetch("terse-web.wasm");
        if (!r.ok) throw new Error("terse-web.wasm: " + r.status);
        if (WebAssembly.instantiateStreaming && (r.headers.get("content-type") || "").includes("wasm")) {
          return (await WebAssembly.instantiateStreaming(r, { env })).instance;
        }
        return (await WebAssembly.instantiate(await r.arrayBuffer(), { env })).instance;
      })(),
      (async () => {
        const r = await fetch("try-content.bin");
        if (!r.ok) throw new Error("try-content.bin: " + r.status);
        // zlib-compressed by `terse manual site`.
        const s = r.body.pipeThrough(new DecompressionStream("deflate"));
        return new Uint8Array(await new Response(s).arrayBuffer());
      })(),
    ]);
    wasm = inst.exports;
    mem = wasm.memory;
    return bundle;
  }

  // ---- Drawing -----------------------------------------------------------

  const rgba = (b, i) => `rgba(${b[i]},${b[i + 1]},${b[i + 2]},${(b[i + 3] / 255).toFixed(3)})`;

  // Plays drawing commands ops[at..] on ctx (whose transform maps screen
  // units) up to an end command; returns the index after it.
  function play(ctx, ops, at, phosphor) {
    const dv = new DataView(ops.buffer, ops.byteOffset, ops.byteLength);
    const f = (i) => dv.getFloat32(i, true);
    let font = null;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    const stroke = (w, colour) => {
      if (phosphor) {
        ctx.globalCompositeOperation = "lighter";
        ctx.strokeStyle = colour.replace(/[\d.]+\)$/, (a) => (parseFloat(a) * 0.25).toFixed(3) + ")");
        ctx.lineWidth = w * 2 + 0.67;
        ctx.stroke();
        ctx.strokeStyle = colour;
        ctx.lineWidth = w;
        ctx.stroke();
        ctx.globalCompositeOperation = "source-over";
      } else {
        ctx.strokeStyle = colour;
        ctx.lineWidth = w;
        ctx.stroke();
      }
    };
    while (at < ops.length) {
      const op = ops[at++];
      switch (op) {
        case 0x02: return at;
        case 0x0A: {
          // Rectangles on whole pixels, so that cell backgrounds side by side show no seams.
          ctx.fillStyle = rgba(ops, at + 16);
          const m = ctx.getTransform();
          if (m.b === 0 && m.c === 0) {
            const x0 = Math.round(m.a * f(at) + m.e), y0 = Math.round(m.d * f(at + 4) + m.f);
            const x1 = Math.round(m.a * (f(at) + f(at + 8)) + m.e), y1 = Math.round(m.d * (f(at + 4) + f(at + 12)) + m.f);
            ctx.setTransform(1, 0, 0, 1, 0, 0);
            ctx.fillRect(x0, y0, Math.max(1, x1 - x0), Math.max(1, y1 - y0));
            ctx.setTransform(m);
          } else ctx.fillRect(f(at), f(at + 4), f(at + 8), f(at + 12));
          at += 20; break;
        }
        case 0x0B: ctx.beginPath(); ctx.moveTo(f(at), f(at + 4)); ctx.lineTo(f(at + 8), f(at + 12)); stroke(f(at + 16), rgba(ops, at + 20)); at += 24; break;
        case 0x0C: ctx.beginPath(); ctx.arc(f(at), f(at + 4), f(at + 8), 0, 2 * Math.PI); stroke(f(at + 12), rgba(ops, at + 16)); at += 20; break;
        case 0x0D: ctx.beginPath(); ctx.arc(f(at), f(at + 4), f(at + 8), 0, 2 * Math.PI); ctx.fillStyle = rgba(ops, at + 12); ctx.fill(); at += 16; break;
        case 0x0E:
          ctx.beginPath(); ctx.moveTo(f(at), f(at + 4)); ctx.lineTo(f(at + 8), f(at + 12)); ctx.lineTo(f(at + 16), f(at + 20)); ctx.closePath();
          ctx.fillStyle = rgba(ops, at + 24); ctx.fill(); at += 28; break;
        case 0x0F: {
          const x = f(at), y = f(at + 4), w = f(at + 8), size = f(at + 12), fam = ops[at + 16], fl = ops[at + 17];
          const len = ops[at + 22] | (ops[at + 23] << 8);
          const text = dec.decode(ops.subarray(at + 24, at + 24 + len));
          const want = `${fl & 2 ? "italic " : ""}${fl & 1 ? "bold " : ""}${fam === 0 ? size * 1.02 : size}px ${FAMILIES[fam]}`;
          if (want !== font) { ctx.font = want; font = want; }
          ctx.fillStyle = rgba(ops, at + 18);
          if (w > 0 && ctx.measureText(text).width > w) ctx.fillText(text, x, y, w); else ctx.fillText(text, x, y);
          at += 24 + len; break;
        }
        case 0x10: ctx.save(); ctx.beginPath(); ctx.rect(f(at), f(at + 4), f(at + 8), f(at + 12)); ctx.clip(); at += 16; break;
        case 0x11: ctx.restore(); font = null; break;
        case 0x12: {
          const filled = ops[at], fill = rgba(ops, at + 1), stroked = ops[at + 5], sc = rgba(ops, at + 6), w = f(at + 10);
          const n = ops[at + 14] | (ops[at + 15] << 8);
          at += 16;
          const p = new Path2D();
          for (let k = 0; k < n; k++) {
            const kind = ops[at++];
            if (kind === 0) { p.moveTo(f(at), f(at + 4)); at += 8; }
            else if (kind === 1) { p.lineTo(f(at), f(at + 4)); at += 8; }
            else if (kind === 2) { p.quadraticCurveTo(f(at), f(at + 4), f(at + 8), f(at + 12)); at += 16; }
            else if (kind === 3) { p.bezierCurveTo(f(at), f(at + 4), f(at + 8), f(at + 12), f(at + 16), f(at + 20)); at += 24; }
            else p.closePath();
          }
          if (filled) { ctx.fillStyle = fill; ctx.fill(p, "nonzero"); }
          if (stroked) { ctx.strokeStyle = sc; ctx.lineWidth = w; ctx.stroke(p); }
          break;
        }
        default: throw new Error("terse-web: unknown drawing command " + op);
      }
    }
    return at;
  }

  // Skips drawing commands from `at` to the end command; returns the index after it.
  function skip(ops, at) {
    while (at < ops.length) {
      const op = ops[at++];
      switch (op) {
        case 0x02: return at;
        case 0x0A: at += 20; break;
        case 0x0B: at += 24; break;
        case 0x0C: at += 20; break;
        case 0x0D: at += 16; break;
        case 0x0E: at += 28; break;
        case 0x0F: at += 24 + (ops[at + 22] | (ops[at + 23] << 8)); break;
        case 0x10: at += 16; break;
        case 0x11: break;
        case 0x12: {
          const n = ops[at + 14] | (ops[at + 15] << 8);
          at += 16;
          for (let k = 0; k < n; k++) at += 1 + 8 * [1, 1, 2, 3, 0][ops[at]];
          break;
        }
        default: throw new Error("terse-web: unknown drawing command " + op);
      }
    }
    return at;
  }

  // ---- The page's machine ------------------------------------------------

  async function run(root, title) {
    const canvas = root.querySelector("canvas");
    const statusEl = root.querySelector(".status");
    const status = (t) => { statusEl.textContent = t; };
    const bundle = await load(status);
    {
      const b = put(bundle), t = put(enc.encode(title || ""));
      const ok = wasm.tw_start(b[0], b[1], t[0], t[1]);
      free(b); free(t);
      if (!ok) throw new Error(dec.decode(bytes().slice(wasm.tw_error(), wasm.tw_error() + wasm.tw_error_len())));
    }
    root.classList.add("running");
    canvas.focus();

    const ctx = canvas.getContext("2d");
    const screens = new Map(); // key -> {ops, phosphor, persistence, canvas, ctx, dirty}
    let trails = true;
    let view = { x: 0, y: 0, w: 1, h: 1, s: 1 };

    function layout() {
      const dpr = window.devicePixelRatio || 1;
      const r = canvas.getBoundingClientRect();
      const W = Math.max(1, Math.round(r.width * dpr)), H = Math.max(1, Math.round(r.height * dpr));
      if (canvas.width !== W || canvas.height !== H) { canvas.width = W; canvas.height = H; }
      const s = Math.min(W / 640, H / 480);
      const w = Math.floor(640 * s), h = Math.floor(480 * s);
      view = { x: Math.floor((W - w) / 2), y: Math.floor((H - h) / 2), w, h, s: w / 640, dpr };
    }

    // The canvas of a screen at w x h pixels, drawn again if needed.
    function screenImage(key, w, h) {
      const sc = screens.get(key);
      if (!sc) return null;
      w = Math.max(1, Math.round(w)); h = Math.max(1, Math.round(h));
      if (!sc.canvas) { sc.canvas = document.createElement("canvas"); sc.ctx = sc.canvas.getContext("2d"); }
      if (sc.canvas.width !== w || sc.canvas.height !== h) { sc.canvas.width = w; sc.canvas.height = h; sc.dirty = true; sc.fresh = true; }
      const fade = sc.phosphor && trails && sc.persistence > 0 && !sc.fresh;
      if (sc.dirty || fade) {
        const c = sc.ctx;
        c.setTransform(1, 0, 0, 1, 0, 0);
        if (fade) {
          // The afterglow: what was drawn fades (to 2% in persistence/6 frames).
          const k = Math.pow(0.02, 1 / Math.max(1, sc.persistence / 6));
          c.fillStyle = `rgba(0,0,0,${(1 - k).toFixed(3)})`;
        } else c.fillStyle = "#000";
        c.fillRect(0, 0, w, h);
        c.setTransform(w / 640, 0, 0, h / 480, 0, 0);
        c.save();
        c.beginPath(); c.rect(0, 0, 640, 480); c.clip();
        play(c, sc.ops, 0, sc.phosphor);
        c.restore();
        sc.dirty = false; sc.fresh = false;
      }
      return sc.canvas;
    }

    // An image drawn into the triangle (p0, p1, p2) of the main canvas,
    // its (u, v) points (q0, q1, q2) mapped there (an affine piece of a panel).
    function triangle(img, sx, sy, q, p) {
      const [[u0, v0], [u1, v1], [u2, v2]] = q, [[x0, y0], [x1, y1], [x2, y2]] = p;
      const d = (u1 - u0) * (v2 - v0) - (u2 - u0) * (v1 - v0);
      if (Math.abs(d) < 1e-9) return;
      const a = ((x1 - x0) * (v2 - v0) - (x2 - x0) * (v1 - v0)) / d;
      const b = ((y1 - y0) * (v2 - v0) - (y2 - y0) * (v1 - v0)) / d;
      const c = ((x2 - x0) * (u1 - u0) - (x1 - x0) * (u2 - u0)) / d;
      const e = ((y2 - y0) * (u1 - u0) - (y1 - y0) * (u2 - u0)) / d;
      ctx.save();
      ctx.beginPath(); ctx.moveTo(x0, y0); ctx.lineTo(x1, y1); ctx.lineTo(x2, y2); ctx.closePath(); ctx.clip();
      ctx.setTransform(a, b, c, e, x0 - a * u0 - c * v0, y0 - b * u0 - e * v0);
      ctx.drawImage(img, sx, sy);
      ctx.restore();
    }

    function draw() {
      const ptr = wasm.tw_draw(), len = wasm.tw_draw_len();
      const ops = bytes().subarray(ptr, ptr + len);
      const dv = new DataView(ops.buffer, ops.byteOffset, ops.byteLength);
      const f = (i) => dv.getFloat32(i, true);
      const u32 = (i) => dv.getUint32(i, true);
      layout();
      const V = view, px = (x) => V.x + x * V.s, py = (y) => V.y + y * V.s;
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.imageSmoothingQuality = "high";
      const seen = new Set();
      let at = 0;
      while (at < ops.length) {
        const op = ops[at++];
        if (op === 0x01) {
          const key = u32(at), flags = ops[at + 4], persistence = ops[at + 5];
          at += 6;
          seen.add(key);
          let sc = screens.get(key);
          if (!sc) { sc = { ops: new Uint8Array(0) }; screens.set(key, sc); }
          sc.phosphor = !!(flags & 2); sc.persistence = persistence;
          if (flags & 1) {
            const end = skip(ops, at);
            sc.ops = ops.slice(at, end);
            sc.dirty = true;
            at = end;
          }
        } else if (op === 0x14) {
          const img = screenImage(u32(at), V.w, V.h);
          at += 4;
          if (img) ctx.drawImage(img, V.x, V.y);
        } else if (op === 0x15) {
          const key = u32(at), x = px(f(at + 4)), y = py(f(at + 8)), w = f(at + 12) * V.s, h = f(at + 16) * V.s;
          at += 20;
          const img = screenImage(key, w, h);
          if (img) ctx.drawImage(img, Math.round(x), Math.round(y), Math.round(w), Math.round(h));
        } else if (op === 0x16) {
          const key = u32(at), src = [f(at + 4), f(at + 8), f(at + 12), f(at + 16)];
          const c = [0, 1, 2, 3].map((i) => [px(f(at + 20 + 8 * i)), py(f(at + 24 + 8 * i))]);
          at += 52;
          const xs = c.map((p) => p[0]), ys = c.map((p) => p[1]);
          const bw = Math.max(...xs) - Math.min(...xs), bh = Math.max(...ys) - Math.min(...ys);
          // The picture at about the size the panel shows it, as a 4:3 image.
          const iw = Math.min(2 * V.w, Math.max(64, Math.max(bw, bh * 4 / 3)));
          const img = key ? screenImage(key, iw, iw * 3 / 4) : null;
          if (!img) {
            ctx.beginPath(); ctx.moveTo(...c[0]); for (const p of c.slice(1)) ctx.lineTo(...p); ctx.closePath();
            ctx.fillStyle = "#141418"; ctx.fill();
            continue;
          }
          const k = img.width / 640;
          const q = [[src[0] * k, src[1] * k], [(src[0] + src[2]) * k, src[1] * k], [(src[0] + src[2]) * k, (src[1] + src[3]) * k], [src[0] * k, (src[1] + src[3]) * k]];
          triangle(img, 0, 0, [q[0], q[1], q[3]], [c[0], c[1], c[3]]);
          triangle(img, 0, 0, [q[1], q[2], q[3]], [c[1], c[2], c[3]]);
        } else if (op === 0x17) {
          ctx.save();
          ctx.beginPath(); ctx.rect(V.x, V.y, V.w, V.h); ctx.clip();
          ctx.setTransform(V.s, 0, 0, V.s, V.x, V.y);
          at = play(ctx, ops, at, true);
          ctx.restore();
        } else throw new Error("terse-web: unknown command " + op);
      }
      for (const k of [...screens.keys()]) if (!seen.has(k)) screens.delete(k);
    }

    // ---- Input -----------------------------------------------------------

    const mods = (e) => (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
    const withStr = (s, fn) => { const b = put(enc.encode(s)); try { return fn(b[0], b[1]); } finally { free(b); } };
    let pasteKey = null;
    canvas.addEventListener("keydown", (e) => {
      const m = mods(e);
      // Ctrl+V and Shift+Insert paste (SPEC §events): the browser's paste event brings the text.
      const isPaste = (e.code === "KeyV" && e.ctrlKey && !e.altKey && !e.metaKey) || (e.code === "Insert" && e.shiftKey);
      if (isPaste && !e.repeat) { pasteKey = { code: e.code, m }; return; }
      const usage = withStr(e.code, (p, n) => wasm.tw_key(1, p, n, m));
      const chars = [...e.key];
      if (chars.length === 1 && !(e.ctrlKey && !e.altKey) && !e.metaKey) {
        const cp = chars[0].codePointAt(0);
        if (cp >= 0x20 && !(cp >= 0x7f && cp < 0xa0)) wasm.tw_text(cp);
      }
      if (usage || chars.length === 1) e.preventDefault();
    });
    canvas.addEventListener("keyup", (e) => {
      if (pasteKey && pasteKey.code === e.code) {
        // No paste event came (nothing to paste): the key alone.
        withStr(e.code, (p, n) => wasm.tw_key(1, p, n, pasteKey.m));
        pasteKey = null;
      }
      withStr(e.code, (p, n) => wasm.tw_key(0, p, n, mods(e)));
      e.preventDefault();
    });
    canvas.addEventListener("paste", (e) => {
      const text = (e.clipboardData && e.clipboardData.getData("text")) || "";
      const k = pasteKey || { code: "KeyV", m: 2 };
      pasteKey = null;
      const c = put(enc.encode(k.code)), t = put(enc.encode(text));
      wasm.tw_paste(c[0], c[1], k.m, t[0], t[1]);
      free(c); free(t);
      e.preventDefault();
    });
    const unitPos = (e) => {
      const r = canvas.getBoundingClientRect(), d = view.dpr;
      const x = ((e.clientX - r.left) * d - view.x) * 640 / view.w, y = ((e.clientY - r.top) * d - view.y) * 480 / view.h;
      return [Math.max(0, Math.min(639, Math.floor(x))), Math.max(0, Math.min(479, Math.floor(y)))];
    };
    const buttons = (b) => (b & 1) | (b & 2) | ((b & 4) ? 4 : 0); // DOM: 1 primary, 2 secondary, 4 middle
    canvas.addEventListener("pointermove", (e) => { const [x, y] = unitPos(e); wasm.tw_pointer(x, y); });
    canvas.addEventListener("pointerdown", (e) => {
      canvas.focus();
      canvas.setPointerCapture(e.pointerId);
      const [x, y] = unitPos(e); wasm.tw_pointer(x, y); wasm.tw_buttons(buttons(e.buttons));
      e.preventDefault();
    });
    canvas.addEventListener("pointerup", (e) => { const [x, y] = unitPos(e); wasm.tw_pointer(x, y); wasm.tw_buttons(buttons(e.buttons)); });
    canvas.addEventListener("contextmenu", (e) => e.preventDefault());
    canvas.addEventListener("wheel", (e) => {
      // One step per line, or per terminal row of travel (as terse-host).
      const row = view.h / 30 / view.dpr;
      const k = e.deltaMode === 1 ? 1 : e.deltaMode === 2 ? 30 : 1 / Math.max(1, row);
      wasm.tw_wheel(e.deltaY * k, e.deltaX * k);
      e.preventDefault();
    }, { passive: false });
    const home = root.querySelector(".home");
    if (home) home.addEventListener("click", () => { wasm.tw_home(); canvas.focus(); });
    const trailBox = root.querySelector(".trails");
    if (trailBox) trailBox.addEventListener("change", () => { trails = trailBox.checked; canvas.focus(); });

    // ---- The frame loop: one machine frame per 1/60 s, never catching up
    // (SPEC §frames: a host that falls behind lets virtual time lag).
    let next = performance.now(), frames = 0, stopped = false;
    let stat = { run: 0, draw: 0, n: 0, used: 0 };
    function tick(now) {
      if (now >= next && !stopped) {
        next = Math.max(next + FRAME_MS, now - FRAME_MS);
        const t0 = performance.now();
        const running = wasm.tw_frame(Date.now() / 1000);
        const t1 = performance.now();
        draw();
        const t2 = performance.now();
        frames++;
        stat.run += t1 - t0; stat.draw += t2 - t1; stat.n++; stat.used += wasm.tw_used();
        const copied = wasm.tw_take_copied();
        if (copied && navigator.clipboard) navigator.clipboard.writeText(dec.decode(bytes().slice(wasm.tw_copied(), wasm.tw_copied() + copied))).catch(() => {});
        if (stat.n === 60) {
          status(`frame ${frames}: machines ${(stat.run / 60).toFixed(2)} ms, drawing ${(stat.draw / 60).toFixed(2)} ms a frame; ${Math.round(stat.used / 60).toLocaleString()} units a frame`);
          root.dataset.frameMs = (stat.run / 60).toFixed(3);
          root.dataset.drawMs = (stat.draw / 60).toFixed(3);
          stat = { run: 0, draw: 0, n: 0, used: 0 };
        }
        root.dataset.frames = frames;
        if (!running) { stopped = true; status("The machine stopped. Reload the page to start again."); }
      }
      requestAnimationFrame(tick);
    }
    requestAnimationFrame(tick);
  }

  // ---- The page: Start loads and runs; nothing is fetched before.
  for (const root of document.querySelectorAll(".terse-demo")) {
    const go = (title) => {
      root.classList.add("loading");
      run(root, title).catch((e) => {
        root.querySelector(".status").textContent = "Could not start: " + e.message;
        console.error(e);
      });
    };
    root.querySelectorAll("[data-start]").forEach((b) => b.addEventListener("click", () => {
      root.querySelectorAll("[data-start]").forEach((x) => { x.disabled = true; });
      go(b.dataset.start);
    }));
    // try.html#sparks (or any built-in's title) starts that program alone at once.
    const hash = decodeURIComponent(location.hash.slice(1));
    if (hash === "shell") go("");
    else if (/^[a-z][a-z0-9 -]{0,30}$/i.test(hash)) go(hash);
  }
})();
