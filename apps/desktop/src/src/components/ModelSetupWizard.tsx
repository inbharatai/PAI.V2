import { useEffect, useRef, useState } from 'react';
import { LOCAL_STATE_TEXT, fetchAssessment, gib, observation, type Assessment, type DeviceFactsData, type LocalModelReport, type LocalState } from './localModelLane';

/** Measured facts only: RAM total/available, VRAM if detected, OS/ABI, storage. No tokens/sec. */
export function DeviceFacts({ device: p }: { device: DeviceFactsData }) {
  return <>
    <div className="hw-profile">
      {[
        ['System', `${observation(p.os)} · ${observation(p.os_version)} · ${observation(p.abi)}`],
        ['Total RAM', observation(p.total_ram_bytes, gib)],
        ['Available RAM now', observation(p.available_ram_bytes, gib)],
        ['GPU identity', observation(p.gpu_name)],
        ['Total GPU memory', observation(p.total_vram_bytes, gib)],
        ['Available GPU memory now', observation(p.available_vram_bytes, gib)],
        ['Free installation storage', observation(p.usable_storage_bytes, gib)],
      ].map(([label, value]) => <div className="hw-profile-card" key={label}><h4>{label}</h4><div>{value}</div></div>)}
    </div>
    <h4>Runtime backend check</h4>
    <ul>{p.backends.map(b => <li key={b.backend}>{b.backend.toUpperCase()}: {b.health.provenance === 'UNKNOWN' ? 'Unknown — not runtime-tested' : b.health.value === 'DETECTED_ONLY' ? 'Detected only — not load-tested' : observation(b.health)}</li>)}</ul>
    <p>A GPU name or driver utility does not prove that a model can run. Available memory, runtime, context, speech and vision overhead all need a qualified check. No device-specific speed has been measured.</p>
  </>;
}

export function LocalStateBadge({ state }: { state: LocalState }) {
  return <span className={`hw-badge ${state === 'UNKNOWN' ? 'unavailable' : 'available'}`} data-local-state={state}>{LOCAL_STATE_TEXT[state].title}</span>;
}

/** Declared model files already present in the selected root, in three honest states. */
export function LocalModelsLane({ models }: { models: LocalModelReport[] }) {
  return <section aria-label="Local models on this machine">
    <h4>Local models on this machine</h4>
    {models.length === 0 && <p>No declared model file is present in the local installation root. Nothing will be downloaded automatically.</p>}
    <ul style={{ listStyle: 'none', padding: 0 }}>
      {models.map(m => <li key={m.path} className="recording-item" style={{ display: 'block' }}>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
          <strong>{m.id}</strong>{m.tier && <span className="hw-badge">{m.tier}</span>}<LocalStateBadge state={m.state} />
          <span>{m.label}</span>
        </div>
        <div className="recording-item-meta">
          {m.present ? (m.hash_verified ? 'Present · SHA-256 verified against declaration' : 'Present · digest not verified') : 'Not present on disk'}
          {' · '}{m.outcome === 'RECOMMENDED' || m.outcome === 'ALLOWED_WITH_LIMITS' ? 'Load allowed' : m.outcome === 'REFUSED' ? 'Load refused' : 'Not loadable'}
          {m.weights_bytes !== null && ` · weights ${gib(m.weights_bytes)}`}
          {m.kv_estimate_bytes !== null && ` · KV ≈ ${gib(m.kv_estimate_bytes)} at ${m.context_tokens.toLocaleString()} ctx (estimated)`}
          {m.required_available_bytes !== null && m.available_ram_bytes !== null && ` · needs ${gib(m.required_available_bytes)} available, ${gib(m.available_ram_bytes)} available now`}
          {m.smoke && ` · smoke: ${m.smoke.generated_tokens} tokens in ${m.smoke.generation_ms} ms (count/time, not a speed claim)`}
        </div>
        {m.reasons.length > 0 && <div className="recording-item-meta">{m.reasons.join(' · ')}</div>}
      </li>)}
    </ul>
    <dl>
      {(Object.keys(LOCAL_STATE_TEXT) as LocalState[]).map(s => <div key={s}><dt><LocalStateBadge state={s} /></dt><dd>{LOCAL_STATE_TEXT[s].detail}</dd></div>)}
    </dl>
  </section>;
}

/** Native assessment only. No frontend RAM/CUDA heuristic, approval flag, URL or
 * file-presence shortcut. Downloads stop explicitly at missing trust; already
 * present declared files are reported in the local lane, never as qualified. */
export function ModelSetupWizard() {
  const [assessment, setAssessment] = useState<Assessment | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [step, setStep] = useState<'device' | 'policy'>('device');
  const [paused, setPaused] = useState(false);
  const epoch = useRef(0);
  const recheck = async () => {
    const run = ++epoch.current;
    setBusy(true); setError(''); setAssessment(null);
    try {
      const result = await fetchAssessment();
      if (epoch.current === run) setAssessment(result);
    } catch (e) { if (epoch.current === run) setError(`Device check unavailable: ${e instanceof Error ? e.message : String(e)}`); }
    finally { if (epoch.current === run) setBusy(false); }
  };
  useEffect(() => { void recheck(); return () => { epoch.current += 1; }; }, []);
  const p = assessment?.device;
  return <section aria-label="Local model setup" style={{ padding: 20, marginBottom: 24, border: '1px solid var(--border)', borderRadius: 'var(--radius-md)', background: 'var(--bg-secondary)' }}>
    <h3>Set up a local model</h3>
    <p>Check this device before any model download. Your hardware details stay here. There is no automatic cloud fallback.</p>
    {paused ? <>
      <p role="status">Setup paused. No download was started. Your vault and existing models are unchanged.</p>
      <button className="btn btn-secondary" onClick={() => { setPaused(false); void recheck(); }}>Resume setup</button>
    </> : <>
      <ol aria-label="Setup steps"><li>Check this device</li><li>Choose a qualified model</li><li>Review local download policy</li><li>Download and verify the complete model/runtime bundle</li><li>Run a real local self-test</li></ol>
      {error && <p role="alert">{error}</p>}
      {busy && <p role="status">Checking native hardware…</p>}
      {step === 'device' && p && <DeviceFacts device={p} />}
      {step === 'device' && assessment && <LocalModelsLane models={assessment.local_models ?? []} />}
      {assessment && <div role="status">
        <h4>Compatibility catalog: {assessment.catalog_state === 'UNCONFIGURED' ? 'not configured' : 'requires native review'}</h4>
        <p>{assessment.reason}</p>
        <p>No 2B, 4B or 12B option is approved for this device by a publisher qualification. Downloads remain blocked. Declared model files that are already present are decided natively in the local lane above and are labelled Qualified, Works here or Unknown — never qualified without a signed record.</p>
      </div>}
      {step === 'policy' && <>
        <h4>Local download policy — not enabled</h4>
        <p>Before enabling downloads, you will need an approved model and its exact licence notices, a storage cap, permitted networks, a metered-data choice and an expiry. Approval must be saved by the native app from this local session, not a website or an imported policy.</p>
        <p>This build cannot offer an approval until the signed catalog, native network checks and runtime self-test integration are available. Nothing has been consented to or downloaded.</p>
        <button className="btn btn-secondary" onClick={() => setStep('device')}>Back to device check</button>
      </>}
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 16 }}>
        <button className="btn btn-secondary" disabled={busy} onClick={() => { void recheck(); }}>Recheck my device</button>
        {step === 'device' && <button className="btn btn-secondary" onClick={() => setStep('policy')}>Review download requirements</button>}
        <button className="btn btn-primary" disabled title="Publisher signing catalog, local policy and runtime qualification are required">Download unavailable</button>
        <button className="btn btn-ghost" onClick={() => { epoch.current += 1; setBusy(false); setPaused(true); }}>Not now</button>
      </div>
    </>}
  </section>;
}
