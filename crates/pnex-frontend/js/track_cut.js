// Cuts a track at the cursor (cases-workspace.md K28). A segment is a
// gap-free list of [lon, lat, t_ms] sorted by time; nothing is interpolated
// across two segments (signal gap). Mirrors pnex_core::track::sample.

function at(seg, t) {
  var lo = 0;
  var hi = seg.length - 1;
  while (hi - lo > 1) {
    var mid = (lo + hi) >> 1;
    if (seg[mid][2] <= t) lo = mid; else hi = mid;
  }
  var a = seg[lo];
  var b = seg[hi];
  if (b[2] === a[2]) return [a[0], a[1]];
  var k = (t - a[2]) / (b[2] - a[2]);
  return [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
}

/** Coordinates of `seg` restricted to [from, to], ends interpolated; null when outside. */
export function slice(seg, from, to) {
  if (!seg.length || from > to) return null;
  var first = seg[0][2];
  var last = seg[seg.length - 1][2];
  if (last < from || first > to) return null;
  var out = [];
  if (first < from) out.push(at(seg, from));
  for (var i = 0; i < seg.length; i++) {
    var p = seg[i];
    if (p[2] >= from && p[2] <= to) out.push([p[0], p[1]]);
  }
  if (last > to) out.push(at(seg, to));
  return out;
}

/**
 * Past of each segment at `t`, split into older and recent (the last
 * `recentMs`): LineString coordinates, at least two points each.
 */
export function cut(segments, t, recentMs) {
  var older = [];
  var recent = [];
  (segments || []).forEach(function (seg) {
    var o = slice(seg, -Infinity, t - recentMs);
    var r = slice(seg, t - recentMs, t);
    if (o && o.length > 1) older.push(o);
    if (r && r.length > 1) recent.push(r);
  });
  return { older: older, recent: recent };
}
