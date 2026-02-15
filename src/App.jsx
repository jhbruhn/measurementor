import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import CanvasPanel from './components/CanvasPanel.jsx';
import { Btn, Card, CardTitle, Input, Label, Select } from './components/ui.jsx';
import { interpolate } from './helpers.js';
import './App.css';

// ── Seek bar ───────────────────────────────────────────────────────────────

function SeekBar({ ts, vinfo, keyframes, onChange }) {
  const max  = vinfo?.duration || 1;
  const step = vinfo ? (1 / vinfo.fps).toFixed(4) : 0.033;
  const onKf = keyframes.some(kf => Math.abs(kf.timestamp - ts) < 0.001);

  const sorted = [...keyframes].sort((a, b) => a.timestamp - b.timestamp);
  const startTs = sorted[0]?.timestamp;
  const endTs   = sorted[sorted.length - 1]?.timestamp;

  return (
    <div className="flex flex-col gap-1.5">
      <div className="relative">
        <div className="absolute inset-0 pointer-events-none" aria-hidden="true">
          {keyframes.map((kf, i) => {
            const isStart = sorted.length >= 2 && kf.timestamp === startTs;
            const isEnd   = sorted.length >= 2 && kf.timestamp === endTs;
            return (
              <div
                key={i}
                className={'seek-tick' + (isStart ? ' seek-tick-start' : isEnd ? ' seek-tick-end' : '')}
                style={{ left: `${(kf.timestamp / max) * 100}%` }}
                title={isStart ? `Start at ${kf.timestamp.toFixed(2)}s` : isEnd ? `End at ${kf.timestamp.toFixed(2)}s` : `Keyframe at ${kf.timestamp.toFixed(2)}s`}
              />
            );
          })}
        </div>
        <input
          type="range"
          className="relative w-full accent-green-600"
          min={0} max={max.toFixed(3)} step={step}
          value={ts}
          aria-label="Seek position"
          onChange={e => onChange(parseFloat(e.target.value))}
        />
      </div>
      <div className="flex items-center gap-2">
        <span className="text-xs font-mono text-gray-600 tabular-nums">{ts.toFixed(2)} s</span>
        {onKf && (
          <span className="text-xs px-1.5 py-0.5 rounded-full bg-green-100 text-green-700 font-medium">
            ● keyframe
          </span>
        )}
        {!onKf && keyframes.length > 0 && (
          <span className="text-xs px-1.5 py-0.5 rounded-full bg-gray-100 text-gray-500">
            ◦ interpolated
          </span>
        )}
      </div>
    </div>
  );
}

// ── Sidebar ────────────────────────────────────────────────────────────────

function Sidebar({
  vpath, vinfo, onLoadVideo, videoError,
  names, onRenameRegion, onDeleteRegion,
  expectations, onSetExpectation,
  keyframes, ts, onSeekTo, onDeleteKf,
  onSaveConfig, onLoadConfig,
  isDirty,
}) {
  const [cfgPath, setCfgPath]         = useState('regions.json');
  const [cfgMsg, setCfgMsg]           = useState('');
  const [expandedRegion, setExpanded] = useState(null);
  const [showKfHelp, setShowKfHelp]   = useState(false);

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
          <span className="text-xs text-gray-500 truncate" title={vpath}>
            {vpath.split(/[\\/]/).pop()}
          </span>
        )}
        {vinfo && (
          <span className="text-xs text-gray-400">
            {vinfo.width}×{vinfo.height} · {vinfo.fps.toFixed(1)} fps · {vinfo.duration.toFixed(1)}s
          </span>
        )}
        {videoError && (
          <div className="rounded border border-red-200 bg-red-50 px-2 py-1.5 text-xs text-red-700">
            {videoError}
          </div>
        )}
      </Card>

      {/* Regions */}
      <Card>
        <CardTitle>Regions</CardTitle>
        {!names.length
          ? <span className="text-xs text-gray-400">Draw on the canvas to create regions.</span>
          : names.map((n, i) => {
            const exp      = expectations[n] || {};
            const expanded = expandedRegion === n;
            const toggle   = () => setExpanded(expanded ? null : n);
            const set      = (field, val) => onSetExpectation(n, field, val);
            const panelId  = `region-panel-${i}`;
            return (
              <div key={i} className="border border-gray-100 rounded overflow-hidden">
                {/* Header row */}
                <div className="flex items-center gap-1 px-1 py-0.5">
                  <button
                    onClick={toggle}
                    aria-expanded={expanded}
                    aria-controls={panelId}
                    className="text-xs text-gray-400 hover:text-gray-600 w-4 shrink-0 text-center focus:outline-none focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-offset-1 rounded"
                    title={expanded ? 'Collapse' : 'Configure expectations'}
                  >
                    {expanded ? '▼' : '▶'}
                  </button>
                  <Input
                    type="text" value={n}
                    onChange={e => onRenameRegion(i, e.target.value)}
                    className="!py-0.5"
                  />
                  <Btn variant="ghost" onClick={() => onDeleteRegion(n)}>✕</Btn>
                </div>

                {/* Expectations panel */}
                {expanded && (
                  <div id={panelId} className="bg-gray-50/80 border-t border-gray-100 px-2 py-2 flex flex-col gap-2">
                    <label className="flex items-start gap-2 text-xs text-gray-600 cursor-pointer select-none">
                      <input
                        type="checkbox" checked={!!exp.numeric}
                        onChange={e => set('numeric', e.target.checked)}
                        className="accent-green-600 mt-0.5 shrink-0"
                      />
                      <span>
                        Numeric region
                        <span className="block text-xs text-gray-400 font-normal">
                          OCR will prefer numeric readings; enables range, digit, and deviation constraints below.
                        </span>
                      </span>
                    </label>

                    {exp.numeric && (<>
                      {/* Range */}
                      <div>
                        <span className="text-xs font-medium text-gray-400 uppercase tracking-wider">Value range</span>
                        <div className="grid grid-cols-2 gap-1 mt-1">
                          <div>
                            <Label>Min</Label>
                            <Input type="number" value={exp.min ?? ''} placeholder="–∞"
                              onChange={e => set('min', e.target.value)} className="!py-0.5" />
                          </div>
                          <div>
                            <Label>Max</Label>
                            <Input type="number" value={exp.max ?? ''} placeholder="+∞"
                              onChange={e => set('max', e.target.value)} className="!py-0.5" />
                          </div>
                        </div>
                      </div>

                      {/* Digit structure */}
                      <div>
                        <span className="text-xs font-medium text-gray-400 uppercase tracking-wider">Digit structure</span>
                        <div className="grid grid-cols-2 gap-1 mt-1">
                          <div>
                            <Label>Total digits</Label>
                            <Input type="number" min={1} max={12}
                              value={exp.total_digits ?? ''} placeholder="any"
                              onChange={e => set('total_digits', e.target.value)} className="!py-0.5" />
                          </div>
                          <div>
                            <Label>Decimal places</Label>
                            <Input type="number" min={0} max={6}
                              value={exp.decimal_places ?? ''} placeholder="any"
                              onChange={e => set('decimal_places', e.target.value)} className="!py-0.5" />
                          </div>
                        </div>
                      </div>

                      {/* Deviation */}
                      <div>
                        <Label>Max change per sample</Label>
                        <Input type="number" min={0}
                          value={exp.max_deviation ?? ''} placeholder="unlimited"
                          onChange={e => set('max_deviation', e.target.value)} className="!py-0.5" />
                      </div>
                    </>)}
                  </div>
                )}
              </div>
            );
          })
        }
      </Card>

      {/* Keyframes */}
      <Card>
        <div className="flex items-center gap-1.5">
          <CardTitle>Keyframes</CardTitle>
          <button
            onClick={() => setShowKfHelp(v => !v)}
            aria-expanded={showKfHelp}
            aria-label="How keyframes work"
            className="ml-auto text-xs text-gray-400 hover:text-gray-600 w-5 h-5 rounded-full border border-gray-200 flex items-center justify-center focus:outline-none focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-offset-1"
            title="How keyframes work"
          >
            ?
          </button>
        </div>
        {showKfHelp && (
          <div className="rounded border border-blue-100 bg-blue-50 px-2 py-2 text-xs text-blue-800 leading-relaxed">
            Keyframes record region positions at a specific moment. <strong>Move a region at time A</strong>, scrub to time B and move it again. The tool interpolates positions between keyframes.
            <br /><br />
            At least 2 keyframes at different timestamps are required before extracting.
          </div>
        )}
        <div className="flex flex-col gap-1">
          {!keyframes.length
            ? <span className="text-xs text-gray-400">No keyframes yet. Move a region to create one.</span>
            : (() => {
                const sorted = [...keyframes].sort((a, b) => a.timestamp - b.timestamp);
                const hasRange = sorted.length >= 2;
                return sorted.map((kf, i) => {
                  const active   = Math.abs(kf.timestamp - ts) < 0.001;
                  const isStart  = hasRange && i === 0;
                  const isEnd    = hasRange && i === sorted.length - 1;
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
                      {isStart && (
                        <span className="shrink-0 px-1 py-0.5 rounded text-xs font-semibold bg-green-100 text-green-700 uppercase tracking-wide">
                          Start
                        </span>
                      )}
                      {isEnd && (
                        <span className="shrink-0 px-1 py-0.5 rounded text-xs font-semibold bg-red-100 text-red-700 uppercase tracking-wide">
                          End
                        </span>
                      )}
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
                });
              })()
          }
        </div>
      </Card>

      {/* Config */}
      <Card>
        <div className="flex items-center gap-1">
          <CardTitle>Config file</CardTitle>
          {isDirty && (
            <span className="ml-1 text-xs text-amber-500" title="Unsaved changes">●</span>
          )}
        </div>
        <div className="flex gap-1.5">
          <Btn full onClick={pickConfigSave}>Save…</Btn>
          <Btn full onClick={pickConfigLoad}>Load…</Btn>
        </div>
        <span
          role="status"
          aria-live="polite"
          className={'text-xs ' + (cfgMsg.startsWith('Error') ? 'text-red-500' : 'text-green-600')}
        >
          {cfgMsg}
        </span>
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

function SortTh({ col, label, sortCol, sortDir, onSort }) {
  const active = sortCol === col;
  return (
    <th
      scope="col"
      onClick={() => onSort(col)}
      className={
        'px-3 py-2 text-left font-medium border-b border-gray-200 whitespace-nowrap ' +
        'cursor-pointer select-none hover:bg-gray-50 ' +
        (active ? 'text-gray-700' : 'text-gray-400')
      }
    >
      {label}
      {active && <span className="ml-1 text-gray-500">{sortDir === 'asc' ? '↑' : '↓'}</span>}
    </th>
  );
}

function ExtractTab({ vpath, vinfo, keyframes, expectations }) {
  const [fpsSample,     setFpsSample]     = useState(30);
  const [lang,          setLang]          = useState('en,de');
  const [preprocess,    setPreprocess]    = useState(true);
  const [oarThreshold,  setOarThreshold]  = useState(90);
  const [showAdvanced,  setShowAdvanced]  = useState(false);
  const [running,       setRunning]       = useState(false);
  const [results,       setResults]       = useState(null);
  const [csvData,       setCsvData]       = useState(null);
  const [progress,      setProgress]      = useState(null);
  const [extractError,  setExtractError]  = useState('');
  const [exportError,   setExportError]   = useState('');
  const [sortCol,       setSortCol]       = useState('ts');
  const [sortDir,       setSortDir]       = useState('asc');

  // Pivot table state: { [frame]: { frame, timestamp, [regionName]: { value, confidence } } }
  const [liveData,     setLiveData]     = useState({});
  const [liveRegions,  setLiveRegions]  = useState([]);
  const [lastPreviews, setLastPreviews] = useState({});
  const unlistenRef = useRef(null);

  // R18: reset sample rate default when video changes
  useEffect(() => {
    if (vinfo) {
      setFpsSample(Math.round(vinfo.fps));
    }
  }, [vinfo]);

  const nRegions = keyframes.length ? Math.max(...keyframes.map(k => k.regions.length), 0) : 0;
  const nFrames  = vinfo ? Math.floor(vinfo.total_frames / fpsSample) : 0;

  function handleSort(col) {
    if (sortCol === col) {
      setSortDir(d => d === 'asc' ? 'desc' : 'asc');
    } else {
      setSortCol(col);
      setSortDir('asc');
    }
  }

  async function cancelExtract() {
    try { await invoke('cancel_extract'); } catch (_) {}
  }

  async function startExtract() {
    setExtractError('');
    if (!vpath) {
      setExtractError('Load a video first.');
      return;
    }
    if (keyframes.length < 2) {
      setExtractError('Define at least 2 keyframes before extracting.');
      return;
    }
    const uniqueTs = new Set(keyframes.map(k => k.timestamp));
    if (uniqueTs.size < 2) {
      setExtractError('Keyframes must have different timestamps.');
      return;
    }

    setRunning(true);
    setResults(null);
    setCsvData(null);
    setProgress({ elapsed_frames: 0, total: nFrames });
    setLiveData({});
    setLiveRegions([]);
    setLastPreviews({});

    unlistenRef.current = await listen('extraction_progress', event => {
      const p = event.payload;
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
          config: { video_path: vpath, keyframes, expectations: buildBackendExpectations(expectations) },
          fps_sample: fpsSample,
          preprocess,
          languages: lang.split(',').map(s => s.trim()).filter(Boolean),
          oar_confidence_threshold: oarThreshold / 100,
        },
      });
      setResults(res.measurements);
      setCsvData(res.csv);
      setProgress(null);
    } catch (e) {
      setExtractError('Extraction error: ' + e);
      setProgress(null);
    } finally {
      if (unlistenRef.current) { unlistenRef.current(); unlistenRef.current = null; }
      setRunning(false);
    }
  }

  async function exportCsv() {
    if (!csvData) return;
    setExportError('');
    const path = await saveDialog({
      title: 'Export CSV',
      defaultPath: 'measurements.csv',
      filters: [{ name: 'CSV', extensions: ['csv'] }],
    });
    if (path) {
      try { await invoke('save_csv', { path, csv: csvData }); }
      catch (e) { setExportError('Export failed: ' + e); }
    }
  }

  const pct = progress && progress.total > 0
    ? Math.min(100, Math.round((progress.elapsed_frames / progress.total) * 100))
    : 0;

  const liveRows = Object.values(liveData)
    .sort((a, b) => {
      let cmp;
      if (sortCol === 'ts') {
        cmp = a.frame - b.frame;
      } else {
        const av = a[sortCol]?.value ?? '';
        const bv = b[sortCol]?.value ?? '';
        cmp = av < bv ? -1 : av > bv ? 1 : 0;
      }
      return sortDir === 'asc' ? cmp : -cmp;
    })
    .slice(0, 500);

  const needsKeyframes = keyframes.length < 2;

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
                onChange={e => setFpsSample(parseInt(e.target.value) || 1)} />
              {vinfo && (
                <span className="text-xs text-gray-400 mt-0.5 block">
                  ≈ {(fpsSample / vinfo.fps).toFixed(2)} s between samples at {vinfo.fps.toFixed(1)} fps
                </span>
              )}
            </div>
            <div>
              <Label>Languages (comma-separated)</Label>
              <Input type="text" value={lang} onChange={e => setLang(e.target.value)} />
            </div>
          </div>
          <div className="flex flex-col gap-3 justify-center">
            <label className="flex items-center gap-2 text-sm text-gray-600 cursor-pointer">
              <input type="checkbox" checked={preprocess} onChange={e => setPreprocess(e.target.checked)}
                className="accent-green-600" />
              Preprocess frames before OCR
            </label>
          </div>
        </div>

        {/* Advanced */}
        <button
          className="flex items-center gap-1 text-xs text-gray-400 hover:text-gray-600 mt-1 select-none focus:outline-none focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-offset-1 rounded"
          onClick={() => setShowAdvanced(v => !v)}
          aria-expanded={showAdvanced}
          aria-controls="advanced-settings"
        >
          {showAdvanced ? '▼' : '▶'} Advanced
        </button>
        {showAdvanced && (
          <div id="advanced-settings" className="mt-2">
            <Label>Fast-path confidence threshold (%)</Label>
            <Input type="number" value={oarThreshold} min={0} max={100}
              onChange={e => setOarThreshold(parseInt(e.target.value) || 90)}
              title="If the primary OCR engine's confidence reaches this value, the secondary engine is skipped" />
            <span className="text-xs text-gray-400 mt-0.5 block">
              Primary OCR result is used directly when confidence ≥ this value; lower values force more cross-checking.
            </span>
          </div>
        )}

        {nRegions > 0 && nFrames > 0 && (
          <span className="text-xs text-gray-400 mt-1">
            ≈ {nRegions} region(s) × {nFrames} frames ≈ {nRegions * nFrames} measurements
          </span>
        )}
      </Card>

      {/* Keyframe warning */}
      {needsKeyframes && (
        <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2.5 text-sm text-amber-800">
          Add at least 2 keyframes with different timestamps in the <strong>Configure Regions</strong> tab before extracting.
        </div>
      )}

      {/* Run / Cancel */}
      <Card>
        {extractError && (
          <div className="rounded border border-red-200 bg-red-50 px-2 py-1.5 text-xs text-red-700">
            {extractError}
          </div>
        )}
        <div className="flex gap-2">
          <Btn variant="primary" full onClick={startExtract} disabled={running}>
            {running ? '⏳ Extracting…' : 'Start extraction'}
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
            <div className="h-full bg-green-500 transition-all duration-150" style={{ width: `${pct}%` }}
              role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}
              aria-label="Extraction progress" />
          </div>
          <div className="flex items-center justify-between text-xs text-gray-500 mt-1">
            <span>Frame {progress.elapsed_frames} / {progress.total}</span>
            <span className="font-mono tabular-nums">{pct}%</span>
          </div>
          {Object.keys(lastPreviews).length > 0 && (
            <div className="mt-2 flex flex-col gap-1.5">
              <span className="text-xs text-gray-400 uppercase tracking-wider">Last OCR input per region</span>
              <div className="flex flex-wrap gap-3">
                {Object.entries(lastPreviews).map(([name, { preview, value, confidence, source }]) => (
                  <div key={name} className="flex flex-col gap-1 min-w-0">
                    <div className="flex items-baseline gap-1.5 flex-wrap">
                      <span className="text-xs font-semibold text-gray-500 truncate">{name}</span>
                      <span className="text-xs font-mono text-gray-800">{value || '—'}</span>
                      <span className="text-xs font-mono" style={{ color: confBar(confidence) }}>
                        {Math.round((confidence ?? 0) * 100)}%
                      </span>
                      {source && (
                        <span className={
                          'text-xs px-1 py-0.5 rounded font-medium ' +
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
            <div className="flex items-center gap-2">
              {exportError && (
                <span className="text-xs text-red-600">{exportError}</span>
              )}
              {csvData && <Btn onClick={exportCsv}>↓ Export CSV…</Btn>}
            </div>
          </div>
          <div className="overflow-auto max-h-[32rem] select-text">
            <table className="w-full text-xs border-collapse">
              <thead className="sticky top-0 z-10 bg-white">
                <tr>
                  <SortTh col="ts" label="t (s)" sortCol={sortCol} sortDir={sortDir} onSort={handleSort} />
                  {liveRegions.map(r => (
                    <SortTh key={r} col={r} label={r} sortCol={sortCol} sortDir={sortDir} onSort={handleSort} />
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
                      const confPct = Math.round((e?.confidence ?? 0) * 100);
                      return (
                        <td key={r} className="px-3 py-1.5"
                          style={{ background: confBg(e?.confidence) }}
                          title={e ? `${e.value || '—'} — ${confPct}% confidence${e.source ? ` (${e.source})` : ''}` : undefined}>
                          {e ? (
                            <>
                              <div className="font-mono font-semibold text-gray-800 leading-tight">
                                {e.value || '—'}
                              </div>
                              <div
                                className="mt-1 h-[3px] rounded-full bg-gray-200 overflow-hidden w-16"
                                role="meter"
                                aria-valuenow={confPct}
                                aria-valuemin={0}
                                aria-valuemax={100}
                                aria-label={`${confPct}% confidence`}
                              >
                                <div style={{
                                  height: '100%',
                                  width: `${confPct}%`,
                                  background: confBar(e.confidence ?? 0),
                                  borderRadius: '9999px',
                                }} />
                              </div>
                              <span className="sr-only">{confPct}% confidence</span>
                              {e.source === 'oar-ocr' && (
                                <div className="mt-0.5 text-xs text-blue-500 font-medium">oar</div>
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

// ── Helpers for expectations ────────────────────────────────────────────────

/** Convert frontend expectation map (string fields) to the Rust-friendly shape. */
function buildBackendExpectations(exps) {
  const parseF = v => (v !== '' && v != null && !isNaN(+v)) ? +v : null;
  const parseI = v => (v !== '' && v != null && !isNaN(parseInt(v, 10))) ? parseInt(v, 10) : null;
  const out = {};
  for (const [name, exp] of Object.entries(exps)) {
    if (!exp?.numeric) continue;
    out[name] = {
      numeric:        true,
      min:            parseF(exp.min),
      max:            parseF(exp.max),
      decimal_places: parseI(exp.decimal_places),
      total_digits:   parseI(exp.total_digits),
      max_deviation:  parseF(exp.max_deviation),
    };
  }
  return out;
}

/** Convert backend expectations (null-valued fields) back to frontend string form. */
function parseBackendExpectations(backendExps) {
  if (!backendExps) return {};
  return Object.fromEntries(
    Object.entries(backendExps).map(([name, exp]) => [name, {
      numeric:        exp.numeric ?? false,
      min:            exp.min          != null ? String(exp.min)          : '',
      max:            exp.max          != null ? String(exp.max)          : '',
      decimal_places: exp.decimal_places != null ? String(exp.decimal_places) : '',
      total_digits:   exp.total_digits   != null ? String(exp.total_digits)   : '',
      max_deviation:  exp.max_deviation  != null ? String(exp.max_deviation)  : '',
    }])
  );
}

// ── App root ───────────────────────────────────────────────────────────────

export default function App() {
  const [vpath,        setVpath]        = useState('');
  const [vinfo,        setVinfo]        = useState(null);
  const [names,        setNames]        = useState([]);
  const [keyframes,    setKeyframes]    = useState([]);
  const [expectations, setExpectations] = useState({});
  const [ts,           setTs]           = useState(0);
  const [activeTab,    setActiveTab]    = useState('configure');
  const [videoError,   setVideoError]   = useState('');
  const [toastMsg,     setToastMsg]     = useState('');
  const [savedSnapshot, setSavedSnapshot] = useState(null);
  const toastTimerRef = useRef(null);
  const canvasRef     = useRef(null);

  // isDirty: true when state has changed since last save/load
  const isDirty = savedSnapshot !== null &&
    JSON.stringify({ names, keyframes, expectations }) !== savedSnapshot;

  const showToast = useCallback((msg) => {
    setToastMsg(msg);
    clearTimeout(toastTimerRef.current);
    toastTimerRef.current = setTimeout(() => setToastMsg(''), 1500);
  }, []);

  function setExpectation(name, field, value) {
    setExpectations(prev => ({
      ...prev,
      [name]: { ...(prev[name] || {}), [field]: value },
    }));
  }

  async function loadVideo(path) {
    const p = (path ?? vpath).trim();
    if (!p) return;
    setVideoError('');
    try {
      const info = await invoke('get_video_info', { path: p });
      setVpath(p);
      setVinfo(info);
      setTs(0);
    } catch (e) {
      setVinfo(null);
      setVideoError('Error loading video: ' + e);
    }
  }

  // ── Region events ──────────────────────────────────────────────────────────

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
    const willCreate = !keyframes.some(kf => kf.timestamp === tRounded);
    setKeyframes(kfs => {
      const pos = interpolate(kfs, tRounded);
      pos[name] = videoRect;
      const regions = names.filter(n => n in pos).map(n => ({ name: n, ...pos[n] }));
      const idx = kfs.findIndex(kf => kf.timestamp === tRounded);
      if (idx >= 0) return kfs.map((kf, i) => i === idx ? { ...kf, regions } : kf);
      return [...kfs, { timestamp: tRounded, regions }]
        .sort((a, b) => a.timestamp - b.timestamp);
    });
    if (willCreate) showToast(`Keyframe saved at t=${tRounded.toFixed(2)}s`);
  }, [ts, names, keyframes, showToast]);

  const handleRegionDeleted = useCallback((name) => {
    setNames(ns => ns.filter(n => n !== name));
    setKeyframes(kfs => kfs.map(kf => ({
      ...kf, regions: kf.regions.filter(r => r.name !== name),
    })));
    setExpectations(prev => { const { [name]: _, ...rest } = prev; return rest; });
  }, []);

  function renameRegion(idx, newName) {
    const oldName = names[idx];
    setNames(ns => ns.map((n, i) => i === idx ? newName : n));
    setKeyframes(kfs => kfs.map(kf => ({
      ...kf,
      regions: kf.regions.map(r => r.name === oldName ? { ...r, name: newName } : r),
    })));
    setExpectations(prev => {
      if (!(oldName in prev)) return prev;
      const { [oldName]: exp, ...rest } = prev;
      return { ...rest, [newName]: exp };
    });
  }

  function deleteKf(timestamp) {
    setKeyframes(kfs => kfs.filter(kf => kf.timestamp !== timestamp));
  }

  async function saveConfig(path) {
    await invoke('save_config', {
      path,
      config: {
        video_path: vpath,
        keyframes,
        expectations: buildBackendExpectations(expectations),
      },
    });
    setSavedSnapshot(JSON.stringify({ names, keyframes, expectations }));
  }

  async function loadConfig(path) {
    const cfg = await invoke('load_config', { path });
    const kfs = cfg.keyframes || [];
    const seen = new Set(); const ns = [];
    kfs.forEach(kf => kf.regions.forEach(r => {
      if (!seen.has(r.name)) { seen.add(r.name); ns.push(r.name); }
    }));
    const exps = parseBackendExpectations(cfg.expectations);
    setKeyframes(kfs);
    setNames(ns);
    if (cfg.video_path) setVpath(cfg.video_path);
    setExpectations(exps);
    setSavedSnapshot(JSON.stringify({ names: ns, keyframes: kfs, expectations: exps }));
  }

  const tabs = [
    { id: 'configure', label: 'Configure Regions' },
    { id: 'extract',   label: 'Extract' },
  ];

  return (
    <div className="flex flex-col h-screen bg-gray-100 font-[system-ui,sans-serif] select-none">
      <div className="flex flex-1 overflow-hidden">
        <Sidebar
          vpath={vpath}
          vinfo={vinfo}
          onLoadVideo={loadVideo}
          videoError={videoError}
          names={names}
          onRenameRegion={renameRegion}
          onDeleteRegion={handleRegionDeleted}
          expectations={expectations}
          onSetExpectation={setExpectation}
          keyframes={keyframes}
          ts={ts}
          onSeekTo={setTs}
          onDeleteKf={deleteKf}
          onSaveConfig={saveConfig}
          onLoadConfig={loadConfig}
          isDirty={isDirty}
        />

        <main className="flex-1 flex flex-col overflow-hidden">
          {/* Tabs */}
          <div
            role="tablist"
            aria-label="Main sections"
            className="shrink-0 flex border-b border-gray-200 bg-white"
          >
            {tabs.map(t => (
              <button
                key={t.id}
                role="tab"
                aria-selected={activeTab === t.id}
                aria-controls={`tabpanel-${t.id}`}
                id={`tab-${t.id}`}
                onClick={() => setActiveTab(t.id)}
                className={
                  'px-4 py-2.5 text-sm font-medium border-b-2 transition-colors ' +
                  'focus:outline-none focus-visible:ring-2 focus-visible:ring-green-500 focus-visible:ring-inset ' +
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
            role="tabpanel"
            id="tabpanel-configure"
            aria-labelledby="tab-configure"
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
            role="tabpanel"
            id="tabpanel-extract"
            aria-labelledby="tab-extract"
            className="flex-1 overflow-auto"
            style={{ display: activeTab === 'extract' ? 'flex' : 'none', flexDirection: 'column' }}
          >
            <ExtractTab vpath={vpath} vinfo={vinfo} keyframes={keyframes} expectations={expectations} />
          </div>
        </main>
      </div>

      {/* Toast notification */}
      {toastMsg && (
        <div
          role="status"
          aria-live="polite"
          className="fixed top-4 right-4 z-50 bg-gray-800 text-white text-sm px-4 py-2 rounded-lg shadow-lg pointer-events-none"
        >
          {toastMsg}
        </div>
      )}
    </div>
  );
}
