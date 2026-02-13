/**
 * Pure helper functions for canvas region drawing and keyframe interpolation.
 */

export const HANDLE_SIZE = 7; // half-size of resize handles in canvas px

// Cursor names for 8 handles: TL TC TR ML MR BL BC BR
export const HANDLE_CURSORS = [
  'nw-resize', 'n-resize', 'ne-resize',
  'w-resize',              'e-resize',
  'sw-resize', 's-resize', 'se-resize',
];

/** Normalize a rect so w/h are always positive. */
export function norm(r) {
  return {
    x: r.w >= 0 ? r.x : r.x + r.w,
    y: r.h >= 0 ? r.y : r.y + r.h,
    w: Math.abs(r.w),
    h: Math.abs(r.h),
  };
}

/** Point-in-rect hit test (uses normalized coords). */
export function hitRect(r, p) {
  const n = norm(r);
  return p.x >= n.x && p.x <= n.x + n.w && p.y >= n.y && p.y <= n.y + n.h;
}

/**
 * Returns 8 handle points for a rect: TL TC TR ML MR BL BC BR
 */
export function handlePoints(r) {
  const n = norm(r);
  const x2 = n.x + n.w, y2 = n.y + n.h;
  const mx = n.x + n.w / 2, my = n.y + n.h / 2;
  return [
    { x: n.x, y: n.y }, { x: mx, y: n.y }, { x: x2, y: n.y },
    { x: n.x, y: my },                      { x: x2, y: my },
    { x: n.x, y: y2 }, { x: mx, y: y2 }, { x: x2, y: y2 },
  ];
}

/** Returns the index of the handle under point p, or -1.
 *  hitSize defaults to HANDLE_SIZE but can be overridden (e.g. HANDLE_SIZE/zoom). */
export function hitHandle(r, p, hitSize = HANDLE_SIZE) {
  return handlePoints(r).findIndex(
    h => Math.abs(p.x - h.x) <= hitSize && Math.abs(p.y - h.y) <= hitSize
  );
}

/**
 * Apply a handle drag to produce a new rect.
 * hi: 0=TL 1=TC 2=TR 3=ML 4=MR 5=BL 6=BC 7=BR
 */
export function applyHandle(orig, hi, dx, dy) {
  let { x, y, w, h } = orig;
  if (hi === 0 || hi === 3 || hi === 5) { x += dx; w -= dx; }
  if (hi === 2 || hi === 4 || hi === 7) { w += dx; }
  if (hi === 0 || hi === 1 || hi === 2) { y += dy; h -= dy; }
  if (hi === 5 || hi === 6 || hi === 7) { h += dy; }
  return { x, y, w: Math.max(8, w), h: Math.max(8, h) };
}

/**
 * Interpolate region positions from keyframes at timestamp ts.
 * Returns: { [name]: { x, y, width, height } } in VIDEO coordinates.
 */
export function interpolate(keyframes, ts) {
  if (!keyframes.length) return {};

  const kfs = [...keyframes].sort((a, b) => a.timestamp - b.timestamp);

  const toObj = kf => {
    const o = {};
    kf.regions.forEach(r => {
      o[r.name] = { x: r.x, y: r.y, width: r.width, height: r.height };
    });
    return o;
  };

  if (ts <= kfs[0].timestamp) return toObj(kfs[0]);
  if (ts >= kfs[kfs.length - 1].timestamp) return toObj(kfs[kfs.length - 1]);

  let a = kfs[0], b = null;
  for (let i = 0; i < kfs.length - 1; i++) {
    if (kfs[i].timestamp <= ts && ts <= kfs[i + 1].timestamp) {
      a = kfs[i]; b = kfs[i + 1]; break;
    }
  }
  if (!b) return {};

  const t = (ts - a.timestamp) / (b.timestamp - a.timestamp);
  const bm = {};
  b.regions.forEach(r => bm[r.name] = r);

  const res = {};
  a.regions.forEach(ra => {
    const rb = bm[ra.name] || ra;
    res[ra.name] = {
      x: Math.round(ra.x + (rb.x - ra.x) * t),
      y: Math.round(ra.y + (rb.y - ra.y) * t),
      width: Math.round(ra.width + (rb.width - ra.width) * t),
      height: Math.round(ra.height + (rb.height - ra.height) * t),
    };
  });
  return res;
}

/** Fetch wrapper — throws on HTTP error. */
export async function api(url, opts) {
  const r = await fetch(url, opts);
  const j = await r.json();
  if (!r.ok) throw new Error(j.error || 'Request failed');
  return j;
}
