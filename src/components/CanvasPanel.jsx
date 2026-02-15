import { forwardRef, useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  HANDLE_SIZE, HANDLE_CURSORS,
  norm, hitRect, handlePoints, hitHandle, applyHandle, interpolate,
} from '../helpers.js';

const CanvasPanel = forwardRef(function CanvasPanel(
  { names, keyframes, ts, vpath, vinfo, onRegionDrawn, onRegionMoved, onRegionDeleted },
  ref
) {
  const cvRef        = useRef(null);
  const containerRef = useRef(null);
  const bgImgRef = useRef(null);
  const dragRef  = useRef({
    type: null, name: null, handleIdx: -1,
    origRect: null, currentRect: null,
    sx: 0, sy: 0, drawRect: null, hoveredName: null,
  });
  const panRef   = useRef({ active: false, startX: 0, startY: 0, origPanX: 0, origPanY: 0 });
  const displayRef = useRef([]);

  // transform state kept in a ref so zoom/pan updates are imperative (no re-render per frame)
  const txRef = useRef({ zoom: 1, panX: 0, panY: 0 });

  // only kept as React state for the toolbar percentage display
  const [displayZoom, setDisplayZoom] = useState(1);

  // Canvas pixel dimensions — driven by the container's actual size so the
  // canvas fills all available vertical space while keeping the video aspect ratio.
  const [cw, setCw] = useState(960);
  const [ch, setCh] = useState(540);
  const scale = vinfo && cw > 0 ? cw / vinfo.width : 1;

  // Measure the canvas container and update cw/ch to fit it.
  const sizeCanvas = useCallback(() => {
    const el = containerRef.current;
    if (!el) return;
    const { width: W, height: H } = el.getBoundingClientRect();
    if (!W || !H) return;
    if (vinfo) {
      const s = Math.min(W / vinfo.width, H / vinfo.height);
      setCw(Math.round(vinfo.width  * s));
      setCh(Math.round(vinfo.height * s));
    } else {
      setCw(Math.round(W));
      setCh(Math.round(H));
    }
  }, [vinfo]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const obs = new ResizeObserver(sizeCanvas);
    obs.observe(el);
    sizeCanvas();
    return () => obs.disconnect();
  }, [sizeCanvas]);

  // ── Clamp pan so the image always fills the canvas ─────────────────────────
  // Reads live canvas dimensions from the element ref to avoid stale closures
  // inside the wheel handler.
  function clampPan(px, py, z) {
    const cwCur = cvRef.current?.width  ?? 960;
    const chCur = cvRef.current?.height ?? 540;
    if (z <= 1) return { x: 0, y: 0 };
    return {
      x: Math.max(0, Math.min(cwCur * (1 - 1 / z), px)),
      y: Math.max(0, Math.min(chCur * (1 - 1 / z), py)),
    };
  }

  // ── Draw ───────────────────────────────────────────────────────────────────
  const draw = useCallback(() => {
    const cv = cvRef.current;
    if (!cv) return;
    const ctx = cv.getContext('2d');
    const d   = dragRef.current;
    const { zoom, panX, panY } = txRef.current;
    const hs  = HANDLE_SIZE / zoom;   // constant screen-space handle size

    // Collect interpolated positions in canvas coords
    const pos = interpolate(keyframes, ts);
    displayRef.current = names
      .filter(n => n in pos)
      .map(n => ({
        name: n,
        x: pos[n].x * scale, y: pos[n].y * scale,
        w: pos[n].width * scale, h: pos[n].height * scale,
      }));

    ctx.clearRect(0, 0, cv.width, cv.height);

    // Apply virtual zoom+pan transform
    ctx.save();
    ctx.setTransform(zoom, 0, 0, zoom, -panX * zoom, -panY * zoom);

    // Background frame
    if (bgImgRef.current) ctx.drawImage(bgImgRef.current, 0, 0, cv.width, cv.height);
    else { ctx.fillStyle = '#1e1e2e'; ctx.fillRect(0, 0, cv.width, cv.height); }

    const onKf = keyframes.some(kf => Math.abs(kf.timestamp - ts) < 0.001);
    const lw   = 1.5 / zoom;

    displayRef.current.forEach(r => {
      const interacting = (d.type === 'move' || d.type === 'resize') && d.name === r.name;
      const dr = interacting ? d.currentRect : r;
      const n  = norm(dr);
      const hovered = d.hoveredName === r.name && !d.type;

      // Fill
      ctx.fillStyle = hovered ? 'rgba(34,197,94,.2)' : 'rgba(34,197,94,.08)';
      ctx.fillRect(n.x, n.y, n.w, n.h);

      // Border
      ctx.strokeStyle = onKf ? '#22c55e' : '#16a34a';
      ctx.lineWidth   = hovered ? 2.5 / zoom : lw;
      if (!onKf) ctx.setLineDash([6 / zoom, 3 / zoom]);
      ctx.strokeRect(n.x, n.y, n.w, n.h);
      ctx.setLineDash([]);

      // Label — constant screen size
      ctx.font = `bold ${12 / zoom}px system-ui`;
      const tw = ctx.measureText(r.name).width;
      ctx.fillStyle = 'rgba(0,0,0,.65)';
      ctx.fillRect(n.x, n.y - 18 / zoom, tw + 10 / zoom, 18 / zoom);
      ctx.fillStyle = '#f0fdf4';
      ctx.fillText(r.name, n.x + 5 / zoom, n.y - 4 / zoom);

      // Resize handles — constant screen size
      if (hovered || interacting) {
        ctx.fillStyle   = '#fff';
        ctx.strokeStyle = '#22c55e';
        ctx.lineWidth   = lw;
        handlePoints(n).forEach(hp => {
          ctx.fillRect(hp.x - hs, hp.y - hs, hs * 2, hs * 2);
          ctx.strokeRect(hp.x - hs, hp.y - hs, hs * 2, hs * 2);
        });
      }
    });

    // New-region draw rubber-band
    if (d.type === 'draw' && d.drawRect) {
      const n = norm(d.drawRect);
      ctx.fillStyle   = 'rgba(34,197,94,.12)';
      ctx.fillRect(n.x, n.y, n.w, n.h);
      ctx.strokeStyle = '#4ade80';
      ctx.lineWidth   = lw;
      ctx.setLineDash([5 / zoom, 4 / zoom]);
      ctx.strokeRect(n.x, n.y, n.w, n.h);
      ctx.setLineDash([]);
    }

    ctx.restore();
  }, [keyframes, ts, names, scale]);

  // Keep a ref to the latest draw so imperative callers (wheel, zoom buttons) always
  // get the freshest version without needing it in their deps arrays.
  const drawRef = useRef(draw);
  useEffect(() => { drawRef.current = draw; });

  // ── Zoom helpers ───────────────────────────────────────────────────────────

  // Zoom around the canvas centre
  function zoomBy(factor) {
    const t = txRef.current;
    const cwCur = cvRef.current?.width  ?? 960;
    const chCur = cvRef.current?.height ?? 540;
    const newZoom = Math.max(1, Math.min(8, +(t.zoom * factor).toFixed(3)));
    const cx = cwCur / 2 / t.zoom + t.panX;
    const cy = chCur / 2 / t.zoom + t.panY;
    const { x: px, y: py } = clampPan(cx - cwCur / 2 / newZoom, cy - chCur / 2 / newZoom, newZoom);
    t.zoom = newZoom; t.panX = px; t.panY = py;
    setDisplayZoom(newZoom);
    drawRef.current();
  }

  function zoomReset() {
    txRef.current = { zoom: 1, panX: 0, panY: 0 };
    setDisplayZoom(1);
    drawRef.current();
  }

  // ── cvPos: screen → video coords ──────────────────────────────────────────
  function cvPos(e) {
    const cv = cvRef.current;
    const b  = cv.getBoundingClientRect();
    const { zoom, panX, panY } = txRef.current;
    return {
      x: (e.clientX - b.left) * (cv.width  / b.width)  / zoom + panX,
      y: (e.clientY - b.top)  * (cv.height / b.height) / zoom + panY,
    };
  }

  // ── Hover / cursor update ──────────────────────────────────────────────────
  function updateCursor(p) {
    const cv = cvRef.current;
    const d  = dragRef.current;
    const hs = HANDLE_SIZE / txRef.current.zoom;
    const rects = [...displayRef.current].reverse();

    for (const r of rects) {
      const hi = hitHandle(r, p, hs);
      if (hi >= 0) {
        cv.style.cursor = HANDLE_CURSORS[hi];
        if (d.hoveredName !== r.name) { d.hoveredName = r.name; draw(); }
        return;
      }
    }
    for (const r of rects) {
      if (hitRect(r, p)) {
        if (cv.style.cursor !== 'move' || d.hoveredName !== r.name) {
          cv.style.cursor = 'move'; d.hoveredName = r.name; draw();
        }
        return;
      }
    }
    if (cv.style.cursor !== 'crosshair' || d.hoveredName !== null) {
      cv.style.cursor = 'crosshair'; d.hoveredName = null; draw();
    }
  }

  // ── Effects ────────────────────────────────────────────────────────────────
  useEffect(() => {
    if (!vpath || !vinfo) { bgImgRef.current = null; draw(); return; }
    let cancelled = false;
    // Debounce: wait 120 ms before firing so rapid seeks (e.g. scrubbing)
    // don't flood the backend with frame-decode requests.
    const timer = setTimeout(() => {
      invoke('get_frame', { path: vpath, timestamp: ts })
        .then(base64 => {
          if (cancelled) return;
          const img = new Image();
          img.onload = () => { bgImgRef.current = img; draw(); };
          img.src = `data:image/png;base64,${base64}`;
        })
        .catch(() => { bgImgRef.current = null; draw(); });
    }, 120);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [ts, vpath, vinfo]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => { draw(); }, [draw]);

  // Scroll-wheel zoom at cursor
  useEffect(() => {
    const cv = cvRef.current;
    if (!cv) return;
    const onWheel = e => {
      e.preventDefault();
      const t      = txRef.current;
      const factor = e.deltaY < 0 ? 1.1 : 0.9;
      const newZoom = Math.max(1, Math.min(8, +(t.zoom * factor).toFixed(3)));

      // Canvas pixel under the cursor
      const b  = cv.getBoundingClientRect();
      const sx = (e.clientX - b.left) * (cv.width  / b.width);
      const sy = (e.clientY - b.top)  * (cv.height / b.height);

      // Video coord under cursor (stays fixed after zoom)
      const cx = sx / t.zoom + t.panX;
      const cy = sy / t.zoom + t.panY;

      const { x: px, y: py } = clampPan(cx - sx / newZoom, cy - sy / newZoom, newZoom);
      t.zoom = newZoom; t.panX = px; t.panY = py;
      setDisplayZoom(newZoom);
      drawRef.current();
    };
    cv.addEventListener('wheel', onWheel, { passive: false });
    return () => cv.removeEventListener('wheel', onWheel);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Mouse ──────────────────────────────────────────────────────────────────
  function onMouseDown(e) {
    e.preventDefault();

    // Right-click: pan
    if (e.button === 2) {
      const t = txRef.current;
      const pan = panRef.current;
      pan.active  = true;
      pan.startX  = e.clientX;
      pan.startY  = e.clientY;
      pan.origPanX = t.panX;
      pan.origPanY = t.panY;
      cvRef.current.style.cursor = 'grabbing';
      return;
    }

    const p = cvPos(e);
    const d = dragRef.current;
    const hs = HANDLE_SIZE / txRef.current.zoom;
    const rects = [...displayRef.current].reverse();

    for (const r of rects) {
      const hi = hitHandle(r, p, hs);
      if (hi >= 0) {
        d.type = 'resize'; d.name = r.name; d.handleIdx = hi;
        d.origRect = { ...norm(r) }; d.currentRect = { ...norm(r) };
        d.sx = p.x; d.sy = p.y; draw(); return;
      }
    }
    for (const r of rects) {
      if (hitRect(r, p)) {
        d.type = 'move'; d.name = r.name;
        d.origRect = { ...norm(r) }; d.currentRect = { ...norm(r) };
        d.sx = p.x; d.sy = p.y; draw(); return;
      }
    }
    d.type = 'draw'; d.name = null;
    d.sx = p.x; d.sy = p.y; d.drawRect = { x: p.x, y: p.y, w: 0, h: 0 };
    draw();
  }

  function onMouseMove(e) {
    // Pan (right-drag)
    const pan = panRef.current;
    if (pan.active) {
      const t = txRef.current;
      const cv = cvRef.current;
      const b  = cv.getBoundingClientRect();
      const { x: px, y: py } = clampPan(
        pan.origPanX - (e.clientX - pan.startX) * (cv.width  / b.width)  / t.zoom,
        pan.origPanY - (e.clientY - pan.startY) * (cv.height / b.height) / t.zoom,
        t.zoom
      );
      t.panX = px; t.panY = py;
      drawRef.current();
      return;
    }

    const p = cvPos(e);
    const d = dragRef.current;
    if (!d.type) { updateCursor(p); return; }
    if (d.type === 'resize') {
      d.currentRect = applyHandle(d.origRect, d.handleIdx, p.x - d.sx, p.y - d.sy);
    } else if (d.type === 'move') {
      d.currentRect = { x: d.origRect.x + (p.x - d.sx), y: d.origRect.y + (p.y - d.sy), w: d.origRect.w, h: d.origRect.h };
    } else if (d.type === 'draw') {
      d.drawRect.w = p.x - d.sx; d.drawRect.h = p.y - d.sy;
    }
    draw();
  }

  function onMouseUp(e) {
    // End pan
    const pan = panRef.current;
    if (pan.active) {
      pan.active = false;
      const cv = cvRef.current;
      if (cv && e) { const p = cvPos(e); updateCursor(p); }
      return;
    }

    const d = dragRef.current;
    if (!d.type) return;
    if (d.type === 'draw' && d.drawRect) {
      const n = norm(d.drawRect);
      if (n.w > 8 && n.h > 8) {
        onRegionDrawn(
          { x: Math.round(n.x / scale), y: Math.round(n.y / scale),
            width: Math.round(n.w / scale), height: Math.round(n.h / scale) },
          `region_${names.length + 1}`
        );
      }
      d.drawRect = null;
    } else if ((d.type === 'move' || d.type === 'resize') && d.currentRect) {
      const n = norm(d.currentRect);
      onRegionMoved(d.name, {
        x: Math.round(n.x / scale), y: Math.round(n.y / scale),
        width: Math.round(n.w / scale), height: Math.round(n.h / scale),
      });
    }
    const prev = d.name;
    d.type = null; d.name = null; d.currentRect = null;
    d.hoveredName = prev;
    draw();
  }

  // ── Keyboard delete ────────────────────────────────────────────────────────
  useEffect(() => {
    function onKey(e) {
      if ((e.key === 'Delete' || e.key === 'Backspace') &&
          dragRef.current.hoveredName &&
          document.activeElement.tagName !== 'INPUT') {
        onRegionDeleted(dragRef.current.hoveredName);
        dragRef.current.hoveredName = null;
      }
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onRegionDeleted]);

  // ── Render ─────────────────────────────────────────────────────────────────
  const zoomPct = Math.round(displayZoom * 100);

  return (
    <div className="rounded-xl border border-gray-200 bg-white overflow-hidden flex flex-col flex-1 min-h-0">
      {/* Canvas — fills all available height, maintains video aspect ratio */}
      <div
        ref={containerRef}
        className="relative flex-1 min-h-0 flex items-center justify-center overflow-hidden bg-[#1e1e2e]"
      >
        <canvas
          ref={cvRef}
          width={cw}
          height={ch}
          style={{ display: 'block', cursor: 'crosshair' }}
          onMouseDown={onMouseDown}
          onMouseMove={onMouseMove}
          onMouseUp={onMouseUp}
          onMouseLeave={onMouseUp}
          onContextMenu={e => e.preventDefault()}
        />
        {/* Onboarding overlay — shown when no video is loaded */}
        {!vpath && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 pointer-events-none select-none">
            <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="#4b5563" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <rect x="2" y="6" width="20" height="14" rx="2" />
              <path d="M10 9l5 3-5 3V9z" fill="#4b5563" stroke="none" />
              <path d="M8 3h2M14 3h2" />
            </svg>
            <p className="text-gray-400 text-sm font-medium">No video loaded</p>
            <p className="text-gray-500 text-xs">Select a video in the sidebar to get started</p>
          </div>
        )}
      </div>{/* canvas container */}

      {/* Zoom bar */}
      <div className="flex items-center gap-1.5 px-3 py-1.5 bg-gray-50 border-t border-gray-200">
        <span className="text-xs text-gray-400 mr-1">Scroll · Right-drag to pan</span>
        <ZoomBtn onClick={() => zoomBy(0.9)} title="Zoom out">−</ZoomBtn>
        <span className="text-xs font-mono text-gray-500 w-12 text-center tabular-nums">{zoomPct}%</span>
        <ZoomBtn onClick={() => zoomBy(1.1)} title="Zoom in">+</ZoomBtn>
        <div className="w-px h-4 bg-gray-200 mx-1" />
        <ZoomBtn onClick={zoomReset} title="Reset to 100%">1:1</ZoomBtn>
        <div className="flex-1" />
        <span className="text-xs text-gray-400">
          {names.length > 0
            ? `${names.length} region${names.length > 1 ? 's' : ''} · hover + Delete to remove`
            : 'Drag to draw a region'}
        </span>
      </div>
    </div>
  );
});

function ZoomBtn({ onClick, title, children }) {
  return (
    <button
      onClick={onClick} title={title}
      className="px-2 py-0.5 rounded bg-gray-100 hover:bg-gray-200 text-xs font-medium text-gray-600 transition-colors border border-gray-200 focus:outline-none focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-offset-1"
    >
      {children}
    </button>
  );
}

export default CanvasPanel;
