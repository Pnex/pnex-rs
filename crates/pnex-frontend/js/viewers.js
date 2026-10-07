// Viewers WebGL de la bibliothèque média (D21) — pattern tron-gerbe.js :
// IIFE exposant `window.pnexViewers`, bundle esbuild
// (`bun run js:viewers` → assets/viewers.js, gitignoré + stub js:ensure).
//
// Glue défensive : WebGL/libs absentes ou erreurs → false (badge « aperçu
// indisponible » côté page — jamais de panique UI, école tron-gerbe.js).

/* eslint-disable */
'use strict';

var pannellumLib = null;
try {
  pannellumLib = require('pannellum');
  require('pannellum/build/pannellum.css');
  // Flèches PNeX des hotspots de tour (fusionnées dans assets/viewers.css
  // par esbuild — inline natif + <link> web, école tailwind.css).
  require('./hotspots.css');
} catch (e) {
  console.warn('[pnex-viewers] pannellum indisponible', e);
}
if (!pannellumLib || typeof pannellumLib.viewer !== 'function') {
  pannellumLib = (typeof window !== 'undefined' && typeof window.pannellum?.viewer === 'function')
    ? window.pannellum
    : null;
}

var gsplatLib = null;
try {
  gsplatLib = require('gsplat');
} catch (e) {
  console.warn('[pnex-viewers] gsplat indisponible', e);
}
// Same shape issue for gsplat (pure ESM): the 1.2.x API (`WebGLRenderer`,
// `Scene`, `Loader`…) must be there — otherwise fall back to a global.
if (!gsplatLib || typeof gsplatLib.WebGLRenderer !== 'function') {
  gsplatLib = (typeof window !== 'undefined' && typeof window.gsplat?.WebGLRenderer === 'function')
    ? window.gsplat
    : null;
}

function hostOf(hostId) {
  return document.getElementById(hostId);
}

function hasWebGL(wantWebGL2) {
  try {
    var canvas = document.createElement('canvas');
    var gl = wantWebGL2
      ? canvas.getContext('webgl2')
      : canvas.getContext('webgl') || canvas.getContext('experimental-webgl');
    if (gl) {
      // Sonde jetable : sans loseContext, chaque appel fuite un contexte
      // (limite navigateur ~16/page) et les mounts finissent par échouer
      // de façon aléatoire — symptôme « ça marche parfois ».
      var lose = gl.getExtension('WEBGL_lose_context');
      if (lose) lose.loseContext();
    }
    return !!gl;
  } catch (e) {
    return false;
  }
}

function warn(msg, err) {
  console.warn('[pnex-viewers] ' + msg, err || '');
}

function mountPanorama(hostId, url) {
  var host = hostOf(hostId);
  if (!host || !pannellumLib || !hasWebGL(false)) {
    return false;
  }
  try {
    host.innerHTML = '';
    var viewer = pannellumLib.viewer(host, {
      // Security (SEC-6): pannellum renders hotspot text as raw HTML unless
      // escapeHTML is set; labels are user content shown on the app origin.
      escapeHTML: true,
      type: 'equirectangular',
      panorama: url,
      autoLoad: true,
      autoRotate: -0.4,
      showControls: true,
    });
    host._pnexViewer = viewer;
    return true;
  } catch (e) {
    warn('panorama mount échoué', e);
    return false;
  }
}

// ─────────────────────────────────────────────────────────────────
// Gaussian splat viewer (gsplat 1.2.9: WebGLRenderer / Scene / Camera /
// OrbitControls / Loader / PLYLoader) + 3D annotations (D147). Layout:
// host > wrap(relative) > [canvas, overlay(pointer-events none)]; the
// markers live in the overlay (pointer-events auto), positioned every
// frame by projecting their world point with the camera's viewProj.
// No occlusion test: a marker stays visible through the geometry.
// Events reuse the panorama globals and seq counters (__pnexAnnotClick /
// Place / Move) with x/y/z instead of yaw/pitch.
function isPly(buffer) {
  var head = new Uint8Array(buffer, 0, Math.min(4, buffer.byteLength));
  return head.length === 4 && head[0] === 0x70 && head[1] === 0x6c && head[2] === 0x79 && head[3] === 0x0a;
}

function mountSplat(hostId, url) {
  var host = hostOf(hostId);
  if (!host || !gsplatLib || !hasWebGL(true)) {
    return false;
  }
  try {
    stopSplat(host);
    host.innerHTML = '';
    var wrap = document.createElement('div');
    wrap.className = 'pnex-splat-wrap';
    wrap.style.position = 'relative';
    wrap.style.width = '100%';
    wrap.style.height = '100%';
    wrap.style.overflow = 'hidden';
    var canvas = document.createElement('canvas');
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    canvas.style.display = 'block';
    var overlay = document.createElement('div');
    overlay.className = 'pnex-splat-overlay';
    overlay.style.position = 'absolute';
    overlay.style.inset = '0';
    overlay.style.pointerEvents = 'none';
    wrap.appendChild(canvas);
    wrap.appendChild(overlay);
    host.appendChild(wrap);
    var st = {
      host: host,
      canvas: canvas,
      overlay: overlay,
      renderer: null,
      scene: null,
      camera: null,
      controls: null,
      pos: null,
      alpha: null,
      count: 0,
      R: 1,
      // Data → world rotation (identity until the cloud is analysed).
      m: [1, 0, 0, 0, 1, 0, 0, 0, 1],
      markers: {},
      editable: false,
      placeMode: false,
      abort: typeof AbortController === 'function' ? new AbortController() : null,
      rendered: false,
      // Card overlay to drive, kept on the host across (re)mounts.
      cards: host._pnexCardsOverlay || null,
      listeners: [],
    };
    host._pnexSplat = st;
    bindSplatEvents(st);
    // Markers / place mode set before the mount (the editor may push them
    // while the splat blob is still downloading).
    var pending = host._pnexSplatPending;
    host._pnexSplatPending = null;
    if (pending) {
      splatSetAnnotations(hostId, pending.json, pending.editable);
      st.placeMode = !!pending.placeMode;
    }
    (async function () {
      try {
        var resp = await fetch(url, st.abort ? { signal: st.abort.signal } : undefined);
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        var buffer = await resp.arrayBuffer();
        if (host._pnexSplat !== st) return;
        var renderer = new gsplatLib.WebGLRenderer(canvas);
        var scene = new gsplatLib.Scene();
        var camera = new gsplatLib.Camera();
        var splat = isPly(buffer)
          ? gsplatLib.PLYLoader.LoadFromArrayBuffer(buffer, scene)
          : gsplatLib.Loader.LoadFromArrayBuffer(buffer, scene);
        // Snapshot for picking BEFORE the first render: the render worker
        // takes over (empties) the data buffers.
        var data = splat.data;
        st.count = data.vertexCount;
        st.pos = new Float32Array(data.positions);
        st.alpha = new Uint8Array(st.count);
        var colors = data.colors;
        for (var i = 0; i < st.count; i++) st.alpha[i] = colors[4 * i + 3];
        var frame = splatFraming(st);
        st.R = frame.R;
        st.m = frame.m;
        // Orient the splat (data → world rotation) and move the picking
        // snapshot to world coordinates; annotations stay in data
        // coordinates (converted at the glue boundary).
        var qv = quatOfMat3(frame.m);
        splat.rotation = new gsplatLib.Quaternion(qv[0], qv[1], qv[2], qv[3]);
        for (var p = 0; p < st.count; p++) {
          var wpt = mulMat3(frame.m, [st.pos[3 * p], st.pos[3 * p + 1], st.pos[3 * p + 2]]);
          st.pos[3 * p] = wpt[0];
          st.pos[3 * p + 1] = wpt[1];
          st.pos[3 * p + 2] = wpt[2];
        }
        Object.keys(st.markers).forEach(function (id) {
          var mk = st.markers[id];
          var wm = mulMat3(frame.m, mk.data);
          mk.x = wm[0];
          mk.y = wm[1];
          mk.z = wm[2];
        });
        camera.data.near = Math.max(frame.R * 1e-3, 1e-3);
        camera.data.far = frame.R * 100;
        var center = new gsplatLib.Vector3(frame.center[0], frame.center[1], frame.center[2]);
        var controls = new gsplatLib.OrbitControls(
          camera, canvas, 0.5, 0.35, frame.R * 1.5, false, center
        );
        controls.minZoom = frame.R * 0.05;
        controls.maxZoom = frame.R * 10;
        st.renderer = renderer;
        st.scene = scene;
        st.camera = camera;
        st.controls = controls;
        var loop = function () {
          if (host._pnexSplat !== st) return;
          try {
            renderer.resize();
            controls.update();
            renderer.render(scene, camera);
            st.rendered = true;
            projectSplatMarkers(st);
          } catch (e) {
            warn('splat frame failed', e);
          }
          host._pnexRaf = requestAnimationFrame(loop);
        };
        host._pnexRaf = requestAnimationFrame(loop);
      } catch (e) {
        if (host._pnexSplat === st) warn('splat load failed', e);
      }
    })();
    return true;
  } catch (e) {
    warn('splat mount failed', e);
    return false;
  }
}

// Framing + orientation from the dense part of the cloud (captures come
// with any axis convention and far floaters):
//  - centre = median, scale R = half-diagonal of the inter-quartile box;
//  - vertical = axis of least variance of the points near the centre (the
//    ground is the dominant flat structure), oriented by the skewness along
//    it (vegetation and objects stick UP from the ground);
//  - the splat is rotated so this vertical becomes gsplat's up (-y).
// Returns { center (world), R, m (3x3 row-major data → world rotation) }.
function splatFraming(st) {
  var n = st.count;
  var step = Math.max(1, Math.floor(n / 20000));
  var xs = [], ys = [], zs = [];
  for (var i = 0; i < n; i += step) {
    if (st.alpha[i] < 40) continue;
    xs.push(st.pos[3 * i]);
    ys.push(st.pos[3 * i + 1]);
    zs.push(st.pos[3 * i + 2]);
  }
  var ident = [1, 0, 0, 0, 1, 0, 0, 0, 1];
  if (xs.length < 10) return { center: [0, 0, 0], R: 1, m: ident };
  var q = function (arr, p) {
    var a = arr.slice().sort(function (x, y) { return x - y; });
    return a[Math.min(a.length - 1, Math.floor(p * a.length))];
  };
  var c = [q(xs, 0.5), q(ys, 0.5), q(zs, 0.5)];
  var dx = q(xs, 0.75) - q(xs, 0.25), dy = q(ys, 0.75) - q(ys, 0.25), dz = q(zs, 0.75) - q(zs, 0.25);
  var R = Math.max(0.5 * Math.sqrt(dx * dx + dy * dy + dz * dz), 0.1);
  // Covariance of the points within 1.5 R of the centre.
  var C = [0, 0, 0, 0, 0, 0, 0, 0, 0], cnt = 0, lim = 1.5 * R;
  for (var k = 0; k < xs.length; k++) {
    var v = [xs[k] - c[0], ys[k] - c[1], zs[k] - c[2]];
    if (Math.sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) > lim) continue;
    for (var a = 0; a < 3; a++) for (var b = 0; b < 3; b++) C[3 * a + b] += v[a] * v[b];
    cnt++;
  }
  if (cnt < 10) return { center: c, R: R, m: ident };
  var up = smallestEigenvector(C);
  // Skewness along the axis: the long tail points up.
  var proj = [];
  for (var k2 = 0; k2 < xs.length; k2++) {
    var w = [xs[k2] - c[0], ys[k2] - c[1], zs[k2] - c[2]];
    if (Math.sqrt(w[0] * w[0] + w[1] * w[1] + w[2] * w[2]) > lim) continue;
    proj.push(w[0] * up[0] + w[1] * up[1] + w[2] * up[2]);
  }
  var med = q(proj, 0.5), m2 = 0, m3 = 0;
  proj.forEach(function (p) { var d = p - med; m2 += d * d; m3 += d * d * d; });
  // Negative skewness = long tail toward -axis: that side is up.
  if (m3 < 0) up = [-up[0], -up[1], -up[2]];
  var m = rotationTo(up, [0, -1, 0]);
  return { center: mulMat3(m, c), R: R, m: m };
}

// Jacobi eigen-decomposition of a symmetric 3x3 (row-major) — returns the
// unit eigenvector of the smallest eigenvalue.
function smallestEigenvector(C) {
  var A = [[C[0], C[1], C[2]], [C[3], C[4], C[5]], [C[6], C[7], C[8]]];
  var V = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
  for (var it = 0; it < 50; it++) {
    var p = 0, qq = 1, big = 0;
    for (var i = 0; i < 3; i++) for (var j = i + 1; j < 3; j++) {
      if (Math.abs(A[i][j]) > big) { big = Math.abs(A[i][j]); p = i; qq = j; }
    }
    if (big < 1e-12) break;
    var th = 0.5 * Math.atan2(2 * A[p][qq], A[qq][qq] - A[p][p]);
    var co = Math.cos(th), si = Math.sin(th);
    for (var k = 0; k < 3; k++) {
      var akp = A[k][p], akq = A[k][qq];
      A[k][p] = co * akp - si * akq;
      A[k][qq] = si * akp + co * akq;
    }
    for (var k1 = 0; k1 < 3; k1++) {
      var apk = A[p][k1], aqk = A[qq][k1];
      A[p][k1] = co * apk - si * aqk;
      A[qq][k1] = si * apk + co * aqk;
    }
    for (var k2 = 0; k2 < 3; k2++) {
      var vkp = V[k2][p], vkq = V[k2][qq];
      V[k2][p] = co * vkp - si * vkq;
      V[k2][qq] = si * vkp + co * vkq;
    }
  }
  var best = 0;
  for (var e = 1; e < 3; e++) if (A[e][e] < A[best][best]) best = e;
  var u = [V[0][best], V[1][best], V[2][best]];
  var len = Math.sqrt(u[0] * u[0] + u[1] * u[1] + u[2] * u[2]) || 1;
  return [u[0] / len, u[1] / len, u[2] / len];
}

// Rotation matrix (row-major) taking unit vector a onto unit vector b.
function rotationTo(a, b) {
  var vx = a[1] * b[2] - a[2] * b[1], vy = a[2] * b[0] - a[0] * b[2], vz = a[0] * b[1] - a[1] * b[0];
  var cth = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  if (cth < -0.999999) {
    // Opposite vectors: half-turn around any axis orthogonal to a.
    var ax = Math.abs(a[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0];
    var ox = a[1] * ax[2] - a[2] * ax[1], oy = a[2] * ax[0] - a[0] * ax[2], oz = a[0] * ax[1] - a[1] * ax[0];
    var ol = Math.sqrt(ox * ox + oy * oy + oz * oz);
    ox /= ol; oy /= ol; oz /= ol;
    return [2 * ox * ox - 1, 2 * ox * oy, 2 * ox * oz, 2 * oy * ox, 2 * oy * oy - 1, 2 * oy * oz, 2 * oz * ox, 2 * oz * oy, 2 * oz * oz - 1];
  }
  var k = 1 / (1 + cth);
  return [
    vx * vx * k + cth, vx * vy * k - vz, vx * vz * k + vy,
    vy * vx * k + vz, vy * vy * k + cth, vy * vz * k - vx,
    vz * vx * k - vy, vz * vy * k + vx, vz * vz * k + cth,
  ];
}

function mulMat3(m, v) {
  return [
    m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
    m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
    m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
  ];
}

function mulMat3T(m, v) {
  return [
    m[0] * v[0] + m[3] * v[1] + m[6] * v[2],
    m[1] * v[0] + m[4] * v[1] + m[7] * v[2],
    m[2] * v[0] + m[5] * v[1] + m[8] * v[2],
  ];
}

// Quaternion (x, y, z, w) of a row-major rotation matrix.
function quatOfMat3(m) {
  var tr = m[0] + m[4] + m[8], qx, qy, qz, qw, S;
  if (tr > 0) {
    S = Math.sqrt(tr + 1) * 2; qw = 0.25 * S;
    qx = (m[7] - m[5]) / S; qy = (m[2] - m[6]) / S; qz = (m[3] - m[1]) / S;
  } else if (m[0] > m[4] && m[0] > m[8]) {
    S = Math.sqrt(1 + m[0] - m[4] - m[8]) * 2; qw = (m[7] - m[5]) / S;
    qx = 0.25 * S; qy = (m[1] + m[3]) / S; qz = (m[2] + m[6]) / S;
  } else if (m[4] > m[8]) {
    S = Math.sqrt(1 + m[4] - m[0] - m[8]) * 2; qw = (m[2] - m[6]) / S;
    qx = (m[1] + m[3]) / S; qy = 0.25 * S; qz = (m[5] + m[7]) / S;
  } else {
    S = Math.sqrt(1 + m[8] - m[0] - m[4]) * 2; qw = (m[3] - m[1]) / S;
    qx = (m[2] + m[6]) / S; qy = (m[5] + m[7]) / S; qz = 0.25 * S;
  }
  return [qx, qy, qz, qw];
}

// World point → overlay pixels (row-vector convention, column-major
// buffer; the projection already flips y). null when behind the camera.
function splatProject(st, x, y, z) {
  var m = st.camera.data.viewProj.buffer;
  var cx = x * m[0] + y * m[4] + z * m[8] + m[12];
  var cy = x * m[1] + y * m[5] + z * m[9] + m[13];
  var cw = x * m[3] + y * m[7] + z * m[11] + m[15];
  if (cw <= 1e-6) return null;
  var w = st.canvas.clientWidth, h = st.canvas.clientHeight;
  return { px: (cx / cw + 1) / 2 * w, py: (1 - cy / cw) / 2 * h, w: w, h: h };
}

function projectSplatMarkers(st) {
  if (!st.camera) return;
  Object.keys(st.markers).forEach(function (id) {
    var mk = st.markers[id];
    if (mk.dragging) return;
    var p = splatProject(st, mk.x, mk.y, mk.z);
    var vis = !!p && p.px > -0.05 * p.w && p.px < 1.05 * p.w && p.py > -0.05 * p.h && p.py < 1.05 * p.h;
    if (vis !== mk.vis) {
      mk.vis = vis;
      mk.el.style.visibility = vis ? 'visible' : 'hidden';
    }
    if (vis && (Math.abs(p.px - mk.px) > 0.5 || Math.abs(p.py - mk.py) > 0.5)) {
      mk.px = p.px;
      mk.py = p.py;
      mk.el.style.transform = 'translate(' + p.px + 'px,' + p.py + 'px) translate(-50%,-50%)';
    }
  });
  if (st.cards) placeCards(st.host, st.cards, function (id) {
    var mk = st.markers[id];
    return mk && mk.vis ? { x: mk.px, y: mk.py } : null;
  });
}

// Pick the visible surface point under (clientX, clientY): one pass over
// the gaussian centres, keeping those within ~6 px (angular) of the
// camera ray; the nearest depth cluster of >= 3 points wins (isolated
// floaters are skipped). Returns {x, y, z} or null.
var splatCandidates = new Float32Array(8192);
function pickSplat(st, clientX, clientY) {
  if (!st.camera || !st.pos) return null;
  var rect = st.canvas.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return null;
  var ndcX = 2 * (clientX - rect.left) / rect.width - 1;
  var ndcY = 1 - 2 * (clientY - rect.top) / rect.height;
  var d = st.camera.screenPointToRay(ndcX, ndcY);
  var o = st.camera.position;
  var tol = 6 / Math.max(st.camera.data.fy, 1);
  var tol2 = tol * tol;
  var near = st.camera.data.near;
  var pos = st.pos, alpha = st.alpha, n = st.count;
  var found = 0, bestR = Infinity, bestT = 0;
  for (var i = 0; i < n; i++) {
    if (alpha[i] < 40) continue;
    var vx = pos[3 * i] - o.x, vy = pos[3 * i + 1] - o.y, vz = pos[3 * i + 2] - o.z;
    var t = vx * d.x + vy * d.y + vz * d.z;
    if (t <= near) continue;
    var r = (vx * vx + vy * vy + vz * vz - t * t) / (t * t);
    if (r < bestR) { bestR = r; bestT = t; }
    if (r <= tol2 && found < splatCandidates.length) splatCandidates[found++] = t;
  }
  var hit = -1;
  if (found > 0) {
    var ts = Array.prototype.slice.call(splatCandidates.subarray(0, found)).sort(function (a, b) { return a - b; });
    var band = 0.02 * st.R;
    for (var k = 0; k < ts.length; k++) {
      var j = k;
      while (j < ts.length && ts[j] <= ts[k] + band) j++;
      if (j - k >= 3) { hit = ts[k]; break; }
    }
    if (hit < 0) hit = ts[0];
  } else if (bestR <= 25 * 25 * tol2 / 36) {
    hit = bestT;
  }
  if (hit < 0) return null;
  return { x: o.x + d.x * hit, y: o.y + d.y * hit, z: o.z + d.z * hit };
}

function splatListen(st, target, type, fn, capture) {
  target.addEventListener(type, fn, capture);
  st.listeners.push([target, type, fn, capture]);
}

function bindSplatEvents(st) {
  var host = st.host;
  var down = null;
  var drag = null;
  splatListen(st, host, 'mousedown', function (e) {
    down = { x: e.clientX, y: e.clientY };
    var el = e.target && e.target.closest ? e.target.closest('.pnex-annot') : null;
    if (!el || e.button !== 0) return;
    var id = el.dataset.annotId;
    var mk = st.markers[id];
    if (!mk) return;
    e.stopPropagation();
    e.preventDefault();
    drag = { id: id, mk: mk, sx: e.clientX, sy: e.clientY, moved: false };
  }, true);
  splatListen(st, document, 'mousemove', function (e) {
    if (!drag || !st.editable) return;
    var dx = e.clientX - drag.sx, dy = e.clientY - drag.sy;
    if (!drag.moved && Math.abs(dx) + Math.abs(dy) < 4) return;
    drag.moved = true;
    drag.mk.dragging = true;
    var rect = st.overlay.getBoundingClientRect();
    drag.mk.el.style.transform = 'translate(' + (e.clientX - rect.left) + 'px,' + (e.clientY - rect.top) + 'px) translate(-50%,-50%)';
  }, false);
  splatListen(st, document, 'mouseup', function (e) {
    if (!drag) return;
    var d = drag;
    drag = null;
    d.mk.dragging = false;
    d.mk.px = -1e9;
    if (!d.moved) {
      annotSeq += 1;
      window.__pnexAnnotClick = JSON.stringify({ seq: annotSeq, item_id: d.id });
      return;
    }
    var p = pickSplat(st, e.clientX, e.clientY);
    if (!p) return;
    d.mk.x = p.x;
    d.mk.y = p.y;
    d.mk.z = p.z;
    var dm = mulMat3T(st.m, [p.x, p.y, p.z]);
    d.mk.data = dm;
    annotMoveSeq += 1;
    window.__pnexAnnotMove = JSON.stringify({ seq: annotMoveSeq, item_id: d.id, x: dm[0], y: dm[1], z: dm[2] });
  }, false);
  splatListen(st, host, 'click', function (e) {
    if (!st.placeMode) return;
    if (e.target && e.target.closest && e.target.closest('.pnex-annot')) return;
    if (down && Math.abs(e.clientX - down.x) + Math.abs(e.clientY - down.y) >= 4) return;
    var p = pickSplat(st, e.clientX, e.clientY);
    if (!p) return;
    var dp = mulMat3T(st.m, [p.x, p.y, p.z]);
    annotPlaceSeq += 1;
    window.__pnexAnnotPlace = JSON.stringify({ seq: annotPlaceSeq, x: dp[0], y: dp[1], z: dp[2] });
  }, false);
}

function splatPending(host) {
  if (!host._pnexSplatPending) host._pnexSplatPending = { json: '[]', editable: false, placeMode: false };
  return host._pnexSplatPending;
}

function splatSetAnnotations(hostId, itemsJson, editable) {
  var host = hostOf(hostId);
  if (!host) return false;
  var st = host._pnexSplat;
  if (!st) {
    // Not mounted yet: kept for the mount.
    var p = splatPending(host);
    p.json = itemsJson;
    p.editable = !!editable;
    return true;
  }
  var items;
  try {
    items = JSON.parse(itemsJson);
  } catch (e) {
    warn('splat annot JSON unreadable', e);
    return false;
  }
  if (!Array.isArray(items)) return false;
  st.editable = !!editable;
  var keep = {};
  items.forEach(function (it) {
    if (!it || !it.id || !isFinite(it.x) || !isFinite(it.y) || !isFinite(it.z)) return;
    keep[it.id] = true;
    var mk = st.markers[it.id];
    if (!mk) {
      var el = document.createElement('div');
      el.style.position = 'absolute';
      el.style.left = '0';
      el.style.top = '0';
      el.style.pointerEvents = 'auto';
      el.style.visibility = 'hidden';
      el.dataset.annotId = it.id;
      st.overlay.appendChild(el);
      mk = { el: el, px: -1e9, py: -1e9, vis: false, dragging: false };
      st.markers[it.id] = mk;
    }
    mk.data = [it.x, it.y, it.z];
    var wpos = mulMat3(st.m, mk.data);
    mk.x = wpos[0];
    mk.y = wpos[1];
    mk.z = wpos[2];
    mk.px = -1e9;
    mk.el.className = 'pnex-annot pnex-annot-' + (it.kind || 'note');
    mk.el.title = it.label || '';
    mk.el.style.cursor = st.editable ? 'grab' : 'pointer';
  });
  Object.keys(st.markers).forEach(function (id) {
    if (!keep[id]) {
      var el = st.markers[id].el;
      if (el.parentNode) el.parentNode.removeChild(el);
      delete st.markers[id];
    }
  });
  return true;
}

function splatSetPlaceMode(hostId, on) {
  var host = hostOf(hostId);
  if (!host) return false;
  var st = host._pnexSplat;
  if (!st) {
    splatPending(host).placeMode = !!on;
    return true;
  }
  st.placeMode = !!on;
  st.canvas.style.cursor = on ? 'crosshair' : '';
  return true;
}

function stopSplat(host) {
  var st = host && host._pnexSplat;
  if (!st) return;
  host._pnexSplat = null;
  if (st.abort) {
    try { st.abort.abort(); } catch (e) {}
  }
  st.listeners.forEach(function (l) {
    l[0].removeEventListener(l[1], l[2], l[3]);
  });
  st.listeners = [];
  if (st.controls) {
    try { st.controls.dispose(); } catch (e) {}
  }
  if (st.renderer && st.rendered) {
    try { st.renderer.dispose(); } catch (e) {}
  }
  if (st.renderer) {
    try {
      var lose = st.renderer.gl.getExtension('WEBGL_lose_context');
      if (lose) lose.loseContext();
    } catch (e) {}
  }
}

// ─────────────────────────────────────────────────────────────────
// Annotation cards that follow their markers (D147). Dioxus renders the
// cards in an overlay that is a SIBLING of the viewer host (unmount wipes
// the host's children); this glue only sets each card's transform /
// visibility. Cards carry `data-annot-card=<item id>`, never `.pnex-annot`.
function placeCards(host, overlayId, anchorOf) {
  var overlay = document.getElementById(overlayId);
  if (!overlay) return;
  var cards = overlay.querySelectorAll('[data-annot-card]');
  if (!cards.length) return;
  var ow = overlay.clientWidth, oh = overlay.clientHeight;
  var anchors = [];
  for (var i = 0; i < cards.length; i++) anchors.push(anchorOf(cards[i].getAttribute('data-annot-card')));
  for (var k = 0; k < cards.length; k++) {
    var card = cards[k], a = anchors[k];
    // Marker outside the view (pannellum only hides hotspots behind the
    // camera): hide its card instead of pinning it to the edge.
    if (a && (a.x < -8 || a.x > ow + 8 || a.y < -8 || a.y > oh + 8)) a = null;
    if (!a) {
      if (card.style.visibility !== 'hidden') card.style.visibility = 'hidden';
      continue;
    }
    var cw = card.offsetWidth, ch = card.offsetHeight;
    var x = Math.max(4, Math.min(ow - cw - 4, a.x - cw / 2));
    var y = Math.max(4, Math.min(oh - ch - 4, a.y - ch - 16));
    var last = card._pnexPos;
    if (!last || Math.abs(last.x - x) > 0.5 || Math.abs(last.y - y) > 0.5) {
      card._pnexPos = { x: x, y: y };
      card.style.transform = 'translate(' + x + 'px,' + y + 'px)';
    }
    if (card.style.visibility !== 'visible') card.style.visibility = 'visible';
    if (!card.dataset.pos) card.dataset.pos = '1';
  }
}

// Panorama: anchor = the pannellum hotspot div of the item, relative to
// the host (hidden when pannellum hides it behind the camera).
function panoCardAnchor(host) {
  var base = host.getBoundingClientRect();
  return function (id) {
    var cfg = host._pnexAnnotCfg && host._pnexAnnotCfg['annot-' + id];
    var div = cfg && cfg.div;
    if (!div || div.style.visibility === 'hidden') return null;
    var r = div.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return null;
    return { x: r.left + r.width / 2 - base.left, y: r.top + r.height / 2 - base.top };
  };
}

function followCards(hostId, overlayId, on) {
  var host = hostOf(hostId);
  if (!host) return false;
  if (host._pnexCardsRaf) {
    cancelAnimationFrame(host._pnexCardsRaf);
    host._pnexCardsRaf = null;
  }
  // Remembered on the host: a splat mounted later picks it up.
  host._pnexCardsOverlay = on ? overlayId : null;
  if (host._pnexSplat) {
    // The splat render loop places the cards from its projected markers.
    host._pnexSplat.cards = on ? overlayId : null;
    return true;
  }
  if (!on) return true;
  var tick = function () {
    if (!host.isConnected) {
      host._pnexCardsRaf = null;
      return;
    }
    if (host._pnexSplat) {
      // A splat mounted meanwhile: hand the cards to its render loop.
      host._pnexSplat.cards = overlayId;
      host._pnexCardsRaf = null;
      return;
    }
    placeCards(host, overlayId, panoCardAnchor(host));
    host._pnexCardsRaf = requestAnimationFrame(tick);
  };
  host._pnexCardsRaf = requestAnimationFrame(tick);
  return true;
}

function unmountHost(hostId) {
  var host = hostOf(hostId);
  if (!host) return;
  // Camera live view: close the socket, stop reconnecting, revoke the URL.
  stopCamera(host);
  // Splat: abort the load, drop listeners, dispose renderer/controls.
  // The cards loop is NOT cancelled here: mountTour unmounts the host
  // before each (re)mount; the loop stops by itself once the host leaves
  // the DOM, or through followCards(…, false).
  stopSplat(host);
  if (host._pnexViewer) {
    try {
      host._pnexViewer.destroy();
    } catch (e) {
      warn('panorama unmount échoué', e);
    }
    host._pnexViewer = null;
  }
  // destroy() de pannellum libère déjà son contexte WebGL, mais seulement
  // s'il est atteint — on force la libération sur le canvas restant (un
  // contexte non libéré compte dans la limite ~16/page et finit par faire
  // échouer les créations suivantes, de façon aléatoire).
  var canvas = host.querySelector('canvas');
  if (canvas) {
    try {
      var gl = canvas.getContext('webgl') || canvas.getContext('webgl2');
      if (gl) {
        var lose = gl.getExtension('WEBGL_lose_context');
        if (lose) lose.loseContext();
      }
    } catch (e) {
      // canvas déjà détaché / contexte perdu : rien à faire
    }
  }
  if (host._pnexRaf) {
    cancelAnimationFrame(host._pnexRaf);
    host._pnexRaf = null;
  }
  // Drag tour + annotations : état par hôte à réinitialiser (le host
  // survit au innerHTML).
  host._pnexDrag = null;
  host._pnexHotspotCfg = {};
  host._pnexHotspotList = [];
  host._pnexAnnotCfg = {};
  host._pnexAnnotList = [];
  host._pnexAnnotDrag = null;
  host._pnexAnnotEditable = false;
  host._pnexTourEditable = false;
  host._pnexAnnotPlaceMode = false;
  host._pnexAnnotSuppressClick = false;
  host.innerHTML = '';
}

window.pnexViewers = {
  panorama: { mount: mountPanorama, unmount: unmountHost },
  splat: {
    mount: mountSplat,
    unmount: unmountHost,
    setAnnotations: splatSetAnnotations,
    setAnnotPlaceMode: splatSetPlaceMode,
  },
  cards: { follow: followCards },
  unmount: unmountHost,
};

// ─────────────────────────────────────────────────────────────────
// Tour (Studio) — pannellum unique partagé avec `panorama` (S12
// studio.md : pas de bundle séparé). Une seule scène montée à la fois
// (S13) : `switch` = destroy + re-mount. Les hotspots sont posés côté
// glue (closure JS synchrone) : le clic pose le global
// `window.__pnexTourLastNav` (chaîne JSON {seq, scene_id, floor_id})
// que Rust poll — pas de closure traversante (marche web + webview).
var tourNavSeq = 0;
var tourMoveSeq = 0;

function mountTour(hostId, sceneJson) {
  var host = hostOf(hostId);
  if (!host || !pannellumLib || !hasWebGL(false)) {
    return false;
  }
  var scene;
  try {
    scene = JSON.parse(sceneJson);
  } catch (e) {
    warn('tour scene JSON illisible', e);
    return false;
  }
  if (!scene || !scene.url) {
    return false;
  }
  try {
    unmountHost(hostId);
    var hotSpots = [];
    var cfgById = {};
    (scene.hotspots || []).forEach(function (h) {
      if (!isFinite(h.yaw) || !isFinite(h.pitch)) return;
      // Flèche PNeX : bleu = walk (même étage), violet = stair — flèche
      // vers le bas (descendre) ou vers le haut (monter) selon le pitch
      // auto calculé côté Rust (±30° si les niveaux diffèrent).
      var css = 'pnex-hotspot pnex-hotspot-walk';
      if (h.kind === 'stair') {
        css = h.pitch < 0
          ? 'pnex-hotspot pnex-hotspot-stair-down'
          : 'pnex-hotspot pnex-hotspot-stair-up';
      }
      var cfg = {
        id: 'hs-' + h.link_id,
        linkId: h.link_id,
        yaw: h.yaw,
        pitch: h.pitch,
        type: 'info',
        cssClass: css,
        text: h.label || '',
        clickHandlerFunc: function () {
          tourNavSeq += 1;
          window.__pnexTourLastNav = JSON.stringify({
            seq: tourNavSeq,
            scene_id: h.target_scene,
            floor_id: h.target_floor,
          });
        },
      };
      hotSpots.push(cfg);
      cfgById[cfg.id] = cfg;
    });
    var viewer = pannellumLib.viewer(host, {
      // Security (SEC-6): link and annotation labels are user content —
      // also covers hotspots added later through addHotSpot.
      escapeHTML: true,
      type: 'equirectangular',
      panorama: scene.url,
      autoLoad: true,
      yaw: scene.yaw || 0,
      pitch: scene.pitch || 0,
      hfov: scene.hfov || 100,
      showControls: true,
      hotSpots: hotSpots,
    });
    host._pnexViewer = viewer;
    host._pnexHotspotCfg = cfgById;
    // Pannellum ne recopie PAS config.id sur le div → mapping config ↔ div
    // par ORDRE de création (identique à l'ordre DOM des .pnex-hotspot).
    host._pnexHotspotList = hotSpots.slice();
    bindTourDrag(hostId);
    return true;
  } catch (e) {
    warn('tour mount échoué', e);
    return false;
  }
}

// ── Drag d'une flèche : repositionner le hotspot (yaw/pitch) ──
// Mousedown en PHASE CAPTURE sur le host : stopPropagation empêche la
// rotation caméra de pannellum pendant qu'on saisit une flèche ; le delta
// pixels → degrés utilise le hfov courant (≈ hfov / largeur canvas).
// Sans mouvement = clic normal (navigation) ; avec mouvement = publish
// `__pnexTourHotspotMove` (chaîne JSON, même école que la nav).

function applyHotspotPos(host, d) {
  var viewer = host && host._pnexViewer;
  if (!viewer || !viewer.removeHotSpot || !viewer.addHotSpot) return;
  try {
    viewer.removeHotSpot(d.id);
    viewer.addHotSpot(d.cfg);
    // removeHotSpot + addHotSpot ré-appende le div EN FIN de DOM : on
    // reflète le nouvel ordre dans la liste miroir.
    var list = host._pnexHotspotList;
    if (list) {
      var i = list.indexOf(d.cfg);
      if (i >= 0) {
        list.splice(i, 1);
        list.push(d.cfg);
      }
    }
  } catch (e) {
    warn('hotspot move échoué', e);
  }
}

function bindTourDrag(hostId) {
  var host = hostOf(hostId);
  // Le host survit aux switches (innerHTML seulement) : une seule liaison.
  if (!host || host._pnexDragBound) return;
  host._pnexDragBound = true;

  host.addEventListener('mousedown', function (e) {
    // Arrow drag is editor-only: the flag is read at event time, so
    // read-only viewers (map POI preview, share page) never drag.
    if (!host._pnexTourEditable || e.button !== 0) return;
    var el = e.target && e.target.closest ? e.target.closest('.pnex-hotspot') : null;
    if (!el) return;
    // Pannellum ne pose pas config.id sur le div : on résout la config par
    // l'ORDRE du div parmi les .pnex-hotspot (miroir maintenu à jour).
    var divs = host.querySelectorAll('.pnex-hotspot');
    var idx = Array.prototype.indexOf.call(divs, el);
    var cfg = host._pnexHotspotList ? host._pnexHotspotList[idx] : null;
    if (!cfg) return;
    e.stopPropagation();
    host._pnexSuppressClick = false;
    var canvas = host.querySelector('canvas');
    var viewer = host._pnexViewer;
    host._pnexDrag = {
      id: cfg.id,
      cfg: cfg,
      yaw0: cfg.yaw,
      pitch0: cfg.pitch,
      yaw: cfg.yaw,
      pitch: cfg.pitch,
      startX: e.clientX,
      startY: e.clientY,
      hfov: viewer && viewer.getHfov ? viewer.getHfov() : 90,
      w: canvas ? canvas.clientWidth : 800,
      h: canvas ? canvas.clientHeight : 450,
      moved: false,
      lastApply: 0,
    };
  }, true);

  document.addEventListener('mousemove', function (e) {
    var d = host._pnexDrag;
    if (!d) return;
    var dx = e.clientX - d.startX;
    var dy = e.clientY - d.startY;
    if (!d.moved && Math.abs(dx) + Math.abs(dy) < 4) return;
    d.moved = true;
    d.yaw = d.yaw0 + dx * (d.hfov / d.w);
    d.pitch = Math.max(-89, Math.min(89, d.pitch0 - dy * (d.hfov / d.h)));
    var now = Date.now();
    if (now - d.lastApply > 60) {
      d.lastApply = now;
      d.cfg.yaw = d.yaw;
      d.cfg.pitch = d.pitch;
      applyHotspotPos(host, d);
    }
  });

  document.addEventListener('mouseup', function (e) {
    var d = host._pnexDrag;
    if (!d) return;
    host._pnexDrag = null;
    if (!d.moved) return; // clic simple : le clickHandlerFunc navigue
    e.stopPropagation();
    d.cfg.yaw = d.yaw;
    d.cfg.pitch = d.pitch;
    applyHotspotPos(host, d);
    host._pnexSuppressClick = true;
    tourMoveSeq += 1;
    window.__pnexTourHotspotMove = JSON.stringify({
      seq: tourMoveSeq,
      link_id: d.cfg.linkId,
      yaw: d.yaw,
      pitch: d.pitch,
    });
  });

  // Un drag ne doit pas naviguer : on avale le click qui suit le mouseup.
  host.addEventListener('click', function (e) {
    if (!host._pnexSuppressClick) return;
    if (e.target && e.target.closest && e.target.closest('.pnex-hotspot')) {
      e.stopPropagation();
      e.preventDefault();
      host._pnexSuppressClick = false;
    }
  }, true);
}

function switchTour(hostId, sceneJson) {
  // S13 : destroy + re-mount — une seule texture en mémoire.
  return mountTour(hostId, sceneJson);
}

function takeTourNav() {
  var raw = typeof window !== 'undefined' ? window.__pnexTourLastNav : null;
  if (!raw) return null;
  try {
    return JSON.parse(raw);
  } catch (e) {
    return null;
  }
}

// ─────────────────────────────────────────────────────────────────
// Annotations sur médias (D55–D60) — overlay non intrusif. Classes
// STRICTEMENT disjointes de .pnex-hotspot : le drag nav résout ses
// hotspots par miroir index (_pnexHotspotList) — un marqueur annot
// qui porterait .pnex-hotspot serait capturé et désalignerait la
// liste. Événements → Rust : globals CHAÎNE JSON brute + seq monotone
// (école __pnexTourLastNav — jamais l'objet parsé, bug take_nav
// 2026-09-12) : __pnexAnnotClick (clic), __pnexAnnotMove (drag
// d'ajustement, mode éditeur), __pnexAnnotPlace (clic de pose, mode
// éditeur). Pose/retrait post-mount UNIQUEMENT (removeHotSpot/
// addHotSpot — jamais de re-mount : flicker + rechargement texture).
var annotSeq = 0;
var annotMoveSeq = 0;
var annotPlaceSeq = 0;

function applyAnnotPos(host, d) {
  var viewer = host && host._pnexViewer;
  if (!viewer || !viewer.removeHotSpot || !viewer.addHotSpot) return;
  try {
    viewer.removeHotSpot(d.id);
    viewer.addHotSpot(d.cfg);
    // remove+add ré-appende le div EN FIN de DOM : refléter le nouvel
    // ordre dans le miroir (résolution div → cfg par index).
    var list = host._pnexAnnotList;
    if (list) {
      var i = list.indexOf(d.cfg);
      if (i >= 0) {
        list.splice(i, 1);
        list.push(d.cfg);
      }
    }
  } catch (e) {
    warn('annot move échoué', e);
  }
}

function setAnnotations(hostId, itemsJson, editable) {
  var host = hostOf(hostId);
  if (!host) return false;
  var viewer = host._pnexViewer;
  if (!viewer || !viewer.addHotSpot || !viewer.removeHotSpot) return false;
  var items;
  try {
    items = JSON.parse(itemsJson);
  } catch (e) {
    warn('annot JSON illisible', e);
    return false;
  }
  if (!Array.isArray(items)) return false;
  host._pnexAnnotEditable = !!editable;
  if (!host._pnexAnnotCfg) host._pnexAnnotCfg = {};
  var keep = {};
  var list = [];
  items.forEach(function (it) {
    if (!it || !it.id || !isFinite(it.yaw) || !isFinite(it.pitch)) return;
    var id = 'annot-' + it.id;
    keep[id] = true;
    var cfg = {
      id: id,
      itemId: it.id,
      yaw: it.yaw,
      pitch: it.pitch,
      type: 'info',
      cssClass: 'pnex-annot pnex-annot-' + (it.kind || 'note'),
      text: it.label || '',
      clickHandlerFunc: function () {
        annotSeq += 1;
        window.__pnexAnnotClick = JSON.stringify({
          seq: annotSeq,
          item_id: it.id,
        });
      },
    };
    list.push(cfg);
    if (host._pnexAnnotCfg[id]) {
      try {
        viewer.removeHotSpot(id);
      } catch (e) {}
    }
    try {
      viewer.addHotSpot(cfg);
      host._pnexAnnotCfg[id] = cfg;
    } catch (e) {
      warn('annot add échoué', e);
    }
  });
  host._pnexAnnotList = list;
  // Retrait des marqueurs absents du lot (changement de scène, toggle).
  Object.keys(host._pnexAnnotCfg).forEach(function (id) {
    if (!keep[id]) {
      try {
        viewer.removeHotSpot(id);
      } catch (e) {}
      delete host._pnexAnnotCfg[id];
    }
  });
  bindAnnotDrag(hostId);
  return true;
}

// ── Drag d'un marqueur (mode éditeur) : delta px → deg hfov/w, école
// bindTourDrag. Lecture seule : _pnexAnnotEditable false → inert. ──

function bindAnnotDrag(hostId) {
  var host = hostOf(hostId);
  if (!host || host._pnexAnnotDragBound) return;
  host._pnexAnnotDragBound = true;

  host.addEventListener('mousedown', function (e) {
    if (!host._pnexAnnotEditable || e.button !== 0) return;
    var el = e.target && e.target.closest ? e.target.closest('.pnex-annot') : null;
    if (!el) return;
    var divs = host.querySelectorAll('.pnex-annot');
    var idx = Array.prototype.indexOf.call(divs, el);
    var cfg = host._pnexAnnotList ? host._pnexAnnotList[idx] : null;
    if (!cfg) return;
    e.stopPropagation();
    host._pnexAnnotSuppressClick = false;
    var canvas = host.querySelector('canvas');
    var viewer = host._pnexViewer;
    host._pnexAnnotDrag = {
      id: cfg.id,
      cfg: cfg,
      yaw0: cfg.yaw,
      pitch0: cfg.pitch,
      yaw: cfg.yaw,
      pitch: cfg.pitch,
      startX: e.clientX,
      startY: e.clientY,
      hfov: viewer && viewer.getHfov ? viewer.getHfov() : 90,
      w: canvas ? canvas.clientWidth : 800,
      h: canvas ? canvas.clientHeight : 450,
      moved: false,
      lastApply: 0,
    };
  }, true);

  document.addEventListener('mousemove', function (e) {
    var d = host._pnexAnnotDrag;
    if (!d) return;
    var dx = e.clientX - d.startX;
    var dy = e.clientY - d.startY;
    if (!d.moved && Math.abs(dx) + Math.abs(dy) < 4) return;
    d.moved = true;
    d.yaw = d.yaw0 + dx * (d.hfov / d.w);
    d.pitch = Math.max(-90, Math.min(90, d.pitch0 - dy * (d.hfov / d.h)));
    var now = Date.now();
    if (now - d.lastApply > 60) {
      d.lastApply = now;
      d.cfg.yaw = d.yaw;
      d.cfg.pitch = d.pitch;
      applyAnnotPos(host, d);
    }
  });

  document.addEventListener('mouseup', function (e) {
    var d = host._pnexAnnotDrag;
    if (!d) return;
    host._pnexAnnotDrag = null;
    if (!d.moved) return; // clic simple : le clickHandlerFunc publie
    e.stopPropagation();
    d.cfg.yaw = d.yaw;
    d.cfg.pitch = d.pitch;
    applyAnnotPos(host, d);
    host._pnexAnnotSuppressClick = true;
    annotMoveSeq += 1;
    window.__pnexAnnotMove = JSON.stringify({
      seq: annotMoveSeq,
      item_id: d.cfg.itemId,
      yaw: d.yaw,
      pitch: d.pitch,
    });
  });

  // Un drag ne doit pas déclencher le clickHandlerFunc : on avale le
  // click qui suit le mouseup (phase capture, école bindTourDrag).
  host.addEventListener('click', function (e) {
    if (!host._pnexAnnotSuppressClick) return;
    if (e.target && e.target.closest && e.target.closest('.pnex-annot')) {
      e.stopPropagation();
      e.preventDefault();
      host._pnexAnnotSuppressClick = false;
    }
  }, true);
}

// ── Mode pose (éditeur) : clic sur le pano hors marqueur → coords
// sphère. mouseEventToCoords = [pitch, yaw] (pannellum 2.5.7,
// synchrone) ; guard < 4 px depuis mousedown pour ne pas confondre
// avec un drag caméra. ──

// Arrow-drag gate for the tour viewer: true only in the tour editor
// (Studio preview) — read-only viewers keep markers fixed. Read by the
// bindTourDrag guard at event time, safe to set before or after mount.
function setTourEditable(hostId, on) {
  var host = hostOf(hostId);
  if (!host) return false;
  host._pnexTourEditable = !!on;
  return true;
}

function setAnnotPlaceMode(hostId, on) {
  var host = hostOf(hostId);
  if (!host) return false;
  host._pnexAnnotPlaceMode = !!on;
  if (on && !host._pnexAnnotPlaceBound) {
    host._pnexAnnotPlaceBound = true;
    host.addEventListener('mousedown', function (e) {
      host._pnexAnnotDown = { x: e.clientX, y: e.clientY };
    }, true);
    host.addEventListener('click', function (e) {
      if (!host._pnexAnnotPlaceMode) return;
      if (e.target && e.target.closest &&
          (e.target.closest('.pnex-annot') || e.target.closest('.pnex-hotspot'))) return;
      var down = host._pnexAnnotDown;
      if (down && Math.abs(e.clientX - down.x) + Math.abs(e.clientY - down.y) >= 4) return;
      var viewer = host._pnexViewer;
      if (!viewer || !viewer.mouseEventToCoords) return;
      var coords;
      try {
        coords = viewer.mouseEventToCoords(e);
      } catch (err) {
        return;
      }
      if (!coords || !isFinite(coords[0]) || !isFinite(coords[1])) return;
      annotPlaceSeq += 1;
      window.__pnexAnnotPlace = JSON.stringify({
        seq: annotPlaceSeq,
        yaw: coords[1],
        pitch: coords[0],
      });
    });
  }
  return true;
}

window.pnexViewers.tour = {
  mount: mountTour,
  // « switch » est un mot réservé JS : `tour.switch(...)` (injection natif)
  // est une erreur de syntaxe — nom typé `switchScene`.
  switchScene: switchTour,
  nav: takeTourNav,
  setAnnotations: setAnnotations,
  setAnnotPlaceMode: setAnnotPlaceMode,
  setTourEditable: setTourEditable,
  unmount: unmountHost,
};

// ─────────────────────────────────────────────────────────────────
// Camera live view (camera-video.md D75) — WebSocket of raw JPEG binary
// frames (`/ws/camera/live`), each frame becomes a blob URL on an <img>
// (the previous URL is revoked). Auto-reconnect with a 1 → 10 s backoff
// while mounted. No polling, no storage. Works on web and in the Android
// webview (same eval bridge as the other viewers).
function stopCamera(host) {
  var cam = host._pnexCam;
  if (!cam) return;
  host._pnexCam = null;
  cam.stopped = true;
  if (cam.timer) clearTimeout(cam.timer);
  if (cam.ws) {
    try {
      cam.ws.onclose = null;
      cam.ws.close();
    } catch (e) {
      // socket already closed
    }
  }
  if (cam.url) {
    try {
      URL.revokeObjectURL(cam.url);
    } catch (e) {
      // nothing to revoke
    }
  }
}

function mountCamera(hostId, wsUrl) {
  var host = hostOf(hostId);
  if (!host || typeof WebSocket === 'undefined') return false;
  stopCamera(host);
  host.innerHTML = '';
  var img = document.createElement('img');
  img.style.width = '100%';
  img.style.height = '100%';
  img.style.objectFit = 'contain';
  img.alt = '';
  host.appendChild(img);
  // `wsUrl` is refreshed from Rust (setUrl) so a reconnect after the access
  // token expired uses the current token, not the one captured at mount.
  var cam = { ws: null, url: null, timer: null, backoff: 1000, stopped: false, frames: 0, wsUrl: wsUrl };
  host._pnexCam = cam;

  function connect() {
    if (cam.stopped) return;
    var ws;
    try {
      ws = new WebSocket(cam.wsUrl);
    } catch (e) {
      warn('camera socket failed', e);
      schedule();
      return;
    }
    ws.binaryType = 'arraybuffer';
    cam.ws = ws;
    ws.onopen = function () {
      cam.backoff = 1000;
    };
    ws.onmessage = function (ev) {
      if (cam.stopped || typeof ev.data === 'string') return;
      var url = URL.createObjectURL(new Blob([ev.data], { type: 'image/jpeg' }));
      var previous = cam.url;
      cam.url = url;
      cam.frames += 1;
      host.dataset.pnexFrames = String(cam.frames);
      img.src = url;
      if (previous) URL.revokeObjectURL(previous);
    };
    ws.onclose = function () {
      cam.ws = null;
      schedule();
    };
    ws.onerror = function () {
      // onclose follows and schedules the reconnect
    };
  }

  function schedule() {
    if (cam.stopped) return;
    var delay = cam.backoff;
    cam.backoff = Math.min(cam.backoff * 2, 10000);
    cam.timer = setTimeout(connect, delay);
  }

  connect();
  return true;
}

// Updates the URL used by the next (re)connection of a mounted camera.
function setCameraUrl(hostId, wsUrl) {
  var host = hostOf(hostId);
  if (!host || !host._pnexCam) return false;
  host._pnexCam.wsUrl = wsUrl;
  return true;
}

window.pnexViewers.camera = { mount: mountCamera, unmount: unmountHost, setUrl: setCameraUrl };
