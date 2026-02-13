import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import CanvasPanel from './components/CanvasPanel.jsx';
import { interpolate } from './helpers.js';
import './App.css';

// ── Shared primitives ──────────────────────────────────────────────────────

function Input(props) {
  return (
    <input
      {...props}
      className={
        'w-full text-sm border border-gray-200 rounded px-2 py-1.5 ' +
        'focus:outline-none focus:border-green-400 bg-white ' +
        (props.className || '')
      }
    />
  );
}

function Select({ children, ...props }) {
  return (
    <select
      {...props}
      className={
        'w-full text-sm border border-gray-200 rounded px-2 py-1.5 bg-white ' +
        'focus:outline-none focus:border-green-400 ' +
        (props.className || '')
      }
    >
      {children}
    </select>
  );
}

function Btn({ variant = 'default', full, disabled, onClick, title, children, className = '' }) {
  const base = 'text-sm font-medium rounded px-3 py-1.5 transition-colors disabled:opacity-50 ';
  const variants = {
    default: 'bg-gray-100 hover:bg-gray-200 text-gray-700 border border-gray-200',
    primary: 'bg-green-600 hover:bg-green-700 text-white',
    danger:  'bg-red-600 hover:bg-red-700 text-white',
    ghost:   'text-gray-400 hover:text-red-500 px-1 py-0.5',
  };
  return (
    <button
      onClick={onClick} title={title} disabled={disabled}
      className={base + variants[variant] + (full ? ' w-full' : '') + ' ' + className}
    >
      {children}
    </button>
  );
}

function Card({ children, className = '' }) {
  return (
    <div className={'rounded-lg border border-gray-200 bg-white p-3 flex flex-col gap-2 ' + className}>
      {children}
    </div>
  );
}

function CardTitle({ children }) {
  return <span className="text-[11px] font-semibold text-gray-400 uppercase tracking-wider">{children}</span>;
}

function Label({ children }) {
  return <label className="block text-xs text-gray-500 mb-0.5">{children}</label>;
}

// ── Seek bar ───────────────────────────────────────────────────────────────

function SeekBar({ ts, vinfo, keyframes, onChange }) {
  const max  = vinfo?.duration || 1;
  const step = vinfo ? (1 / vinfo.fps).toFixed(4) : 0.033;
  const onKf = keyframes.some(kf => Math.abs(kf.timestamp - ts) < 0.001);

  return (
    <div className="flex flex-col gap-1.5">
      <div className="relative">
        <div className="absolute inset-0 pointer-events-none" aria-hidden="true">
          {keyframes.map((kf, i) => (
            <div
              key={i}
              className="seek-tick"
              style={{ left: `${(kf.timestamp / max) * 100}%` }}
              title={`Keyframe at ${kf.timestamp.toFixed(2)}s`}
            />
          ))}
        </div>
        <input
          type="range"
          className="relative w-full accent-green-600"
          min={0} max={max.toFixed(3)} step={step}
          value={ts}
          onChange={e => onChange(parseFloat(e.target.value))}
        />
      </div>
      <div className="flex items-center gap-2">
        <span className="text-xs font-mono text-gray-600 tabular-nums">{ts.toFixed(2)} s</span>
        {onKf && (
          <span className="text-[11px] px-1.5 py-0.5 rounded-full bg-green-100 text-green-700 font-medium">
            ● keyframe
          </span>
        )}
        {!onKf && keyframes.length > 0 && (
          <span className="text-[11px] px-1.5 py-0.5 rounded-full bg-gray-100 text-gray-500">
            ◦ interpolated
          </span>
        )}
      </div>
    </div>
  );
}

// ── Sidebar ────────────────────────────────────────────────────────────────

function Sidebar({
  vpath, setVpath, vinfo, onLoadVideo,
  names, onRenameRegion, onDeleteRegion,
  keyframes, ts, onSeekTo, onDeleteKf,
  onSaveConfig, onLoadConfig,
}) {
  const [cfgPath, setCfgPath] = useState('regions.json');
  const [cfgMsg, setCfgMsg]   = useState('');

  async function pickAndLoadVideo() {
    const path = await openDialog({
      title: 'Select Video',
      filters: [{ name: 'Video', extensions: ['mp4', 'mov', 'avi', 'mkv', 'm4v', 'webm'] }],
    });
    if (path) { await onLoadVideo(path); }
  }

  async function pickConfigLoad() {
    const path = await openDialog({
      title: 'Open Region Config',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (path) {
      setCfgPath(path);
      try { await onLoadConfig(path); setCfgMsg('Loaded!'); }
      catch (e) { setCfgMsg('Error: ' + e.message); }
      setTimeout(() => setCfgMsg(''), 3000);
    }
  }

  async function pickConfigSave() {
    const path = await saveDialog({
      title: 'Save Region Config',
      defaultPath: cfgPath,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (path) {
      setCfgPath(path);
      try { await onSaveConfig(path); setCfgMsg('Saved!'); }
      catch (e) { setCfgMsg('Error: ' + e.message); }
      setTimeout(() => setCfgMsg(''), 3000);
    }
  }

  return (
    <aside className="w-72 shrink-0 bg-gray-50 border-r border-gray-200 overflow-y-auto flex flex-col gap-2.5 p-2.5">
      {/* Video */}
      <Card>
        <CardTitle>Video</CardTitle>
        <Btn variant="primary" full onClick={pickAndLoadVideo}>
          {vpath ? '⟳ Change video…' : 'Select video…'}
        </Btn>
        {vpath && (
          <span className="text-[11px] text-gray-500 truncate" title={vpath}>
            {vpath.split(/[\\/]/).pop()}
          </span>
        )}
        {vinfo && (
          <span className="text-[11px] text-gray-400">
            {vinfo.width}×{vinfo.height} · {vinfo.fps.toFixed(1)} fps · {vinfo.duration.toFixed(1)}s
          </span>
        )}
      </Card>

      {/* Regions */}
      <Card>
        <CardTitle>Regions</CardTitle>
        {!names.length
          ? <span className="text-xs text-gray-400">Draw on the canvas to create regions.</span>
          : names.map((n, i) => (
            <div key={i} className="flex items-center gap-1">
              <Input
                type="text" value={n}
                onChange={e => onRenameRegion(i, e.target.value)}
                className="!py-1"
              />
              <Btn variant="ghost" onClick={() => onDeleteRegion(n)}>✕</Btn>
            </div>
          ))
        }
      </Card>

      {/* Keyframes */}
      <Card>
        <CardTitle>Keyframes</CardTitle>
        <span className="text-[11px] text-gray-400">Auto-created when you move a region.</span>
        <div className="flex flex-col gap-1">
          {!keyframes.length
            ? <span className="text-xs text-gray-400">No keyframes yet.</span>
            : [...keyframes].sort((a, b) => a.timestamp - b.timestamp).map((kf, i) => {
              const active = Math.abs(kf.timestamp - ts) < 0.001;
              return (
                <div
                  key={i}
                  className={
                    'flex items-center gap-1.5 rounded px-2 py-1 text-xs border ' +
                    (active
                      ? 'bg-green-50 border-green-200 text-green-800'
                      : 'bg-white border-gray-100 text-gray-700')
                  }
                >
                  <span
                    className="flex-1 font-mono cursor-pointer hover:underline"
                    onClick={() => onSeekTo(kf.timestamp)}
                  >
                    t={kf.timestamp.toFixed(2)}s
                  </span>
                  <span className="text-gray-400">{kf.regions.length} pos</span>
                  <Btn variant="ghost" onClick={() => onDeleteKf(kf.timestamp)}>✕</Btn>
                </div>
              );
            })
          }
        </div>
      </Card>

      {/* Config */}
      <Card>
        <CardTitle>Config file</CardTitle>
        <div className="flex gap-1.5">
          <Btn full onClick={pickConfigSave}>Save…</Btn>
          <Btn full onClick={pickConfigLoad}>Load…</Btn>
        </div>
        {cfgMsg && (
          <span className={
            'text-xs ' + (cfgMsg.startsWith('Error') ? 'text-red-500' : 'text-green-600')
          }>
            {cfgMsg}
          </span>
        )}
      </Card>
    </aside>
  );
}

// ── Extract tab ────────────────────────────────────────────────────────────

// Confidence helpers — background tint and bar colour for a confidence value [0,1]
function confBg(c) {
  if (c === undefined || c === null) return undefined;
  if (c >= 0.8) return 'rgba(34,197,94,0.13)';
  if (c >= 0.5) return 'rgba(234,179,8,0.16)';
  if (c > 0)    return 'rgba(239,68,68,0.13)';
  return undefined;
}
function confBar(c) {
  if (c >= 0.8) return '#22c55e';
  if (c >= 0.5) return '#eab308';
  return '#ef4444';
}

function ExtractTab({ vpath, vinfo, keyframes }) {
  const [fpsSample,   setFpsSample]   = useState(30);
  const [lang,        setLang]        = useState('en,de');
  const [preprocess,     setPreprocess]     = useState(true);
  const [filterNumeric,  setFilterNumeric]  = useState(true);
  const [oarThreshold,   setOarThreshold]   = useState(90);
  const [running,     setRunning]     = useState(false);
  const [results,     setResults]     = useState(null);
  const [csvData,     setCsvData]     = useState(null);
  const [progress,    setProgress]    = useState(null);
  // Pivot table state: { [frame]: { frame, timestamp, [regionName]: { value, confidence } } }
  const [liveData,    setLiveData]    = useState({});
  const [liveRegions, setLiveRegions] = useState([]);  // ordered region names
  // { [regionName]: { preview: string, value: string, confidence: number, source: string } }
  const [lastPreviews, setLastPreviews] = useState({});
  const unlistenRef = useRef(null);

  const nRegions = keyframes.length ? Math.max(...keyframes.map(k => k.regions.length), 0) : 0;
  const nFrames  = vinfo ? Math.floor(vinfo.total_frames / fpsSample) : 0;

  async function cancelExtract() {
    try { await invoke('cancel_extract'); } catch (_) {}
  }

  async function startExtract() {
    if (keyframes.length < 2) { alert('Define at least 2 keyframes before extracting.'); return; }
    if (!vpath)               { alert('Load a video first.'); return; }
    const uniqueTs = new Set(keyframes.map(k => k.timestamp));
    if (uniqueTs.size < 2)    { alert('Keyframes must have different timestamps.'); return; }

    setRunning(true);
    setResults(null);
    setCsvData(null);
    setProgress({ elapsed_frames: 0, total: nFrames });
    setLiveData({});
    setLiveRegions([]);
    setLastPreviews({});

    unlistenRef.current = await listen('extraction_progress', event => {
      const p = event.payload;
      // p.regions is an array — one entry per region in this frame
      setProgress({ elapsed_frames: p.elapsed_frames, total: p.total });
      setLastPreviews(prev => {
        const next = { ...prev };
        p.regions.forEach(r => {
          next[r.region_name] = { preview: r.ocr_preview, value: r.value, confidence: r.confidence, source: r.source };
        });
        return next;
      });
      setLiveData(prev => {
        const row = { ...(prev[p.frame] ?? {}), frame: p.frame, timestamp: p.timestamp };
        p.regions.forEach(r => { row[r.region_name] = { value: r.value, confidence: r.confidence, source: r.source }; });
        return { ...prev, [p.frame]: row };
      });
      setLiveRegions(prev => {
        const seen = new Set(prev);
        p.regions.forEach(r => seen.add(r.region_name));
        return seen.size === prev.length ? prev : [...seen];
      });
    });

    try {
      const res = await invoke('extract', {
        params: {
          video_path: vpath,
          config: { video_path: vpath, keyframes },
          fps_sample: fpsSample,
          preprocess,
          filter_numeric: filterNumeric,
          languages: lang.split(',').map(s => s.trim()).filter(Boolean),
          use_gpu: false,
          backend: '',
          oar_confidence_threshold: oarThreshold / 100,
        },
      });
      setResults(res.measurements);
      setCsvData(res.csv);
      setProgress(null);
    } catch (e) {
      alert('Extraction error: ' + e);
      setProgress(null);
    } finally {
      if (unlistenRef.current) { unlistenRef.current(); unlistenRef.current = null; }
      setRunning(false);
    }
  }

  async function exportCsv() {
    if (!csvData) return;
    const path = await saveDialog({
      title: 'Export CSV',
      defaultPath: 'measurements.csv',
      filters: [{ name: 'CSV', extensions: ['csv'] }],
    });
    if (path) {
      try { await invoke('save_csv', { path, csv: csvData }); }
      catch (e) { alert('Export failed: ' + e); }
    }
  }

  const pct = progress && progress.total > 0
    ? Math.min(100, Math.round((progress.elapsed_frames / progress.total) * 100))
    : 0;

  // Rows sorted newest-first for live monitoring; cap at 500 visible rows
  const liveRows = Object.values(liveData)
    .sort((a, b) => b.frame - a.frame)
    .slice(0, 500);

  return (
    <div className="flex flex-col gap-4 p-4 overflow-auto">
      {/* Settings */}
      <Card>
        <CardTitle>Settings</CardTitle>
        <div className="grid grid-cols-2 gap-4">
          <div className="flex flex-col gap-3">
            <div>
              <Label>Sample every N frames</Label>
              <Input type="number" value={fpsSample} min={1} max={300}
                onChange={e => setFpsSample(parseInt(e.target.value) || 30)} />
            </div>
            <div>
              <Label>Languages (comma-separated)</Label>
              <Input type="text" value={lang} onChange={e => setLang(e.target.value)} />
            </div>
          </div>
          <div className="flex flex-col gap-3">
            <label className="flex items-center gap-2 text-sm text-gray-600 cursor-pointer">
              <input type="checkbox" checked={preprocess} onChange={e => setPreprocess(e.target.checked)}
                className="accent-green-600" />
              Preprocess frames before OCR
            </label>
            <label className="flex items-center gap-2 text-sm text-gray-600 cursor-pointer">
              <input type="checkbox" checked={filterNumeric} onChange={e => setFilterNumeric(e.target.checked)}
                className="accent-green-600" />
              Prefer numeric result over higher confidence
            </label>
            <div>
              <Label>oar-ocr fast-path threshold (%)</Label>
              <Input type="number" value={oarThreshold} min={0} max={100}
                onChange={e => setOarThreshold(parseInt(e.target.value) || 90)}
                title="If oar-ocr confidence ≥ this value, Tesseract is skipped" />
            </div>
          </div>
        </div>

        {nRegions > 0 && nFrames > 0 && (
          <span className="text-xs text-gray-400 mt-1">
            ≈ {nRegions} region(s) × {nFrames} frames ≈ {nRegions * nFrames} measurements
          </span>
        )}
      </Card>

      {/* Run / Cancel */}
      <Card>
        <div className="flex gap-2">
          <Btn variant="primary" full onClick={startExtract} disabled={running}>
            {running ? '⏳ Extracting…' : '▶ Start extraction'}
          </Btn>
          {running && (
            <Btn variant="danger" onClick={cancelExtract} title="Cancel extraction">
              ✕ Cancel
            </Btn>
          )}
        </div>
      </Card>

      {/* Progress bar — only while running */}
      {running && progress && (
        <Card>
          <div className="w-full h-2 rounded-full bg-gray-100 overflow-hidden">
            <div className="h-full bg-green-500 transition-all duration-150" style={{ width: `${pct}%` }} />
          </div>
          <div className="flex items-center justify-between text-xs text-gray-500 mt-1">
            <span>
              Frame {progress.elapsed_frames} / {progress.total}
            </span>
            <span className="font-mono tabular-nums">{pct}%</span>
          </div>
          {Object.keys(lastPreviews).length > 0 && (
            <div className="mt-2 flex flex-col gap-1.5">
              <span className="text-[10px] text-gray-400 uppercase tracking-wider">Last OCR input per region</span>
              <div className="flex flex-wrap gap-3">
                {Object.entries(lastPreviews).map(([name, { preview, value, confidence, source }]) => (
                  <div key={name} className="flex flex-col gap-1 min-w-0">
                    <div className="flex items-baseline gap-1.5 flex-wrap">
                      <span className="text-[10px] font-semibold text-gray-500 truncate">{name}</span>
                      <span className="text-[11px] font-mono text-gray-800">{value || '—'}</span>
                      <span className="text-[10px] font-mono" style={{ color: confBar(confidence) }}>
                        {Math.round((confidence ?? 0) * 100)}%
                      </span>
                      {source && (
                        <span className={
                          'text-[9px] px-1 py-0.5 rounded font-medium ' +
                          (source === 'oar-ocr'
                            ? 'bg-blue-100 text-blue-700'
                            : 'bg-gray-100 text-gray-500')
                        }>
                          {source}
                        </span>
                      )}
                    </div>
                    <img
                      src={`data:image/png;base64,${preview}`}
                      alt={`OCR input — ${name}`}
                      className="rounded border border-gray-200 bg-black"
                      style={{ imageRendering: 'pixelated', height: '64px', width: 'auto', maxWidth: '240px' }}
                    />
                  </div>
                ))}
              </div>
            </div>
          )}
        </Card>
      )}

      {/* Live pivot table — visible during and after extraction */}
      {liveRegions.length > 0 && (
        <Card className="p-0 overflow-hidden">
          <div className="flex items-center justify-between px-3 py-2 border-b border-gray-100">
            <CardTitle>
              {running ? `Live results — ${liveRows.length} frames` : `Results — ${liveRows.length} frames`}
            </CardTitle>
            {csvData && <Btn onClick={exportCsv}>↓ Export CSV…</Btn>}
          </div>
          <div className="overflow-auto max-h-[32rem]">
            <table className="w-full text-xs border-collapse">
              <thead className="sticky top-0 z-10 bg-white">
                <tr>
                  <th className="px-3 py-2 text-left font-medium text-gray-400 border-b border-gray-200 whitespace-nowrap">
                    t (s)
                  </th>
                  {liveRegions.map(r => (
                    <th key={r}
                      className="px-3 py-2 text-left font-medium text-gray-600 border-b border-gray-200 whitespace-nowrap">
                      {r}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {liveRows.map(row => (
                  <tr key={row.frame} className="border-b border-gray-50 hover:bg-gray-50/60">
                    <td className="px-3 py-1.5 font-mono text-gray-400 whitespace-nowrap tabular-nums">
                      {row.timestamp?.toFixed(2)}
                    </td>
                    {liveRegions.map(r => {
                      const e = row[r];
                      return (
                        <td key={r} className="px-3 py-1.5"
                          style={{ background: confBg(e?.confidence) }}>
                          {e ? (
                            <>
                              <div className="font-mono font-semibold text-gray-800 leading-tight">
                                {e.value || '—'}
                              </div>
                              <div className="mt-1 h-[3px] rounded-full bg-gray-200 overflow-hidden w-16">
                                <div style={{
                                  height: '100%',
                                  width: `${Math.round((e.confidence ?? 0) * 100)}%`,
                                  background: confBar(e.confidence ?? 0),
                                  borderRadius: '9999px',
                                }} />
                              </div>
                              {e.source === 'oar-ocr' && (
                                <div className="mt-0.5 text-[9px] text-blue-500 font-medium">oar</div>
                              )}
                            </>
                          ) : (
                            <span className="text-gray-300">—</span>
                          )}
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </div>
  );
}

// ── App root ───────────────────────────────────────────────────────────────

export default function App() {
  const [vpath,     setVpath]     = useState('');
  const [vinfo,     setVinfo]     = useState(null);
  const [names,     setNames]     = useState([]);
  const [keyframes, setKeyframes] = useState([]);
  const [ts,        setTs]        = useState(0);
  const [activeTab, setActiveTab] = useState('configure');

  const canvasRef = useRef(null);

  async function loadVideo(path) {
    const p = (path ?? vpath).trim();
    if (!p) return;
    try {
      const info = await invoke('get_video_info', { path: p });
      setVpath(p);
      setVinfo(info);
      setTs(0);
    } catch (e) {
      setVinfo(null);
      alert('Error loading video: ' + e);
    }
  }

  // ── Region events ─────────────────────────────────────────────────────────

  const handleRegionDrawn = useCallback((videoRect, name) => {
    const tRounded = parseFloat(ts.toFixed(3));
    const newNames = [...names, name];
    setNames(newNames);
    setKeyframes(kfs => {
      const updatedKfs = kfs.map(kf => ({
        ...kf,
        regions: [...kf.regions, { name, ...videoRect }],
      }));
      const hasKfAtTs = updatedKfs.some(kf => kf.timestamp === tRounded);
      if (hasKfAtTs) return updatedKfs;
      const pos = interpolate(updatedKfs, tRounded);
      pos[name] = videoRect;
      const regions = newNames.filter(n => n in pos).map(n => ({ name: n, ...pos[n] }));
      return [...updatedKfs, { timestamp: tRounded, regions }]
        .sort((a, b) => a.timestamp - b.timestamp);
    });
  }, [ts, names]);

  const handleRegionMoved = useCallback((name, videoRect) => {
    const tRounded = parseFloat(ts.toFixed(3));
    setKeyframes(kfs => {
      const pos = interpolate(kfs, tRounded);
      pos[name] = videoRect;
      const regions = names.filter(n => n in pos).map(n => ({ name: n, ...pos[n] }));
      const idx = kfs.findIndex(kf => kf.timestamp === tRounded);
      if (idx >= 0) return kfs.map((kf, i) => i === idx ? { ...kf, regions } : kf);
      return [...kfs, { timestamp: tRounded, regions }]
        .sort((a, b) => a.timestamp - b.timestamp);
    });
  }, [ts, names]);

  const handleRegionDeleted = useCallback((name) => {
    setNames(ns => ns.filter(n => n !== name));
    setKeyframes(kfs => kfs.map(kf => ({
      ...kf, regions: kf.regions.filter(r => r.name !== name),
    })));
  }, []);

  function renameRegion(idx, newName) {
    const oldName = names[idx];
    setNames(ns => ns.map((n, i) => i === idx ? newName : n));
    setKeyframes(kfs => kfs.map(kf => ({
      ...kf,
      regions: kf.regions.map(r => r.name === oldName ? { ...r, name: newName } : r),
    })));
  }

  function deleteKf(timestamp) {
    setKeyframes(kfs => kfs.filter(kf => kf.timestamp !== timestamp));
  }

  async function saveConfig(path) {
    await invoke('save_config', {
      path,
      config: { video_path: vpath, keyframes },
    });
  }

  async function loadConfig(path) {
    const cfg = await invoke('load_config', { path });
    const kfs = cfg.keyframes || [];
    setKeyframes(kfs);
    const seen = new Set(); const ns = [];
    kfs.forEach(kf => kf.regions.forEach(r => {
      if (!seen.has(r.name)) { seen.add(r.name); ns.push(r.name); }
    }));
    setNames(ns);
    if (cfg.video_path) setVpath(cfg.video_path);
  }

  return (
    <div className="flex flex-col h-screen bg-gray-100 font-[system-ui,sans-serif] select-none">
      <div className="flex flex-1 overflow-hidden">
        <Sidebar
          vpath={vpath} setVpath={setVpath}
          vinfo={vinfo} onLoadVideo={loadVideo}
          names={names}
          onRenameRegion={renameRegion}
          onDeleteRegion={handleRegionDeleted}
          keyframes={keyframes}
          ts={ts}
          onSeekTo={setTs}
          onDeleteKf={deleteKf}
          onSaveConfig={saveConfig}
          onLoadConfig={loadConfig}
        />

        <main className="flex-1 flex flex-col overflow-hidden">
          {/* Tabs */}
          <div className="shrink-0 flex border-b border-gray-200 bg-white">
            {[
              { id: 'configure', label: '🎬 Configure Regions' },
              { id: 'extract',   label: '⚙ Extract' },
            ].map(t => (
              <button
                key={t.id}
                onClick={() => setActiveTab(t.id)}
                className={
                  'px-4 py-2.5 text-sm font-medium border-b-2 transition-colors ' +
                  (activeTab === t.id
                    ? 'border-green-500 text-green-700'
                    : 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-200')
                }
              >
                {t.label}
              </button>
            ))}
          </div>

          {/* Both tabs stay mounted — display:none preserves state across switches */}
          <div
            className="flex-1 overflow-hidden flex flex-col gap-4 p-4"
            style={{ display: activeTab === 'configure' ? 'flex' : 'none' }}
          >
            <Card>
              <SeekBar ts={ts} vinfo={vinfo} keyframes={keyframes} onChange={setTs} />
            </Card>
            <CanvasPanel
              ref={canvasRef}
              names={names}
              keyframes={keyframes}
              ts={ts}
              vpath={vpath}
              vinfo={vinfo}
              onRegionDrawn={handleRegionDrawn}
              onRegionMoved={handleRegionMoved}
              onRegionDeleted={handleRegionDeleted}
            />
          </div>

          <div
            className="flex-1 overflow-auto"
            style={{ display: activeTab === 'extract' ? 'flex' : 'none', flexDirection: 'column' }}
          >
            <ExtractTab vpath={vpath} vinfo={vinfo} keyframes={keyframes} />
          </div>
        </main>
      </div>
    </div>
  );
}
