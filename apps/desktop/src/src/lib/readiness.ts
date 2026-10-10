import type { DocumentMetadata, ModelCacheStatus, ModelConfig, ModelInfo, ModelStatus } from './tauri';

export interface ReadinessRow { label: string; value: string; detail: string }

/** Presentation only: these observations never select, load, grant or qualify anything. */
export function modelReadiness(
  selected: ModelInfo | undefined,
  status: ModelStatus | null,
  runtimeConfig: ModelConfig | null,
  cache: ModelCacheStatus | null,
): ReadinessRow[] {
  const loaded = status === 'LOADED' || status === 'GENERATING';
  const matches = !!selected && !!runtimeConfig?.model_path && (
    runtimeConfig.model_path === selected.path ||
    (!!cache?.staged && !!cache.cached_path && runtimeConfig.model_path === cache.cached_path)
  );
  return [
    { label: 'Selection', value: selected?.name ?? 'None selected', detail: 'Selection is the target of Load Model, not proof that it is running.' },
    { label: 'Selected asset', value: !selected ? 'Unknown' : selected.available ? 'Present on disk' : 'Missing on disk', detail: 'Discovery checks file presence. It does not establish digest integrity or workflow qualification.' },
    { label: 'Integrity evidence', value: cache?.staged ? 'Verified cache marker present' : cache ? 'No verified cache staged' : 'Unknown / cache probe unavailable', detail: 'Cache staging verifies SHA-256 against the manifest. This read-only probe checks the marker and file metadata, not a fresh hash of the selected asset.' },
    { label: 'Runtime', value: status === null ? 'Unknown / status unavailable' : loaded ? (matches ? 'Selected model loaded' : 'Another or unidentified model loaded') : status === 'LOADING' ? 'Loading — not yet loaded' : status === 'ERROR' ? 'Runtime error' : 'Not loaded', detail: `Last reported state: ${status ?? 'unknown'}. Loaded is not a self-test or a claim of task quality. Refresh to observe changes made elsewhere.` },
    { label: 'Image input', value: selected?.mmproj_path ? 'Projector path configured' : 'No projector path reported', detail: 'A path alone does not verify projector presence, model compatibility or a loaded vision workflow. Speech readiness is separate and is not tested here.' },
  ];
}

export const TRUSTED_HOST_DISCLOSURE = 'Agent tools use trusted host execution with partial enforcement, not an isolated coding sandbox. Full-access confirmation is static AllowedOnce, not an interactive approval for every action. Folder grants, host-command permission and browser guards still apply. Browser and commands may access the network. Ordinary chat changes have no general undo.';
export const CODING_ISOLATION_DISCLOSURE = 'Coding Tasks is a separate reviewed-worktree lane. Isolated execution requires the supported Linux isolation backend; it is blocked on Windows and macOS, with no trusted-host fallback under the same label. Revert is scoped to task-worktree changes, not command, browser or network effects.';

/** Existing metadata only; no inferred provenance authentication or new persistent schema. */
export function documentSourceSummary(doc: Pick<DocumentMetadata, 'id' | 'source_platform' | 'page_count' | 'word_count'>): string {
  const parts = [`Source ID: ${doc.id}`, `Recorded platform: ${doc.source_platform || 'unknown'}`];
  if (doc.page_count !== null && Number.isFinite(doc.page_count) && doc.page_count >= 0) parts.push(`Recorded pages: ${doc.page_count}`);
  if (doc.word_count !== null && Number.isFinite(doc.word_count) && doc.word_count >= 0) parts.push(`Recorded words: ${doc.word_count}`);
  return `${parts.join(' · ')}. Metadata is not proof of complete extraction or verified content.`;
}
