import { useEffect, useRef, useState } from 'react';
import { DeviceFacts, LocalModelsLane } from './ModelSetupWizard';
import { fetchAssessment, type Assessment } from './localModelLane';

/** Facts about THIS machine (RAM total/available, VRAM if detected, OS/ABI,
 * storage) and the three honest states of each declared local model file:
 * Qualified / Works here / Unknown. No tokens-per-second guesses, no
 * backend promoted beyond "detected". Downloads stay blocked separately. */
export function HardwareProfile() {
  const [assessment, setAssessment] = useState<Assessment | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const epoch = useRef(0);
  const recheck = async () => {
    const run = ++epoch.current;
    setBusy(true); setError(''); setAssessment(null);
    try {
      const result = await fetchAssessment();
      if (epoch.current === run) setAssessment(result);
    } catch (e) { if (epoch.current === run) setError(`Hardware facts unavailable: ${e instanceof Error ? e.message : String(e)}`); }
    finally { if (epoch.current === run) setBusy(false); }
  };
  useEffect(() => { void recheck(); return () => { epoch.current += 1; }; }, []);
  return <div>
    <div className="main-header"><h2>Hardware Profile</h2></div>
    <div className="main-body">
      <p>Measured on this machine just now. Values marked unknown were not measured and are never guessed.</p>
      {error && <p role="alert">{error}</p>}
      {busy && <p role="status">Measuring…</p>}
      {assessment && <DeviceFacts device={assessment.device} />}
      {assessment && <LocalModelsLane models={assessment.local_models ?? []} />}
      {assessment && <p role="status">Model downloads: {assessment.catalog_state === 'UNCONFIGURED' ? 'blocked — publisher catalog not configured' : 'require native review'}. Existing files and your vault are unchanged.</p>}
      <button className="btn btn-secondary" disabled={busy} onClick={() => { void recheck(); }}>Re-measure</button>
    </div>
  </div>;
}
