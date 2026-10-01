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
// Même souci de forme pour gsplat (ESM pur) : `Renderer`/`Scene` doivent
// exister — sinon retomber sur la globale éventuelle.
if (!gsplatLib || typeof gsplatLib.Renderer !== 'function') {
  gsplatLib = (typeof window !== 'undefined' && typeof window.gsplat?.Renderer === 'function')
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

function mountSplat(hostId, url) {
  var host = hostOf(hostId);
  if (!host || !gsplatLib || !hasWebGL(true)) {
    return false;
  }
  try {
    host.innerHTML = '';
    var canvas = document.createElement('canvas');
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    host.appendChild(canvas);
    (async function () {
      try {
        var resp = await fetch(url);
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        var buffer = await resp.arrayBuffer();
        var renderer = new gsplatLib.Renderer(canvas);
        var scene = new gsplatLib.Scene();
        var data = await gsplatLib.SceneFormat.fromArrayBuffer(buffer);
        scene.setData(data);
        renderer.addScene(scene);
        renderer.render();
      } catch (e) {
        warn('splat load échoué', e);
      }
    })();
    return true;
  } catch (e) {
    warn('splat mount échoué', e);
    return false;
  }
}

function unmountHost(hostId) {
  var host = hostOf(hostId);
  if (!host) return;
  // Camera live view: close the socket, stop reconnecting, revoke the URL.
  stopCamera(host);
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
  splat: { mount: mountSplat, unmount: unmountHost },
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
