// Pure layout helpers of the case graph (cases-graph-sigma.md GS4, GS5),
// kept apart from graph.js so `bun test js/` runs them without WebGL.
import Graph from 'graphology';
import forceAtlas2 from 'graphology-layout-forceatlas2';
import noverlap from 'graphology-layout-noverlap';

// Same frame as pnex_core::ontology::layout (SIDE).
const SIDE = 1000;
const MASK64 = (1n << 64n) - 1n;
const encoder = new TextEncoder();

// FNV-1a 64 of `seed`, a 0 byte, then `key` (UTF-8): the exact port of
// `hashed` in layout.rs, two coordinates in [0, 1].
export function hashed(seed, key) {
  let h = 0xcbf29ce484222325n;
  const bytes = [...encoder.encode(seed), 0, ...encoder.encode(key)];
  for (const b of bytes) {
    h ^= BigInt(b);
    h = (h * 0x100000001b3n) & MASK64;
  }
  return [Number(h >> 32n) / 0xffffffff, Number(h & 0xffffffffn) / 0xffffffff];
}

// Places every node of `graph` that has no `x`/`y` yet; nodes that have one
// (saved, dragged, or placed earlier) are fixed and never move. Isolated
// newcomers go in a column right of the drawing instead of the simulation.
export function forceLayout(graph, seed, aspect) {
  const a = Math.sqrt(Math.min(3, Math.max(0.4, aspect || 1.6)));
  const placed = (n) => Number.isFinite(graph.getNodeAttribute(n, 'x'));
  const fresh = graph.filterNodes((n) => !placed(n));
  if (fresh.length === 0) return;
  const loose = fresh.filter((n) => graph.degree(n) === 0);
  const sim = new Graph({ type: 'undirected' });
  graph.forEachNode((n, attr) => {
    if (graph.degree(n) === 0 && !placed(n)) return;
    const fixed = placed(n);
    const [hx, hy] = hashed(seed, n);
    sim.addNode(n, {
      x: fixed ? attr.x : hx * SIDE * a,
      y: fixed ? attr.y : hy * (SIDE / a),
      size: attr.size || 8,
      fixed,
    });
  });
  graph.forEachEdge((_e, _attr, s, t) => {
    if (s !== t && sim.hasNode(s) && sim.hasNode(t) && !sim.hasEdge(s, t)) sim.addEdge(s, t);
  });
  // shortcut: synchronous, ~40 ms at 75 nodes, ~400 ms at 300. Past 300,
  // Barnes-Hut and iterations scaled by 300/n keep a parliament-sized case
  // (graph.rs SIGMA_CAP, 2 000) near 1.1 s; move to a worker if that drags.
  const scale = Math.min(1, 300 / sim.order);
  if (sim.order > 0 && sim.filterNodes((n) => !sim.getNodeAttribute(n, 'fixed')).length > 0) {
    const settings = {
      ...forceAtlas2.inferSettings(sim),
      gravity: 0.6,
      scalingRatio: 12,
      strongGravityMode: true,
      slowDown: 3,
      barnesHutOptimize: sim.order > 300,
    };
    forceAtlas2.assign(sim, { iterations: Math.max(120, Math.round(600 * scale)), settings });
    noverlap.assign(sim, {
      maxIterations: Math.max(40, Math.round(300 * scale)),
      settings: { margin: 10, ratio: 1.6, expansion: 1.1 },
      // noverlap ignores `fixed`: put the fixed ones back where they were.
      outputReducer: (key, pos) =>
        sim.getNodeAttribute(key, 'fixed')
          ? { x: graph.getNodeAttribute(key, 'x'), y: graph.getNodeAttribute(key, 'y') }
          : pos,
    });
    sim.forEachNode((n, attr) => {
      if (!attr.fixed) graph.mergeNodeAttributes(n, { x: attr.x, y: attr.y });
    });
  }
  // The column of isolated nodes, sorted by title, centred on the drawing.
  let [x1, y0, y1] = [-Infinity, Infinity, -Infinity];
  graph.forEachNode((n, attr) => {
    if (!Number.isFinite(attr.x)) return;
    x1 = Math.max(x1, attr.x);
    y0 = Math.min(y0, attr.y);
    y1 = Math.max(y1, attr.y);
  });
  if (!Number.isFinite(x1)) [x1, y0, y1] = [0, 0, 0];
  const title = (n) => String(graph.getNodeAttribute(n, 'title') || n);
  loose.sort((p, q) => title(p).localeCompare(title(q)));
  // Past 15, the column wraps into a block about as wide as tall: a case of
  // unlinked members would otherwise draw as one thin line.
  const gap = 40;
  const rows = Math.max(15, Math.ceil(Math.sqrt(loose.length)));
  const top = (y0 + y1) / 2 - ((Math.min(rows, loose.length) - 1) * gap) / 2;
  loose.forEach((n, i) =>
    graph.mergeNodeAttributes(n, {
      x: x1 + 120 + Math.floor(i / rows) * gap * 2,
      y: top + (i % rows) * gap,
    }),
  );
}

// Rows of the chronological view (GS5): object families above or below the
// events' axis, farther out by rank.
const LANES = { person: -1, piece: -2, claim: -3, organization: 1, place: 2, identifier: 3, other: 4 };
const DAY = 86400000;

// Positions of the chronological view: events left to right by date with
// compressed gaps, objects in their family's lane at the mean X of their
// events, the rest (undated, unlinked) in a column on the left. Derived,
// never saved.
export function chronoLayout(nodes, edges) {
  const out = {};
  const events = nodes
    .filter((n) => n.event && Number.isFinite(n.at))
    .sort((p, q) => p.at - q.at || (p.id < q.id ? -1 : 1));
  let x = 0;
  const rowEnd = [-Infinity, -Infinity, -Infinity];
  const rowY = [0, 55, -55];
  events.forEach((e, i) => {
    if (i > 0) x += 26 * Math.min(5, Math.max(1, (e.at - events[i - 1].at) / DAY / 60));
    // First row whose last event is far enough, else the one freed longest ago.
    let row = rowEnd.findIndex((end) => x - end >= 60);
    if (row < 0) row = rowEnd.indexOf(Math.min(...rowEnd));
    rowEnd[row] = x;
    out[e.id] = { x, y: rowY[row] };
  });
  const xs = {};
  for (const ed of edges) {
    for (const [p, q] of [[ed.a, ed.b], [ed.b, ed.a]]) {
      if (out[p] && !out[q]) (xs[q] ||= []).push(out[p].x);
    }
  }
  const lanes = {};
  const left = [];
  for (const n of nodes) {
    if (out[n.id]) continue;
    const seen = xs[n.id];
    if (!seen || n.event) {
      left.push(n);
      continue;
    }
    const mean = seen.reduce((s, v) => s + v, 0) / seen.length;
    (lanes[n.family in LANES ? n.family : 'other'] ||= []).push({ n, x: mean });
  }
  for (const [family, items] of Object.entries(lanes)) {
    const rank = LANES[family];
    const sign = Math.sign(rank);
    const base = sign * (190 + 230 * (Math.abs(rank) - 1));
    items.sort((p, q) => p.x - q.x);
    // Sub-rows 60 apart, so neighbours in X do not overlap.
    const ends = [];
    for (const { n, x: nx } of items) {
      let sub = ends.findIndex((end) => nx - end >= 90);
      if (sub < 0) sub = ends.push(-Infinity) - 1;
      // Stay inside the lane: at most three sub-rows, then reuse the first.
      sub = Math.min(sub, 2);
      ends[sub] = nx;
      out[n.id] = { x: nx, y: base + sign * sub * 60 };
    }
  }
  const x0 = events.length ? -160 : 0;
  left.forEach((n, i) => {
    out[n.id] = { x: x0, y: (i - (left.length - 1) / 2) * 40 };
  });
  return out;
}

// Link types read upward in the hierarchy view (member -> group).
const GROUPS = new Set(['member_of', 'party_member']);

// Layers of the hierarchy view: each relation points down, from its source
// (owner, funder, group) to its target, so a chain of control reads top to
// bottom. Rank = longest path from a root (a cycle is cut where it closes);
// each layer is ordered by the mean position of its parents, wide layers
// wrap into sub-rows. What has no relation sits in rows under the drawing.
export function hierarchyLayout(nodes, edges) {
  const ids = new Set(nodes.map((n) => n.id));
  const down = new Map();
  const up = new Map();
  for (const e of edges) {
    if (e.kind !== 'relation' || e.a === e.b || !ids.has(e.a) || !ids.has(e.b)) continue;
    // A membership points to its group: the group goes above its members.
    const [a, b] = GROUPS.has(e.link) ? [e.b, e.a] : [e.a, e.b];
    (down.get(a) || down.set(a, []).get(a)).push(b);
    (up.get(b) || up.set(b, []).get(b)).push(a);
  }
  const linked = nodes.filter((n) => down.has(n.id) || up.has(n.id));
  const loose = nodes.filter((n) => !down.has(n.id) && !up.has(n.id));
  // Cut each cycle where a walk from the roots closes it (a back edge).
  const back = new Set();
  const state = new Map(); // 1 on the walk, 2 done
  const walk = (n) => {
    state.set(n, 1);
    for (const c of down.get(n) || []) {
      if (state.get(c) === 1) back.add(`${n}\u0000${c}`);
      else if (!state.has(c)) walk(c);
    }
    state.set(n, 2);
  };
  const roots = linked.filter((n) => !up.has(n.id));
  for (const n of [...roots, ...linked]) if (!state.has(n.id)) walk(n.id);
  const rank = new Map();
  const depth = (n) => {
    if (rank.has(n)) return rank.get(n);
    let r = 0;
    for (const p of up.get(n) || []) if (!back.has(`${p}\u0000${n}`)) r = Math.max(r, depth(p) + 1);
    rank.set(n, r);
    return r;
  };
  linked.forEach((n) => depth(n.id));
  const layers = [];
  for (const n of linked) (layers[rank.get(n.id)] ||= []).push(n);
  const out = {};
  const [gapX, gapY, sub, wrap] = [150, 190, 45, 24];
  let y = 0;
  const title = (n) => String(n.title || n.id);
  const row = (items, top) => {
    const lines = Math.ceil(items.length / wrap);
    items.forEach((n, i) => {
      const line = i % lines;
      const col = Math.floor(i / lines);
      const width = Math.ceil(items.length / lines);
      out[n.id] = { x: (col - (width - 1) / 2) * gapX + (line % 2) * (gapX / 2), y: top + line * sub };
    });
    return top + (lines - 1) * sub;
  };
  layers.forEach((layer, r) => {
    if (r > 0) {
      const mean = (n) => {
        const xs = (up.get(n.id) || []).map((p) => out[p]?.x).filter(Number.isFinite);
        return xs.length ? xs.reduce((s, v) => s + v, 0) / xs.length : Infinity;
      };
      layer.sort((p, q) => mean(p) - mean(q) || title(p).localeCompare(title(q)));
    } else {
      layer.sort((p, q) => (down.get(q.id)?.length || 0) - (down.get(p.id)?.length || 0)
        || title(p).localeCompare(title(q)));
    }
    y = row(layer, y) + gapY;
  });
  loose.sort((p, q) => title(p).localeCompare(title(q)));
  row(loose, y);
  return out;
}

export const LANE_OF = LANES;

// Degree of every node over `edges`, by id.
function degrees(edges) {
  const deg = {};
  for (const e of edges) {
    deg[e.a] = (deg[e.a] || 0) + 1;
    deg[e.b] = (deg[e.b] || 0) + 1;
  }
  return deg;
}

// Golden-angle spiral: the i-th of a group, the busiest at the centre.
const GOLDEN = Math.PI * (3 - Math.sqrt(5));
const STEP = 16;

// The link type a node is most often the source of among the relations
// (a tie goes alphabetically), by id: certifier and certified company are
// both organizations, yet one `porte`, the other `habilite_pour`.
function roles(edges) {
  const out = {};
  for (const e of edges) {
    if (e.kind !== 'relation' || !e.link || e.a === e.b) continue;
    const c = (out[e.a] ||= {});
    c[e.link] = (c[e.link] || 0) + 1;
  }
  const best = {};
  for (const [id, counts] of Object.entries(out)) {
    best[id] = Object.entries(counts).sort((p, q) => q[1] - p[1] || (p[0] < q[0] ? -1 : 1))[0][0];
  }
  return best;
}

// Positions of the cluster view: one disc per object type and role (the
// link it mostly starts; events apart), the busiest nodes in the middle of
// their disc, the discs on a circle, largest first. Also returns each
// disc's centre, radius, count, type and link label, for its label.
// Derived, never saved.
export function clusterLayout(nodes, edges) {
  const deg = degrees(edges);
  const role = roles(edges);
  const labels = {};
  for (const e of edges) if (e.link && e.label) labels[e.link] = e.label;
  const groups = new Map();
  for (const n of nodes) {
    const type = n.event ? 'event' : n.type || n.family || 'other';
    const link = n.event ? null : role[n.id] || null;
    const k = link ? `${type} ${link}` : type;
    if (!groups.has(k)) groups.set(k, { type, link, items: [] });
    groups.get(k).items.push(n);
  }
  const list = [...groups.entries()]
    .map(([key, g]) => ({ key, ...g, r: STEP * Math.sqrt(g.items.length) + 24 }))
    .sort((p, q) => q.items.length - p.items.length || (p.key < q.key ? -1 : 1));
  const gap = 90;
  const around = list.reduce((s, g) => s + 2 * g.r + gap, 0);
  const angles = [];
  let walked = 0;
  for (const g of list) {
    angles.push(((walked + g.r + gap / 2) / around) * 2 * Math.PI - Math.PI / 2);
    walked += 2 * g.r + gap;
  }
  // The arc is not the chord: wide enough that neighbours never overlap.
  let radius = list.length > 1 ? around / (2 * Math.PI) : 0;
  const pairs = list.length > 2 ? list.length : list.length - 1;
  for (let i = 0; i < pairs; i++) {
    const j = (i + 1) % list.length;
    let da = Math.abs(angles[j] - angles[i]);
    if (da > Math.PI) da = 2 * Math.PI - da;
    const chord = 2 * Math.sin(da / 2);
    if (chord > 0) radius = Math.max(radius, (list[i].r + list[j].r + gap) / chord);
  }
  const out = {};
  const clusters = [];
  list.forEach((g, gi) => {
    const a = angles[gi];
    const [cx, cy] = [radius * Math.cos(a), radius * Math.sin(a)];
    g.items.sort((p, q) => (deg[q.id] || 0) - (deg[p.id] || 0) || String(p.title).localeCompare(String(q.title)));
    g.items.forEach((n, i) => {
      const r = STEP * Math.sqrt(i);
      out[n.id] = { x: cx + r * Math.cos(i * GOLDEN), y: cy + r * Math.sin(i * GOLDEN) };
    });
    clusters.push({
      key: g.key, type: g.type, via: g.link ? labels[g.link] || g.link : null,
      x: cx, y: cy, r: g.r, count: g.items.length,
    });
  });
  return { positions: out, clusters };
}

// Positions of the orbit view around `center`: its neighbours on a first
// ring (grouped by type, then by title), their own neighbours on a second
// ring next to them, everything else gathered on the centre, hidden.
// `ring` tells each node's ring (0, 1, 2; absent = hidden), `via` the
// labels of the links that tie a first-ring node to the centre.
export function orbitLayout(nodes, edges, center) {
  const out = {};
  const ring = {};
  const via = {};
  if (!nodes.some((n) => n.id === center)) return { positions: out, ring, via };
  const near = new Map();
  const add = (p, q) => (near.get(p) || near.set(p, new Set()).get(p)).add(q);
  for (const e of edges) {
    if (e.a === e.b) continue;
    add(e.a, e.b);
    add(e.b, e.a);
    const other = e.a === center ? e.b : e.b === center ? e.a : null;
    if (other && e.label) (via[other] ||= new Set()).add(e.label);
  }
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const kind = (id) => (byId.get(id)?.event ? 'event' : byId.get(id)?.type || '');
  const title = (id) => String(byId.get(id)?.title || id);
  const first = [...(near.get(center) || [])].filter((id) => byId.has(id))
    .sort((p, q) => kind(p).localeCompare(kind(q)) || title(p).localeCompare(title(q)));
  ring[center] = 0;
  out[center] = { x: 0, y: 0 };
  // Rings sized so neighbours sit ~70 apart.
  const r1 = Math.max(220, (first.length * 70) / (2 * Math.PI));
  const angle = {};
  first.forEach((id, i) => {
    const a = (i / first.length) * 2 * Math.PI - Math.PI / 2;
    angle[id] = a;
    ring[id] = 1;
    out[id] = { x: r1 * Math.cos(a), y: r1 * Math.sin(a) };
  });
  // Second ring: in the order of the mean angle of their first-ring
  // neighbours, then spread evenly so labels do not pile up.
  const second = [];
  for (const id of first) {
    for (const q of near.get(id) || []) {
      if (ring[q] !== undefined || !byId.has(q)) continue;
      ring[q] = 2;
      second.push(q);
    }
  }
  const mean = (id) => {
    const as = [...(near.get(id) || [])].filter((q) => ring[q] === 1).map((q) => angle[q]);
    const [s, c] = as.reduce(([s, c], a) => [s + Math.sin(a), c + Math.cos(a)], [0, 0]);
    return Math.atan2(s, c);
  };
  const keyed = second.map((id) => ({ id, a: mean(id) })).sort((p, q) => p.a - q.a);
  const r2 = Math.max(r1 + 200, (keyed.length * 26) / (2 * Math.PI));
  const shift = keyed.length ? keyed[0].a : 0;
  keyed.forEach(({ id }, i) => {
    const a = shift + (i / keyed.length) * 2 * Math.PI;
    out[id] = { x: r2 * Math.cos(a), y: r2 * Math.sin(a) };
  });
  for (const n of nodes) if (!out[n.id]) out[n.id] = { x: 0, y: 0 };
  const labels = {};
  for (const [id, set] of Object.entries(via)) if (ring[id] === 1) labels[id] = [...set].slice(0, 2).join(' · ');
  return { positions: out, ring, via: labels };
}
