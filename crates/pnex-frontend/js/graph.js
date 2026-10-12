// The case graph drawn by Sigma.js (cases-graph-sigma.md GS1–GS10), bundled
// as an IIFE into assets/graph.js and loaded by main.rs like map.js.
//
// window.pnexGraph:
//   mount(hostId, opts) → bool      opts {caseId, canWrite, canDrag}
//   setData(hostId, data)           {nodes, edges, aspect}
//   setState(hostId, state)         {selected, faded, edges, hiddenFamilies, mode}
//   relayout, fit, zoomIn, zoomOut, unmount, error(hostId)
//   positions(hostId) → JSON        force-view positions {id: [x, y]}
//   screenPos(hostId, id) → JSON    {x, y} in viewport pixels (e2e)
// Events to Rust, polled (no closure crosses the bridge):
//   window.__pnexGraphEvent = JSON {seq, host, kind, id}, kind ∈
//   click | dblclick | stage | drop.
// Data comes as JSON and is drawn on canvas only (R11, R13).
import Graph from 'graphology';
import Sigma from 'sigma';
import { animateNodes } from 'sigma/utils';
import EdgeCurveProgram from '@sigma/edge-curve';
import { createNodeBorderProgram } from '@sigma/node-border';
import { chronoLayout, forceLayout } from './graph_layout.js';

const hosts = {};
let seq = 0;

const FALLBACK = {
  ink: '#16212b', ground: '#eef1f4', surface: '#ffffff', slate: '#4c5966',
  'line-strong': '#c3cbd3', signal: '#0e7c9c', mark: '#e5482d', ok: '#2b7a4b',
  idle: '#8793a0', warn: '#c77700',
};
const FAMILY_TOKEN = {
  person: 'signal', organization: 'ok', place: 'warn', piece: 'mark',
  claim: 'mark', identifier: 'warn', other: 'idle',
};
// Families drawn as a ring (coloured border, ground core), like events.
const RINGED = new Set(['claim', 'identifier']);

function readColors() {
  const css = getComputedStyle(document.documentElement);
  const out = {};
  for (const k of Object.keys(FALLBACK)) {
    out[k] = css.getPropertyValue(`--color-${k}`).trim() || FALLBACK[k];
  }
  return out;
}

// `c` mixed toward `to` by `t` (hex colours only; anything else as is).
function mix(c, to, t) {
  const hex = (s) => /^#[0-9a-f]{6}$/i.test(s) && [1, 3, 5].map((i) => parseInt(s.slice(i, i + 2), 16));
  const [a, b] = [hex(c), hex(to)];
  if (!a || !b) return c;
  return '#' + a.map((v, i) => Math.round(v + (b[i] - v) * t).toString(16).padStart(2, '0')).join('');
}

function parse(v) {
  return typeof v === 'string' ? JSON.parse(v) : v;
}

function short(s) {
  const chars = [...String(s || '')];
  return chars.length > 30 ? chars.slice(0, 30).join('') + '…' : chars.join('');
}

function webgl() {
  try {
    const c = document.createElement('canvas');
    return !!(c.getContext('webgl2') || c.getContext('webgl'));
  } catch (_) {
    return false;
  }
}

function emit(host, kind, id) {
  seq += 1;
  window.__pnexGraphEvent = JSON.stringify({ seq, host, kind, id: id ?? null });
}

// Boxes of the labels drawn this frame (reset before each render): the
// density grid of Sigma thins labels out but lets neighbours overlap.
let labelBoxes = [];

// Labels: a halo of the ground colour under the text, never HTML (R11).
// A label that would overlap one already drawn is skipped, except the
// focused node's own.
function drawLabel(ctx, data, settings) {
  if (!data.label) return;
  const size = settings.labelSize;
  ctx.font = `${data.highlighted ? 600 : 400} ${size}px ${settings.labelFont}`;
  const [x, y] = [data.x + data.size + 4, data.y + size / 3];
  const box = [x - 2, y - size, x + ctx.measureText(data.label).width + 2, y + 4];
  const hit = (b) => box[0] < b[2] && b[0] < box[2] && box[1] < b[3] && b[1] < box[3];
  if (!data.highlighted && labelBoxes.some(hit)) return;
  labelBoxes.push(box);
  ctx.lineJoin = 'round';
  ctx.lineWidth = 4;
  ctx.strokeStyle = data.halo;
  ctx.strokeText(data.label, x, y);
  ctx.fillStyle = data.ink;
  ctx.fillText(data.label, x, y);
}

// Hover: the full title in a box (canvas too).
function drawHover(ctx, data, settings) {
  const size = settings.labelSize;
  const text = data.full || data.label || '';
  ctx.font = `600 ${size}px ${settings.labelFont}`;
  const w = ctx.measureText(text).width;
  const [x, y] = [data.x + data.size + 2, data.y - size / 2 - 5];
  ctx.fillStyle = data.surface;
  ctx.strokeStyle = data.line;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.roundRect(x, y, w + 12, size + 10, 4);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = data.ink;
  ctx.fillText(text, x + 6, y + size + 2);
}

const NodeProgram = createNodeBorderProgram({
  borders: [
    { size: { attribute: 'b1', defaultValue: 0 }, color: { attribute: 'c1' } },
    { size: { attribute: 'b2', defaultValue: 0 }, color: { attribute: 'c2' } },
    { size: { fill: true }, color: { attribute: 'color' } },
  ],
  drawLabel,
  drawHover,
});

function nodeSize(n, deg) {
  return n.event ? 5 + 1.1 * Math.sqrt(deg) : Math.min(17, 5.5 + 2.4 * Math.sqrt(deg));
}

// Colours and rings of a node from its kind and family.
function paint(n, colors) {
  const fam = colors[FAMILY_TOKEN[n.family] || 'idle'];
  if (n.event) return { color: colors.ground, b1: 0.32, c1: colors.ink, b2: 0, c2: colors.ground };
  if (RINGED.has(n.family)) return { color: colors.ground, b1: 0.3, c1: fam, b2: 0, c2: fam };
  if (n.extra) return { color: fam, b1: 0.2, c1: fam, b2: 0.15, c2: colors.ground };
  return { color: fam, b1: 0.12, c1: colors.ground, b2: 0, c2: fam };
}

function mount(hostId, optsRaw) {
  const opts = parse(optsRaw) || {};
  const el = document.getElementById(hostId);
  if (!el) {
    hosts[hostId] = { error: 'host' };
    return false;
  }
  if (hosts[hostId]?.renderer) return true;
  if (!webgl()) {
    hosts[hostId] = { error: 'webgl' };
    return false;
  }
  const graph = new Graph({ multi: true, type: 'undirected' });
  const h = {
    el, graph, opts, colors: readColors(), force: new Map(), data: { nodes: [], edges: [] },
    state: { selected: null, faded: new Set(), edges: {}, hidden: new Set(), mode: 'force' },
    hover: null, hoverEdge: null, drag: null, moved: false, justDropped: false, error: null,
  };
  try {
    h.renderer = new Sigma(graph, el, {
      allowInvalidContainer: true,
      defaultNodeType: 'pnex',
      nodeProgramClasses: { pnex: NodeProgram },
      edgeProgramClasses: { curve: EdgeCurveProgram },
      renderEdgeLabels: true,
      enableEdgeEvents: true,
      labelFont: '"IBM Plex Sans", system-ui, sans-serif',
      labelSize: 12,
      edgeLabelFont: '"IBM Plex Sans", system-ui, sans-serif',
      edgeLabelSize: 11,
      labelDensity: 3,
      labelGridCellSize: 90,
      labelRenderedSizeThreshold: 4,
      minCameraRatio: 0.15,
      maxCameraRatio: 3,
      stagePadding: 70,
      doubleClickZoomingDuration: 0,
      zIndex: true,
      nodeReducer: (node, d) => nodeReducer(h, node, d),
      edgeReducer: (edge, d) => edgeReducer(h, edge, d),
    });
  } catch (e) {
    hosts[hostId] = { error: 'webgl' };
    return false;
  }
  wire(hostId, h);
  h.observer = new MutationObserver(() => {
    h.colors = readColors();
    repaint(h);
  });
  h.observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  hosts[hostId] = h;
  return true;
}

function wire(hostId, h) {
  const r = h.renderer;
  r.on('beforeRender', () => (labelBoxes = []));
  r.on('enterNode', ({ node }) => {
    h.hover = node;
    refresh(h, true);
  });
  r.on('leaveNode', () => {
    h.hover = null;
    refresh(h, true);
  });
  r.on('enterEdge', ({ edge }) => {
    h.hoverEdge = edge;
    refresh(h, true);
  });
  r.on('leaveEdge', () => {
    h.hoverEdge = null;
    refresh(h, true);
  });
  r.on('clickNode', ({ node }) => {
    if (!h.justDropped) emit(hostId, 'click', node);
  });
  r.on('doubleClickNode', (e) => {
    e.preventSigmaDefault();
    emit(hostId, 'dblclick', e.node);
  });
  r.on('doubleClickStage', (e) => e.preventSigmaDefault());
  r.on('clickStage', () => {
    if (!h.justDropped) emit(hostId, 'stage', null);
  });
  if (h.opts.canDrag === false) return;
  r.on('downNode', ({ node }) => {
    h.drag = node;
    h.moved = false;
    // The frame stays put under the hand: no rescale while dragging.
    if (!r.getCustomBBox()) r.setCustomBBox(r.getBBox());
  });
  r.getMouseCaptor().on('mousemovebody', (e) => {
    if (!h.drag) return;
    const p = r.viewportToGraph(e);
    h.graph.mergeNodeAttributes(h.drag, { x: p.x, y: p.y });
    h.moved = true;
    e.preventSigmaDefault();
    e.original.preventDefault();
    e.original.stopPropagation();
  });
  const release = () => {
    if (!h.drag) return;
    const node = h.drag;
    h.drag = null;
    if (!h.moved) return;
    // Only the force view is kept; the chronological one is derived.
    if (h.state.mode === 'force') {
      const { x, y } = h.graph.getNodeAttributes(node);
      h.force.set(node, [x, y]);
      emit(hostId, 'drop', node);
    }
    h.justDropped = true;
    setTimeout(() => (h.justDropped = false), 0);
  };
  r.getMouseCaptor().on('mouseup', release);
  r.getMouseCaptor().on('mouseleave', release);
}

// The focus: hovered node, else the selected one; its direct neighbours.
function focusOf(h) {
  const f = h.hover || h.state.selected;
  if (!f || !h.graph.hasNode(f)) return null;
  return { node: f, near: new Set(h.graph.neighbors(f)), degree: h.graph.degree(f) };
}

function nodeReducer(h, node, d) {
  const c = h.colors;
  const out = { ...d, halo: c.ground, ink: c.ink, surface: c.surface, line: c['line-strong'] };
  if (h.state.hidden.has(d.family)) return { ...out, hidden: true };
  const focus = h.focus;
  const selected = h.state.selected === node;
  let fade = h.state.faded.has(node) ? 0.6 : 0;
  if (focus && focus.node !== node && !focus.near.has(node)) {
    fade = 0.78;
    out.label = null;
  } else if (focus) {
    out.forceLabel = true;
    out.zIndex = 1;
  }
  if (focus?.node === node || selected) {
    out.highlighted = true;
    out.forceLabel = true;
    out.zIndex = 2;
  }
  if (selected) {
    out.b2 = Math.max(out.b2 || 0, 0.18);
    out.c2 = c.signal;
  }
  if (fade > 0) {
    for (const k of ['color', 'c1', 'c2']) out[k] = mix(out[k], c.ground, fade);
  }
  return out;
}

function edgeReducer(h, edge, d) {
  const c = h.colors;
  const shown = h.state.edges[edge];
  if (shown === 'gone') return { ...d, hidden: true };
  const [s, t] = h.graph.extremities(edge);
  if (h.state.hidden.has(h.graph.getNodeAttribute(s, 'family')) ||
      h.state.hidden.has(h.graph.getNodeAttribute(t, 'family'))) {
    return { ...d, hidden: true };
  }
  const out = { ...d, color: c['line-strong'], size: 1.2, label: null };
  const focus = h.focus;
  const lit = focus && (s === focus.node || t === focus.node);
  if (lit) {
    out.color = c.signal;
    out.size = 2;
    out.zIndex = 1;
    if (focus.degree <= 6) out.label = d.label;
  } else if (focus) {
    out.color = mix(c['line-strong'], c.ground, 0.88);
  }
  if (h.hoverEdge === edge) out.label = d.label;
  if (shown === 'ahead' || h.state.faded.has(s) || h.state.faded.has(t)) {
    out.color = mix(out.color, c.ground, 0.6);
  }
  return out;
}

function repaint(h) {
  h.graph.forEachNode((n, attr) => h.graph.mergeNodeAttributes(n, paint(attr, h.colors)));
  h.renderer.setSetting('labelColor', { color: h.colors.ink });
  h.renderer.setSetting('edgeLabelColor', { color: h.colors.slate });
  refresh(h);
}

// `light`: only the reducers' output changed (hover), no re-indexation.
function refresh(h, light) {
  h.focus = focusOf(h);
  h.renderer.refresh(light ? { skipIndexation: true } : undefined);
}

// Force-view positions of `work` (a detached graph: Sigma rejects a node
// without x/y), placing what has none yet; remembered in `h.force`.
function forcePositions(h, work) {
  work.forEachNode((n) => {
    const p = h.force.get(n);
    work.mergeNodeAttributes(n, p ? { x: p[0], y: p[1] } : { x: undefined, y: undefined });
  });
  forceLayout(work, h.opts.caseId || '', h.data.aspect);
  work.forEachNode((n, attr) => h.force.set(n, [attr.x, attr.y]));
}

// Positions of the current view: the force one, or the derived timeline.
function viewTarget(h) {
  if (h.state.mode === 'chrono') return chronoLayout(h.data.nodes, h.data.edges);
  const out = {};
  h.force.forEach((p, n) => (out[n] = { x: p[0], y: p[1] }));
  return out;
}

// Moves the drawn nodes to the current view, gliding unless reduced motion.
function show(h, animate) {
  const target = viewTarget(h);
  const reduced = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
  if (animate && !reduced) {
    animateNodes(h.graph, target, { duration: 700 });
  } else {
    h.graph.forEachNode((n) => target[n] && h.graph.mergeNodeAttributes(n, target[n]));
  }
}

function setData(hostId, raw) {
  const h = hosts[hostId];
  if (!h?.renderer) return false;
  const data = parse(raw);
  h.data = data;
  const g = h.graph;
  const known = new Set(g.nodes());
  const work = new Graph({ multi: true, type: 'undirected' });
  for (const n of data.nodes) {
    if (Number.isFinite(n.x) && !h.force.has(n.id)) h.force.set(n.id, [n.x, n.y]);
    if (work.hasNode(n.id)) continue;
    work.addNode(n.id, {
      label: short(n.title), full: n.title, family: n.family, event: !!n.event,
      extra: !!n.extra, at: n.at,
    });
  }
  data.edges.forEach((e, k) => {
    if (!work.hasNode(e.a) || !work.hasNode(e.b)) return;
    work.addEdgeWithKey(`e${k}`, e.a, e.b, {
      label: e.label, type: e.kind === 'relation' ? 'curve' : 'line',
      curvature: 0.25,
    });
  });
  work.forEachNode((n, attr) => {
    work.mergeNodeAttributes(n, { size: nodeSize(attr, work.degree(n)), ...paint(attr, h.colors) });
  });
  forcePositions(h, work);
  const target = viewTarget(h);
  work.forEachNode((n) => work.mergeNodeAttributes(n, target[n]));
  // A newcomer may need room: only then does the drawing re-fit.
  if (work.nodes().some((n) => !known.has(n))) h.renderer.setCustomBBox(null);
  g.clear();
  g.import(work);
  repaint(h);
  return true;
}

function setState(hostId, raw) {
  const h = hosts[hostId];
  if (!h?.renderer) return false;
  const s = parse(raw);
  const modeChanged = (s.mode || 'force') !== h.state.mode;
  h.state = {
    selected: s.selected || null,
    faded: new Set(s.faded || []),
    edges: s.edges || {},
    hidden: new Set(s.hiddenFamilies || []),
    mode: s.mode || 'force',
  };
  if (modeChanged) {
    h.renderer.setCustomBBox(null);
    show(h, true);
    h.renderer.getCamera().animatedReset({ duration: 300 });
  }
  refresh(h);
  return true;
}

function relayout(hostId) {
  const h = hosts[hostId];
  if (!h?.renderer) return false;
  h.force.clear();
  h.renderer.setCustomBBox(null);
  forcePositions(h, h.graph.copy());
  show(h, false);
  refresh(h);
  h.renderer.getCamera().animatedReset({ duration: 300 });
  return true;
}

function camera(hostId, f) {
  const h = hosts[hostId];
  if (!h?.renderer) return false;
  f(h.renderer.getCamera());
  return true;
}

function positions(hostId) {
  const h = hosts[hostId];
  if (!h?.renderer) return '{}';
  const out = {};
  h.force.forEach((p, n) => {
    if (h.graph.hasNode(n)) out[n] = p;
  });
  return JSON.stringify(out);
}

function screenPos(hostId, id) {
  const h = hosts[hostId];
  if (!h?.renderer || !h.graph.hasNode(id)) return 'null';
  const { x, y } = h.graph.getNodeAttributes(id);
  const p = h.renderer.graphToViewport({ x, y });
  const r = h.el.getBoundingClientRect();
  return JSON.stringify({ x: r.left + p.x, y: r.top + p.y });
}

function unmount(hostId) {
  const h = hosts[hostId];
  if (h?.renderer) {
    h.observer?.disconnect();
    h.renderer.kill();
  }
  delete hosts[hostId];
}

window.pnexGraph = {
  mount,
  setData,
  setState,
  relayout,
  fit: (id) => camera(id, (c) => c.animatedReset({ duration: 300 })),
  zoomIn: (id) => camera(id, (c) => c.animatedZoom({ duration: 200 })),
  zoomOut: (id) => camera(id, (c) => c.animatedUnzoom({ duration: 200 })),
  positions,
  screenPos,
  unmount,
  error: (id) => hosts[id]?.error || '',
};
