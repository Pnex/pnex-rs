//! Carte géo POI-first (D27 + D35–D37) — bundle esbuild IIFE de
//! maplibre-gl 6.x, mounted on `window.pnexMap` (viewers.js / flasher.js
//! pattern). Loaded by `main.rs`.
//!
//! Le style de tuiles est le style Protomaps auto-hébergé fourni par
//! l'utilisateur (`map.alpine-box.com/style/light-en` — vectoriel, glyphes
//! sans suffixe .pbf, CORS réfléchi).
//!
//! Les items sont des **markers HTML** (pas des layers symbol) : le texte
//! maplibre est rendu en SDF monochrome — un emoji couleur n'y passe pas ;
//! un élément DOM l'affiche nativement et reçoit le clic. Trois formes :
//! - pin      → emoji + label (POI) ;
//! - cluster  → rond bleu avec le NUMÉRO (clustering backend D37) ;
//! - position → device GPS live (couches filtrées).
//!
//! Événements vers Rust (poll côté pont, école tron.rs : pas de closure
//! traversante, marche wasm + natif via document::eval) :
//! - clic item/carte : `window.__pnexMapLastClick = {seq, host, kind, id,
//!   label, lng, lat}` — `kind` ∈ `pin|cluster|position|map` (`map` = clic
//!   carte nue, coords renseignées) ;
//! - viewport : `window.__pnexMapView = {seq, west, south, east, north,
//!   zoom}` sur `moveend` (debounce 300 ms) → refetch du cluster côté page.
//!
//! Échecs silencieux : script absent, hôte introuvable, WebGL manquant,
//! erreur du style → `false` + `error(hostId)` renseigné — la page affiche
//! le badge « carte indisponible », jamais de panic.
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
maplibreGl.setWorkerUrl(
  URL.createObjectURL(new Blob([workerSource], { type: 'text/javascript' }))
);

(function () {
  'use strict';

  var VIEW_DEBOUNCE_MS = 300;

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
    // Chaîne JSON : lue uniformément par le pont wasm (as_string) et
    // le pont natif (document::eval → as_string).
    window.__pnexMapLastClick = JSON.stringify({
      seq: window.__pnexMapSeq,
      kind: kind,
      id: id || null,
      label: label || '',
      lng: typeof lng === 'number' ? lng : null,
      lat: typeof lat === 'number' ? lat : null,
    });
  }

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
    el.appendChild(dot);
    if (p.label) el.appendChild(lab);
    return el;
  }

  var api = {
    /**
     * Monte une carte dans le div hôte.
     * opts: { styleUrl, center: [lon, lat], zoom }
     * → true si monté.
     */
    mount: function (hostId, opts) {
      // Le pont wasm passe la opts en CHAÎNE JSON (document::eval natif la
      // parse déjà) — accepter les deux formes.
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
        var map = new ml.Map({
          container: host,
          style: opts.styleUrl,
          center: opts.center || [2.35, 48.85],
          zoom: typeof opts.zoom === 'number' ? opts.zoom : 4,
          attributionControl: { compact: true },
        });
        host._pnexMap = map;
        host._pnexMapMarkers = [];
        host._pnexMapClickSeq = 0;
        host._pnexMapViewSeq = 0;
        map.on('error', function (e) {
          setErr(host, (e && e.error && e.error.message) || 'map');
        });
        // Clic carte nue (hors marker — les markers stoppent la propagation)
        // : mode ajout POI côté page.
        map.on('click', function (e) {
          emitClick('map', null, '', e.lngLat.lng, e.lngLat.lat);
        });
        // Viewport → refetch cluster (debounce : fini les rafales de moveend).
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
        });
        return true;
      } catch (e) {
        setErr(host, String(e));
        try { if (host._pnexMap) host._pnexMap.remove(); } catch (err) { /* déjà mort */ }
        host._pnexMap = null;
        return false;
      }
    },

    /**
     * Remplace tous les markers (pins POI, clusters numérotés, positions
     * GPS live). item: {kind:'pin'|'cluster'|'position', lon, lat, label,
     * emoji, count, id}.
     */
    setItems: function (hostId, items) {
      // Le pont wasm passe les items en CHAÎNE JSON (document::eval natif la
      // parse déjà) — accepter les deux formes (piège identique à mount).
      if (typeof items === 'string') {
        try { items = JSON.parse(items); } catch (e) { return false; }
      }
      var host = hostOf(hostId);
      var map = host && host._pnexMap;
      var ml = maplibreMod();
      if (!host || !map || !ml) return false;
      (host._pnexMapMarkers || []).forEach(function (m) {
        try { m.remove(); } catch (e) { /* déjà retiré */ }
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
        } catch (e) { /* lng/lat invalide : item suivant */ }
      });
      return true;
    },

    /** Remplace les markers (compat Phase A : = setItems en mode pin). */
    setPins: function (hostId, pins) {
      var items = (pins || []).map(function (p) {
        return Object.assign({ kind: 'pin' }, p);
      });
      return api.setItems(hostId, items);
    },

    /** Centre la carte (doux) sur un point. */
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

    /** Dernier clic item/carte (pollé par le pont Rust). */
    lastClick: function () {
      return window.__pnexMapLastClick || null;
    },

    /** Dernier viewport (moveend debouncé, pollé par le pont Rust). */
    lastView: function () {
      return window.__pnexMapView || null;
    },

    /** Code d'erreur du dernier mount (lib|webgl|map|…) ou null. */
    error: function (hostId) {
      var host = hostOf(hostId);
      return (host && host._pnexMapError) || null;
    },

    unmount: function (hostId) {
      var host = hostOf(hostId);
      if (host && host._pnexMap) {
        try { host._pnexMap.remove(); } catch (e) { /* déjà mort */ }
        host._pnexMap = null;
        host._pnexMapMarkers = [];
      }
      if (host) host._pnexMapError = null;
      return true;
    },
  };

  window.pnexMap = api;
})();
