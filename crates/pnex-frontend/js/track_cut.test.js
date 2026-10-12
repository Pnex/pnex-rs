// Run with `bun test js/`.
import { expect, test } from 'bun:test';
import { cut, slice } from './track_cut.js';

const seg = [[2.0, 48.0, 0], [2.2, 48.2, 10000], [2.4, 48.4, 20000]];

test('slice interpolates both ends', () => {
  const r = slice(seg, 5000, 15000).map((p) => p.map((v) => Math.round(v * 1e6) / 1e6));
  expect(r).toEqual([[2.1, 48.1], [2.2, 48.2], [2.3, 48.3]]);
  expect(slice(seg, 30000, 40000)).toBeNull();
});

test('cut stops at the cursor and never bridges a gap', () => {
  const gap = [seg, [[3.0, 49.0, 60000], [3.2, 49.2, 70000]]];
  const at30 = cut(gap, 30000, 5000);
  expect(at30.older).toEqual([[[2.0, 48.0], [2.2, 48.2], [2.4, 48.4]]]);
  expect(at30.recent).toEqual([]);
  const at65 = cut(gap, 65000, 2000);
  expect(at65.older.length).toBe(2);
  const round = (l) => l.map((p) => p.map((v) => Math.round(v * 1e6) / 1e6));
  expect(at65.recent.map(round)).toEqual([[[3.06, 49.06], [3.1, 49.1]]]);
});

test('cut before the track draws nothing', () => {
  expect(cut([seg], -1, 1000)).toEqual({ older: [], recent: [] });
});
