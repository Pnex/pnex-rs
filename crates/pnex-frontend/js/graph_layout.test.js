// Run with `bun test js/`.
import { expect, test } from 'bun:test';
import Graph from 'graphology';
import { chronoLayout, forceLayout, hashed, hierarchyLayout, LANE_OF } from './graph_layout.js';

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
