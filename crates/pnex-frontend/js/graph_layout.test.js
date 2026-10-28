// Run with `bun test js/`.
import { expect, test } from 'bun:test';
import Graph from 'graphology';
import { chronoLayout, clusterLayout, forceLayout, hashed, hierarchyLayout, LANE_OF, orbitLayout } from './graph_layout.js';

// Same vectors as `hashed_matches_the_js_port` in pnex-core's layout.rs.
test('hashed is the exact port of layout.rs', () => {
  const close = (got, want) => got.forEach((v, i) => expect(v).toBeCloseTo(want[i], 12));
  close(hashed('case-1', 'obj-1'), [0.4513241717245719, 0.6161696809847302]);
  close(hashed('0192f0c4-case', 'été'), [0.5870898609485221, 0.002698675497132045]);
  close(hashed('', ''), [0.6851156532496949, 0.5234637184821683]);
});

function sample() {
  const g = new Graph({ multi: true, type: 'undirected' });
  for (const id of ['e1', 'e2', 'ann', 'bob', 'carl', 'memo', 'zed']) g.addNode(id, { title: id, size: 8 });
  g.addEdge('e1', 'ann');
  g.addEdge('e1', 'bob');
  g.addEdge('e2', 'bob');
  g.addEdge('e2', 'carl');
  g.addEdge('ann', 'carl');
  return g;
}

const snapshot = (g) => Object.fromEntries(g.mapNodes((n, a) => [n, [a.x, a.y]]));

test('same graph and seed, same drawing', () => {
  const [a, b] = [sample(), sample()];
  forceLayout(a, 'case-1', 1.6);
  forceLayout(b, 'case-1', 1.6);
  expect(snapshot(a)).toEqual(snapshot(b));
  const c = sample();
  forceLayout(c, 'case-2', 1.6);
  expect(snapshot(c)).not.toEqual(snapshot(a));
});

test('placed nodes never move, newcomers get a place', () => {
  const g = sample();
  g.mergeNodeAttributes('ann', { x: 10, y: 20 });
  g.mergeNodeAttributes('bob', { x: -40, y: 5 });
  forceLayout(g, 'case-1', 1.6);
  expect([g.getNodeAttribute('ann', 'x'), g.getNodeAttribute('ann', 'y')]).toEqual([10, 20]);
  expect([g.getNodeAttribute('bob', 'x'), g.getNodeAttribute('bob', 'y')]).toEqual([-40, 5]);
  g.forEachNode((_n, a) => expect(Number.isFinite(a.x) && Number.isFinite(a.y)).toBe(true));
});

test('isolated nodes stand in a column right of the drawing, by title', () => {
  const g = sample();
  forceLayout(g, 'case-1', 1.6);
  const linked = ['e1', 'e2', 'ann', 'bob', 'carl'].map((n) => g.getNodeAttribute(n, 'x'));
  const [memo, zed] = [g.getNodeAttributes('memo'), g.getNodeAttributes('zed')];
  expect(memo.x).toBe(zed.x);
  expect(memo.x).toBeGreaterThan(Math.max(...linked));
  expect(memo.y).toBeLessThan(zed.y);
});

test('many isolated nodes wrap into a block, not one long column', () => {
  const g = new Graph({ type: 'undirected' });
  for (let i = 0; i < 400; i++) g.addNode(`n${String(i).padStart(3, '0')}`, { title: `n${i}`, size: 8 });
  forceLayout(g, 'case-1', 1.6);
  const xs = new Set(g.mapNodes((_n, a) => a.x));
  const ys = new Set(g.mapNodes((_n, a) => a.y));
  expect([xs.size, ys.size]).toEqual([20, 20]);
});

test('the chronological view: dates left to right, objects in their lane', () => {
  const day = 86400000;
  const nodes = [
    { id: 'e2', event: true, at: 400 * day },
    { id: 'e1', event: true, at: 0 },
    { id: 'e3', event: true, at: 401 * day },
    { id: 'ann', family: 'person' },
    { id: 'acme', family: 'organization' },
    { id: 'gadget', family: 'device' },
    { id: 'memo', family: 'piece' },
  ];
  const edges = [
    { a: 'e1', b: 'ann' }, { a: 'e2', b: 'ann' }, { a: 'e2', b: 'acme' }, { a: 'e3', b: 'gadget' },
  ];
  const p = chronoLayout(nodes, edges);
  expect(p.e1.x).toBeLessThan(p.e2.x);
  expect(p.e2.x).toBeLessThan(p.e3.x);
  // A long gap is compressed: at most 5 steps of 26.
  expect(p.e2.x - p.e1.x).toBe(130);
  // Ann sits between her two events, above; Acme below; a device farthest.
  expect(p.ann.x).toBe((p.e1.x + p.e2.x) / 2);
  const lane = (y, rank) => {
    const base = Math.sign(rank) * (190 + 230 * (Math.abs(rank) - 1));
    return Math.sign(y) === Math.sign(rank) && Math.abs(y - base) <= 120;
  };
  expect(lane(p.ann.y, LANE_OF.person)).toBe(true);
  expect(lane(p.acme.y, LANE_OF.organization)).toBe(true);
  expect(lane(p.gadget.y, LANE_OF.other)).toBe(true);
  // Unlinked: the column on the left.
  expect(p.memo.x).toBeLessThan(p.e1.x);
});

test('a parliament-sized case lays out in about a second (Barnes-Hut, scaled iterations)', () => {
  const g = new Graph({ multi: true, type: 'undirected' });
  for (let i = 0; i < 2000; i++) g.addNode(`p${i}`, { title: `p${i}`, size: 8 });
  // Members of 12 groups, as deputies of a chamber.
  for (let i = 12; i < 2000; i++) g.addEdge(`p${i}`, `p${i % 12}`);
  const t0 = performance.now();
  forceLayout(g, 'case-1', 1.6);
  const ms = performance.now() - t0;
  g.forEachNode((_n, a) => expect(Number.isFinite(a.x) && Number.isFinite(a.y)).toBe(true));
  expect(ms).toBeLessThan(4000);
  console.log(`2000 nodes: ${Math.round(ms)} ms`);
}, 10000);

test('hierarchy: owners above what they own, cycles cut, loose nodes below', () => {
  const nodes = ['state', 'holding', 'tv', 'radio', 'show', 'alone'].map((id) => ({ id, title: id }));
  const rel = (a, b) => ({ a, b, kind: 'relation' });
  const edges = [rel('state', 'holding'), rel('holding', 'tv'), rel('holding', 'radio'), rel('tv', 'show'),
    rel('show', 'holding'), { a: 'tv', b: 'alone', kind: 'role' }];
  const out = hierarchyLayout(nodes, edges);
  expect(out.state.y).toBeLessThan(out.holding.y);
  expect(out.holding.y).toBeLessThan(out.tv.y);
  expect(out.tv.y).toBe(out.radio.y);
  expect(out.tv.y).toBeLessThan(out.show.y);
  expect(out.alone.y).toBeGreaterThan(out.show.y);
  for (const p of Object.values(out)) expect(Number.isFinite(p.x) && Number.isFinite(p.y)).toBe(true);  // A member sits under its party.
  const party = hierarchyLayout([{ id: 'mp' }, { id: 'party' }], [{ a: 'mp', b: 'party', kind: 'relation', link: 'party_member' }]);
  expect(party.party.y).toBeLessThan(party.mp.y);
});

test('clusters: one disc per type, largest first, busiest node at its centre', () => {
  const nodes = [
    { id: 'p1', type: 'person', title: 'A' }, { id: 'p2', type: 'person', title: 'B' },
    { id: 'p3', type: 'person', title: 'C' }, { id: 'm1', type: 'media', title: 'M' },
    { id: 'e1', event: true, type: 'event', title: 'E' },
  ];
  const edges = [{ a: 'p2', b: 'm1' }, { a: 'p2', b: 'e1' }];
  const { positions, clusters } = clusterLayout(nodes, edges);
  expect(clusters.map((c) => [c.key, c.count])).toEqual([['person', 3], ['event', 1], ['media', 1]]);
  const person = clusters[0];
  expect(positions.p2).toEqual({ x: person.x, y: person.y });
  for (const n of nodes) expect(Number.isFinite(positions[n.id].x)).toBe(true);
});

test('clusters: neighbouring discs never overlap, 2 or 3 groups', () => {
  const make = (sizes) => sizes.flatMap((n, g) =>
    Array.from({ length: n }, (_, i) => ({ id: `g${g}-${i}`, type: `t${g}`, title: `${i}` })));
  for (const sizes of [[926, 40], [926, 40, 12], [5, 5, 5]]) {
    const { clusters } = clusterLayout(make(sizes), []);
    const k = clusters.length;
    for (let i = 0; i < (k > 2 ? k : k - 1); i++) {
      const [p, q] = [clusters[i], clusters[(i + 1) % k]];
      expect(Math.hypot(p.x - q.x, p.y - q.y)).toBeGreaterThanOrEqual(p.r + q.r + 90 - 1e-6);
    }
  }
  expect(clusterLayout(make([3]), []).clusters[0]).toMatchObject({ x: 0, y: 0 });
});

test('clusters: one disc per role, by the link a node mostly starts', () => {
  const nodes = [
    { id: 'cert', type: 'organization' }, { id: 'c1', type: 'organization' },
    { id: 'c2', type: 'organization' }, { id: 'rs', type: 'title' }, { id: 'lone', type: 'organization' },
  ];
  const rel = (a, b, link) => ({ a, b, kind: 'relation', link, label: link.replace('_', ' ') });
  const edges = [rel('cert', 'rs', 'holds'), rel('c1', 'rs', 'accredited_for'), rel('c2', 'rs', 'accredited_for')];
  const { clusters } = clusterLayout(nodes, edges);
  const by = Object.fromEntries(clusters.map((c) => [c.key, [c.type, c.via, c.count]]));
  expect(by).toEqual({
    'organization accredited_for': ['organization', 'accredited for', 2],
    'organization holds': ['organization', 'holds', 1],
    organization: ['organization', null, 1],
    title: ['title', null, 1],
  });
});

test('orbit: neighbours on ring 1 with the link label, theirs on ring 2, the rest hidden', () => {
  const nodes = ['c', 'a', 'b', 'x', 'far'].map((id) => ({ id, type: 'person', title: id }));
  const edges = [
    { a: 'c', b: 'a', label: 'owns' }, { a: 'b', b: 'c', label: 'funds' },
    { a: 'a', b: 'x', label: 'knows' },
  ];
  const { positions, ring, via } = orbitLayout(nodes, edges, 'c');
  expect(ring).toEqual({ c: 0, a: 1, b: 1, x: 2 });
  expect(via).toEqual({ a: 'owns', b: 'funds' });
  expect(positions.c).toEqual({ x: 0, y: 0 });
  expect(positions.far).toEqual({ x: 0, y: 0 });
  const d = (id) => Math.hypot(positions[id].x, positions[id].y);
  expect(d('a')).toBeCloseTo(d('b'));
  expect(d('x')).toBeGreaterThan(d('a'));
  expect(orbitLayout(nodes, edges, 'gone').positions).toEqual({});
});
