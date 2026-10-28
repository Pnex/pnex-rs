//! POI-first geo map (D27 + D35–D37) — esbuild IIFE bundle of
//! maplibre-gl 6.x, mounted on `window.pnexMap` (viewers.js / flasher.js
//! pattern). Loaded by `main.rs`.
//!
//! The basemap style is the org's default geo provider (geo-layers.md
//! L24–L26), chosen by the page; empty = plain background.
//!
//! Items are **HTML markers** (not symbol layers): maplibre text is
//! rendered as monochrome SDF — a colour emoji does not survive it; a DOM
//! element shows it natively and receives the click. Three shapes:
//! - pin      → emoji + label (POI);
//! - cluster  → blue circle with the NUMBER (backend clustering D37);
//! - position → live GPS device (filtered layers).
//!
//! Events to Rust (polled on the bridge side, tron.rs pattern: no closure
//! crossing over, works on wasm + native via document::eval):
//! - item/map click: `window.__pnexMapLastClick = {seq, host, kind, id,
//!   label, lng, lat}` — `kind` ∈ `pin|cluster|position|map` (`map` = click
//!   on the bare map, coords filled in);
//! - viewport: `window.__pnexMapView = {seq, west, south, east, north,
//!   zoom}` on `moveend` (300 ms debounce) → page-side cluster refetch.
//!
//! Silent failures: missing script, host not found, no WebGL, style
//! error → `false` + `error(hostId)` set — the page shows the
//! "map unavailable" badge, never a panic.
// maplibre-gl 6.x is ESM-only (no CJS entry): the old require() here was
// left as a runtime __require stub by esbuild (IIFE) and threw in the
// browser — the static import below is the only reliable source.
import * as maplibreGl from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
// v6 ships its worker as a sibling module resolved against import.meta.url
// (`new URL('./maplibre-gl-worker.mjs', …)`), which an IIFE bundle cannot
// rewrite — at runtime the fetch 404s and the map mounts with a dead worker:
// no tiles, blank canvas, no error event. js:map bundles the worker into a
// single classic script (assets/.map-worker.txt, inlined as text here) and we
// hand it back to the lib as a blob URL before any Map is created.
import workerSource from '../assets/.map-worker.txt';
import { cut } from './track_cut.js';
maplibreGl.setWorkerUrl(
  URL.createObjectURL(new Blob([workerSource], { type: 'text/javascript' }))
);

(function () {
  'use strict';

  var VIEW_DEBOUNCE_MS = 300;
  // Track layers (cases-workspace.md K28): whole route dashed, then the
  // past up to the cursor in two opacity slices by age.
  var TRACK_LAYERS = [
    { id: 'pnex-trk-full', opacity: 0.35, width: 2, dash: [2, 2] },
    { id: 'pnex-trk-older', opacity: 0.55, width: 3 },
    { id: 'pnex-trk-recent', opacity: 0.95, width: 4 },
  ];

  function lines(parts) {
    return {
      type: 'FeatureCollection',
      features: parts.map(function (p) {
        return { type: 'Feature', properties: { color: p.color }, geometry: { type: 'LineString', coordinates: p.coords } };
      }),
    };
  }

  function ensureTrackLayers(map) {
    TRACK_LAYERS.forEach(function (l) {
      if (map.getSource(l.id)) return;
      map.addSource(l.id, { type: 'geojson', data: lines([]) });
      var paint = { 'line-color': ['get', 'color'], 'line-opacity': l.opacity, 'line-width': l.width };
      if (l.dash) paint['line-dasharray'] = l.dash;
      map.addLayer({ id: l.id, type: 'line', source: l.id, layout: { 'line-cap': 'round', 'line-join': 'round' }, paint: paint });
    });
  }

  // Framed on the real container size: on the tracks and areas, until a
  // track has been framed once in this mount.
  function fit(host) {
    var map = host._pnexMap;
    if (!map || host._pnexTracksFitted) return;
    var b = new (maplibreMod().LngLatBounds)();
    var tracks = false;
    ((host._pnexTracks && host._pnexTracks.tracks) || []).forEach(function (tr) {
      (tr.segments || []).forEach(function (seg) {
        seg.forEach(function (p) { b.extend([p[0], p[1]]); tracks = true; });
      });
    });
    ((host._pnexZones && host._pnexZones.features) || []).forEach(function (f) {
      f.geometry.coordinates[0].forEach(function (c) { b.extend(c); });
    });
    if (b.isEmpty()) return;
    map.fitBounds(b, { padding: 32, maxZoom: 16, animate: false });
    host._pnexTracksFitted = tracks;
  }

  // Areas of the case (K29): light fill, outlined.
  function drawZones(host) {
    var map = host._pnexMap;
    if (!map || !host._pnexZones || !map.isStyleLoaded()) return;
    if (!map.getSource('pnex-zones')) {
      map.addSource('pnex-zones', { type: 'geojson', data: host._pnexZones });
      var before = map.getLayer('pnex-trk-full') ? 'pnex-trk-full' : undefined;
      map.addLayer({ id: 'pnex-zones-fill', type: 'fill', source: 'pnex-zones', paint: { 'fill-color': '#0b7285', 'fill-opacity': 0.12 } }, before);
      map.addLayer({ id: 'pnex-zones-line', type: 'line', source: 'pnex-zones', paint: { 'line-color': '#0b7285', 'line-width': 1.5 } }, before);
    } else {
      map.getSource('pnex-zones').setData(host._pnexZones);
    }
    fit(host);
  }

  function drawTracks(host) {
    var map = host._pnexMap;
    var data = host._pnexTracks;
    if (!map || !data || !map.isStyleLoaded()) return;
    ensureTrackLayers(map);
    var t = typeof host._pnexTime === 'number' ? host._pnexTime : Infinity;
    var full = [];
    var older = [];
    var recent = [];
    (data.tracks || []).forEach(function (tr) {
      (tr.segments || []).forEach(function (seg) {
        if (seg.length > 1) full.push({ color: tr.color, coords: seg.map(function (p) { return [p[0], p[1]]; }) });
      });
      var c = cut(tr.segments, t, data.recentMs || 0);
      c.older.forEach(function (coords) { older.push({ color: tr.color, coords: coords }); });
      c.recent.forEach(function (coords) { recent.push({ color: tr.color, coords: coords }); });
    });
    if (!host._pnexTracksDrawn) {
      map.getSource('pnex-trk-full').setData(lines(full));
      host._pnexTracksDrawn = true;
      fit(host);
    }
    map.getSource('pnex-trk-older').setData(lines(older));
    map.getSource('pnex-trk-recent').setData(lines(recent));
  }

  function maplibreMod() {
    // Static ESM import first (bundled by esbuild); window.maplibregl
    // stays as a fallback for a global-script setup.
    if (maplibreGl && maplibreGl.Map) return maplibreGl;
    if (typeof window !== 'undefined' && window.maplibregl && window.maplibregl.Map) {
      return window.maplibregl;
    }
    return null;
  }

  function webglAvailable() {
    try {
      var c = document.createElement('canvas');
      return !!(c.getContext('webgl2') || c.getContext('webgl'));
    } catch (e) {
      return false;
    }
  }

  function hostOf(hostId) {
    return document.getElementById(hostId);
  }

  function setErr(host, code) {
    if (host) host._pnexMapError = code;
  }

  function emitClick(kind, id, label, lng, lat) {
    window.__pnexMapSeq = (window.__pnexMapSeq || 0) + 1;
    // JSON string: read uniformly by the wasm bridge (as_string) and
    // the native bridge (document::eval → as_string).
    window.__pnexMapLastClick = JSON.stringify({
      seq: window.__pnexMapSeq,
      kind: kind,
      id: id || null,
      label: label || '',
      lng: typeof lng === 'number' ? lng : null,
      lat: typeof lat === 'number' ? lat : null,
    });
  }

  // Palette of property colours (pnex_core::ontology::schema::COLORS).
  var PIN_TONES = {
    green: '#22c55e', amber: '#f59e0b', red: '#ef4444',
    blue: '#3b82f6', violet: '#8b5cf6', gray: '#9ca3af',
  };

  function buildPinElement(p) {
    var el = document.createElement('div');
    el.style.cssText =
      'display:flex;flex-direction:column;align-items:center;cursor:pointer;' +
      'pointer-events:auto;transform:translate(-50%,-100%);';
    var dot = document.createElement('div');
    dot.textContent = p.emoji || '📍';
    dot.style.cssText =
      'font-size:22px;line-height:1;filter:drop-shadow(0 1px 2px rgba(15,23,42,.45));';
    var lab = document.createElement('div');
    lab.textContent = p.label || '';
    lab.style.cssText =
      'font-size:11px;font-weight:600;color:#1e293b;background:rgba(255,255,255,.9);' +
      'border-radius:4px;padding:0 4px;margin-top:1px;max-width:140px;' +
      'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
    // Status colour (D339): a ring under the pin and the label's edge.
    var tone = PIN_TONES[p.color];
    if (tone) {
      dot.style.cssText +=
        'border-radius:9999px;padding:2px;box-shadow:0 0 0 3px ' + tone + ';background:#fff;';
      lab.style.borderLeft = '3px solid ' + tone;
    }
    el.appendChild(dot);
    if (p.label) el.appendChild(lab);
    return el;
  }

  function buildClusterElement(count) {
    var el = document.createElement('div');
    el.textContent = String(count);
    el.style.cssText =
      'display:flex;align-items:center;justify-content:center;cursor:pointer;' +
      'pointer-events:auto;width:34px;height:34px;border-radius:9999px;' +
      'background:rgba(37,99,235,.92);color:#fff;font-size:13px;font-weight:700;' +
      'border:2px solid rgba(255,255,255,.9);box-shadow:0 1px 4px rgba(15,23,42,.4);' +
      'transform:translate(-50%,-50%);font-family:system-ui,sans-serif;';
    return el;
  }

  function buildPositionElement(p) {
    var el = document.createElement('div');
    el.style.cssText =
      'display:flex;flex-direction:column;align-items:center;cursor:pointer;' +
      'pointer-events:auto;transform:translate(-50%,-100%);';
    var dot = document.createElement('div');
    dot.textContent = p.emoji || '🛰️';
    dot.style.cssText =
      'font-size:20px;line-height:1;filter:drop-shadow(0 0 3px rgba(37,99,235,.8));';
    var lab = document.createElement('div');
    lab.textContent = p.label || '';
    lab.style.cssText =
      'font-size:10px;font-weight:600;color:#1d4ed8;background:rgba(255,255,255,.92);' +
      'border-radius:4px;padding:0 4px;margin-top:1px;max-width:120px;' +
      'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
    // Status colour (D339): a ring under the pin and the label's edge.
    var tone = PIN_TONES[p.color];
    if (tone) {
      dot.style.cssText +=
        'border-radius:9999px;padding:2px;box-shadow:0 0 0 3px ' + tone + ';background:#fff;';
      lab.style.borderLeft = '3px solid ' + tone;
    }
    el.appendChild(dot);
    if (p.label) el.appendChild(lab);
    return el;
  }

  var api = {
    /**
     * Mounts a map in the host div.
     * opts: { styleUrl, center: [lon, lat], zoom }
     * → true if mounted.
     */
    mount: function (hostId, opts) {
      // The wasm bridge passes opts as a JSON STRING (native document::eval
      // already parses it) — accept both forms.
      if (typeof opts === 'string') {
        try { opts = JSON.parse(opts); } catch (e) { return false; }
      }
      var host = hostOf(hostId);
      if (!host) return false;
      var ml = maplibreMod();
      if (!ml) { setErr(host, 'lib'); return false; }
      if (!webglAvailable()) { setErr(host, 'webgl'); return false; }
      try {
        api.unmount(hostId);
        // The dark variant of the basemap follows the app theme (data-theme).
        var dark = document.documentElement.getAttribute('data-theme') === 'dark';
        var map = new ml.Map({
          container: host,
          // No basemap configured (L26): plain background, POIs still usable.
          style: (dark && opts.styleUrlDark) || opts.styleUrl || {
            version: 8, sources: {},
            layers: [{ id: 'bg', type: 'background', paint: { 'background-color': dark ? '#0f161d' : '#e9ecef' } }],
          },
          center: opts.center || [2.35, 48.85],
          zoom: typeof opts.zoom === 'number' ? opts.zoom : 4,
          attributionControl: { compact: true },
        });
        host._pnexMap = map;
        host._pnexMapMarkers = [];
        host._pnexTracksFitted = false;
        // Sequences survive a remount (basemap switch): the page polls for
        // a seq greater than the last one it saw.
        host._pnexMapClickSeq = host._pnexMapClickSeq || 0;
        host._pnexMapViewSeq = host._pnexMapViewSeq || 0;
        map.on('error', function (e) {
          setErr(host, (e && e.error && e.error.message) || 'map');
        });
        // Click on the bare map (outside markers — markers stop propagation)
        // : page-side POI add mode.
        map.on('click', function (e) {
          emitClick('map', null, '', e.lngLat.lng, e.lngLat.lat);
        });
        // Viewport → cluster refetch (debounce: no more moveend bursts).
        var viewTimer = null;
        var pushView = function () {
          var c = map.getCenter();
          var b = map.getBounds();
          host._pnexMapViewSeq = (host._pnexMapViewSeq || 0) + 1;
          window.__pnexMapView = JSON.stringify({
            seq: host._pnexMapViewSeq,
            host: hostId,
            west: b.getWest(), south: b.getSouth(),
            east: b.getEast(), north: b.getNorth(),
            zoom: map.getZoom(),
            center: [c.lng, c.lat],
          });
        };
        map.on('moveend', function () {
          if (viewTimer) clearTimeout(viewTimer);
          viewTimer = setTimeout(pushView, VIEW_DEBOUNCE_MS);
        });
        map.on('load', function () {
          pushView();
          if (opts.items) api.setItems(hostId, opts.items);
          host._pnexTracksDrawn = false;
          drawZones(host);
          drawTracks(host);
        });
        return true;
      } catch (e) {
        setErr(host, String(e));
        try { if (host._pnexMap) host._pnexMap.remove(); } catch (err) { /* already dead */ }
        host._pnexMap = null;
        return false;
      }
    },

    /**
     * Replaces all markers (POI pins, numbered clusters, live GPS
     * positions). item: {kind:'pin'|'cluster'|'position', lon, lat, label,
     * emoji, count, id}.
     */
    setItems: function (hostId, items) {
      // The wasm bridge passes items as a JSON STRING (native document::eval
      // already parses it) — accept both forms (same trap as mount).
      if (typeof items === 'string') {
        try { items = JSON.parse(items); } catch (e) { return false; }
      }
      var host = hostOf(hostId);
      var map = host && host._pnexMap;
      var ml = maplibreMod();
      if (!host || !map || !ml) return false;
      (host._pnexMapMarkers || []).forEach(function (m) {
        try { m.remove(); } catch (e) { /* already removed */ }
      });
      host._pnexMapMarkers = [];
      (items || []).forEach(function (p) {
        if (typeof p.lon !== 'number' || typeof p.lat !== 'number') return;
        var kind = p.kind || 'pin';
        var el;
        if (kind === 'cluster') {
          el = buildClusterElement(p.count || 0);
        } else if (kind === 'position') {
          el = buildPositionElement(p);
        } else {
          el = buildPinElement(p);
        }
        el.addEventListener('click', function (ev) {
          ev.stopPropagation();
          emitClick(kind, p.id, p.label, p.lon, p.lat);
        });
        try {
          var anchor = kind === 'cluster' ? 'center' : 'bottom';
          var marker = new ml.Marker({ element: el, anchor: anchor })
            .setLngLat([p.lon, p.lat])
            .addTo(map);
          host._pnexMapMarkers.push(marker);
        } catch (e) { /* invalid lng/lat: next item */ }
      });
      return true;
    },

    /** Replaces the markers (Phase A compat: = setItems in pin mode). */
    setPins: function (hostId, pins) {
      var items = (pins || []).map(function (p) {
        return Object.assign({ kind: 'pin' }, p);
      });
      return api.setItems(hostId, items);
    },

    /**
     * Tracks of a window, sent once: {recentMs, tracks: [{id, color,
     * segments: [[[lon, lat, t_ms], ...], ...]}]}. Survives a remount.
     */
    setTracks: function (hostId, data) {
      if (typeof data === 'string') {
        try { data = JSON.parse(data); } catch (e) { return false; }
      }
      var host = hostOf(hostId);
      if (!host) return false;
      host._pnexTracks = data;
      host._pnexTracksDrawn = false;
      drawTracks(host);
      return true;
    },

    /** Areas as a GeoJSON FeatureCollection of polygons. Survives a remount. */
    setZones: function (hostId, data) {
      if (typeof data === 'string') {
        try { data = JSON.parse(data); } catch (e) { return false; }
      }
      var host = hostOf(hostId);
      if (!host) return false;
      host._pnexZones = data;
      drawZones(host);
      return true;
    },

    /** Cursor instant (ms): only the past of each track is redrawn. */
    setTime: function (hostId, t) {
      var host = hostOf(hostId);
      if (!host) return false;
      host._pnexTime = t;
      drawTracks(host);
      return true;
    },

    /** Centres the map (smoothly) on a point. */
    flyTo: function (hostId, lon, lat, zoom) {
      var host = hostOf(hostId);
      var map = host && host._pnexMap;
      if (!map) return false;
      map.flyTo({
        center: [lon, lat],
        zoom: typeof zoom === 'number' ? zoom : map.getZoom(),
        duration: 600,
      });
      return true;
    },

    /** Last item/map click (polled by the Rust bridge). */
    lastClick: function () {
      return window.__pnexMapLastClick || null;
    },

    /** Last viewport (debounced moveend, polled by the Rust bridge). */
    lastView: function () {
      return window.__pnexMapView || null;
    },

    /** Error code of the last mount (lib|webgl|map|…) or null. */
    error: function (hostId) {
      var host = hostOf(hostId);
      return (host && host._pnexMapError) || null;
    },

    unmount: function (hostId) {
      var host = hostOf(hostId);
      if (host && host._pnexMap) {
        try { host._pnexMap.remove(); } catch (e) { /* already dead */ }
        host._pnexMap = null;
        host._pnexMapMarkers = [];
      }
      if (host) host._pnexMapError = null;
      return true;
    },
  };

  window.pnexMap = api;
})();
