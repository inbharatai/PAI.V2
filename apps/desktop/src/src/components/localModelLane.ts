// Native assessment contract shared by ModelSetupWizard, HardwareProfile and
// ModelManager. Types and helpers only (no components) so fast refresh stays
// intact. The renderer never supplies RAM, approval, URLs or file paths.
import { invoke } from '@tauri-apps/api/core';

export type Observation<T> = { provenance: 'UNKNOWN' } | { provenance: 'DETECTED' | 'ESTIMATED' | 'TESTED'; value: T };
export type LocalState = 'QUALIFIED' | 'WORKS_HERE' | 'UNKNOWN';
export type LocalOutcome = 'RECOMMENDED' | 'ALLOWED_WITH_LIMITS' | 'UNAVAILABLE' | 'REFUSED';
/** Native explanation of one declared model file already present in the root. */
export interface LocalModelReport {
  id: string; path: string; mmproj_path: string | null; tier: string | null;
  present: boolean; hash_verified: boolean;
  weights_bytes: number | null; projector_bytes: number | null; kv_estimate_bytes: number | null;
  context_tokens: number; total_ram_bytes: number | null; available_ram_bytes: number | null;
  required_available_bytes: number | null;
  state: LocalState; outcome: LocalOutcome; label: string; reasons: string[];
  decision: unknown | null;
  smoke: { generated_tokens: number; generation_ms: number; captured_at_ms: number } | null;
}
export interface DeviceFactsData {
  os: Observation<string>; os_version: Observation<string>; abi: Observation<string>;
  total_ram_bytes: Observation<number>; available_ram_bytes: Observation<number>;
  gpu_name: Observation<string>; total_vram_bytes: Observation<number>; available_vram_bytes: Observation<number>;
  usable_storage_bytes: Observation<number>;
  backends: { backend: string; health: Observation<string> }[];
}
export interface Assessment {
  schema_version: number;
  catalog_state: string;
  eligible_now: boolean;
  assets_ready: boolean;
  reason: string;
  decisions: unknown[];
  device: DeviceFactsData;
  local_models?: LocalModelReport[];
}
export function observation<T>(o: Observation<T> | undefined, format: (v: T) => string = String): string {
  if (!o || o.provenance === 'UNKNOWN') return 'Unknown — not measured';
  return `${format(o.value)} · ${o.provenance.toLowerCase()}`;
}
export const gib = (n: number) => `${(n / 2 ** 30).toFixed(1)} GiB`;

/** One native IPC; the renderer never supplies RAM, approval, URLs or file paths. */
export async function fetchAssessment(): Promise<Assessment> {
  const result = await invoke<Assessment>('get_model_setup_assessment');
  if (result.schema_version !== 1 || !result.device || !Array.isArray(result.decisions)) throw new Error('Unsupported native assessment');
  if (result.local_models !== undefined && !Array.isArray(result.local_models)) throw new Error('Unsupported native assessment');
  return result;
}

export const LOCAL_STATE_TEXT: Record<LocalState, { title: string; detail: string }> = {
  QUALIFIED: { title: 'Qualified', detail: 'A signed qualification record covers this exact file and device class.' },
  WORKS_HERE: { title: 'Works here', detail: 'Loaded and answered a real inference smoke on this machine. Not publisher-qualified.' },
  UNKNOWN: { title: 'Unknown', detail: 'No evidence beyond file presence and RAM estimates. A fitting file may still be allowed to load; it becomes "Works here" only after a real inference smoke.' },
};
