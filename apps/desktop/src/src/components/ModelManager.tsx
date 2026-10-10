import { useState, useEffect, useRef } from 'react';
import { ModelReadinessPanel } from './ModelReadinessPanel';
import { ModelSetupWizard, LocalStateBadge } from './ModelSetupWizard';
import { fetchAssessment, type LocalModelReport } from './localModelLane';
import { modelReadiness, TRUSTED_HOST_DISCLOSURE, CODING_ISOLATION_DISCLOSURE } from '../lib/readiness';
import { tauriApi, type ModelInfo, type ModelConfig, type ModelStatus, type AccelerationBackend, type SecurityLevel, type ModelCacheStatus, type ContextBudget } from '../lib/tauri';

export function ModelManager() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [localInstall, setLocalInstall] = useState(true);
  // Native three-state labels for declared local files (Qualified / Works here /
  // Unknown), keyed by path. Display only: the backend re-decides on Load.
  const [localReports, setLocalReports] = useState<LocalModelReport[]>([]);
  const [selectedModelPath, setSelectedModelPath] = useState<string>('');
  const [modelStatus, setModelStatus] = useState<ModelStatus>('NOT_LOADED');
  const [loadingModel, setLoadingModel] = useState(false);
  const modelOperation = useRef(0);
  useEffect(() => () => { modelOperation.current += 1; }, []);
  const [accelBackends, setAccelBackends] = useState<AccelerationBackend[]>([]);
  const [config, setConfig] = useState<ModelConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [runtimeConfig, setRuntimeConfig] = useState<ModelConfig | null>(null);
  const [statusObserved, setStatusObserved] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [healthNotice, setHealthNotice] = useState<string | null>(null);
  const selectionRef = useRef(selectedModelPath);
  selectionRef.current = selectedModelPath;
  const [error, setError] = useState<string | null>(null);
  const [securityLevel, setSecurityLevel] = useState<SecurityLevel>('STANDARD');
  const [vaultRoot, setVaultRoot] = useState<string>('');
  const [cacheStatus, setCacheStatus] = useState<ModelCacheStatus | null>(null);
  const [stagingCache, setStagingCache] = useState(false);
  // Universal-adaptive context derivation for the selected model + current
  // request. Null until a model is selected; unverified stays honest.
  const [contextBudget, setContextBudget] = useState<ContextBudget | null>(null);

  useEffect(() => {
    async function load() {
      try {
        // Detect vault root from USB pendrive, not hardcoded path
        const vaultInfo = await tauriApi.detectVault();
        const isLocal = vaultInfo.storage_kind === 'local';
        setLocalInstall(isLocal);
        const vaultRoot = vaultInfo.detected ? vaultInfo.vault_root : '';
        setVaultRoot(vaultRoot);
        if (isLocal) {
          try { setLocalReports((await fetchAssessment()).local_models ?? []); } catch { setLocalReports([]); }
        }

        const [modelList, backends, status, modelConfig, secLevel] = await Promise.all([
          tauriApi.listModels(vaultRoot),
          tauriApi.detectAcceleration(),
          tauriApi.getModelStatus(),
          tauriApi.getModelConfig(),
          tauriApi.getSecurityLevel(),
        ]);
        setModels(modelList);
        setAccelBackends(backends);
        setModelStatus(status);
        setStatusObserved(true);
        setConfig(modelConfig);
        setRuntimeConfig(modelConfig);
        setSecurityLevel(secLevel);

        // Prefer the first available model; if none, fall back to any model path
        // so the UI still shows which model would be loaded once the asset is present.
        if (modelList.length > 0) {
          const firstAvailable = modelList.find(m => m.available);
          const firstPath = firstAvailable?.path ?? modelList[0].path;
          setSelectedModelPath(firstPath);
        }
      } catch (e: any) {
        setError(e?.message || 'Failed to load model info');
      } finally {
        setLoading(false);
      }
    }
    load();
  }, []);

  // Follow automatic loading so the initial status cannot leave this panel
  // stuck on a spinner after the native server has finished loading.
  useEffect(() => {
    if (modelStatus !== 'LOADING' || loadingModel) return;
    let active = true;
    const interval = window.setInterval(() => {
      void Promise.all([tauriApi.getModelStatus(), tauriApi.getModelConfig()]).then(([status, runtime]) => {
        if (active) { setModelStatus(status); setRuntimeConfig(runtime); }
      }).catch(() => undefined);
    }, 1000);
    return () => { active = false; window.clearInterval(interval); };
  }, [modelStatus, loadingModel]);

  // Derive the budget the server launcher will actually apply, so the panel
  // states the granted context and every clamp reason before a session starts.
  useEffect(() => {
    if (!selectedModelPath || !config) { setContextBudget(null); return; }
    let active = true;
    tauriApi
      .getContextBudget(selectedModelPath, config.context_size, config.cache_type_k)
      .then(budget => { if (active) setContextBudget(budget); })
      .catch(() => { if (active) setContextBudget(null); });
    return () => { active = false; };
  }, [selectedModelPath, config?.context_size, config?.cache_type_k]);

  // Probe the host-disk model cache for the selected model. Cheap on purpose:
  // the backend only reads the manifest and stats two files, never hashes
  // the multi-GB model.
  useEffect(() => {
    let cancelled = false;
    setCacheStatus(null);
    if (!selectedModelPath || !vaultRoot) return;
    void tauriApi
      .modelCacheStatus(selectedModelPath, vaultRoot)
      .then(status => {
        if (!cancelled) setCacheStatus(status);
      })
      .catch(() => {
        // No manifest hash for this model (or no cache yet) — not an error
        // worth displacing real errors for; just show as unstaged.
        if (!cancelled) setCacheStatus(null);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedModelPath, vaultRoot]);

  const stageToHostCache = async () => {
    if (!selectedModelPath || !vaultRoot) return;
    const stagedPath = selectedModelPath;
    setStagingCache(true);
    setError(null);
    try {
      const status = await tauriApi.stageModelCache(selectedModelPath, vaultRoot);
      if (selectionRef.current === stagedPath) setCacheStatus(status);
    } catch (e: any) {
      setError(e?.message || 'Failed to stage the model to the host cache');
    } finally {
      setStagingCache(false);
    }
  };

  if (loading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: '48px' }}>
        <span className="spinner" />
      </div>
    );
  }

  const bestBackend = accelBackends[0] || 'CPU';
  const refreshObservations = async () => {
    const path = selectedModelPath;
    const operation = modelOperation.current;
    setRefreshing(true);
    try {
      const [status, runtime, modelList, cache] = await Promise.all([
        tauriApi.getModelStatus(), tauriApi.getModelConfig(), tauriApi.listModels(vaultRoot),
        path && vaultRoot ? tauriApi.modelCacheStatus(path, vaultRoot).catch(() => null) : Promise.resolve(null),
      ]);
      if (operation !== modelOperation.current) return;
      setModelStatus(status);
      setStatusObserved(true);
      setRuntimeConfig(runtime);
      setModels(modelList);
      if (selectionRef.current === path) setCacheStatus(cache);
      setError(null);
    } catch (e: unknown) {
      setError(`Could not refresh observations: ${e instanceof Error ? e.message : String(e)}`);
    } finally { setRefreshing(false); }
  };


  return (
    <div>
      <div className="main-header">
        <h2>Model Manager</h2>
        <div className="main-header-actions">
          <span style={{
            padding: '4px 12px',
            borderRadius: '4px',
            fontSize: '11px',
            fontWeight: 700,
            background: modelStatus === 'LOADED' ? 'var(--success-bg)' :
                        modelStatus === 'LOADING' ? 'var(--warning-bg)' :
                        modelStatus === 'ERROR' ? 'var(--danger-bg)' : 'var(--bg-tertiary)',
            color: modelStatus === 'LOADED' ? 'var(--success)' :
                   modelStatus === 'LOADING' ? 'var(--warning)' :
                   modelStatus === 'ERROR' ? 'var(--danger)' : 'var(--text-muted)',
          }}>
            {statusObserved ? modelStatus : 'UNKNOWN'}
          </span>
        </div>
      </div>

      <div className="main-body">
        {localInstall && <ModelSetupWizard />}
        <ModelReadinessPanel
          rows={modelReadiness(models.find(model => model.path === selectedModelPath), statusObserved ? modelStatus : null, runtimeConfig, cacheStatus)}
          refreshing={refreshing || loadingModel || stagingCache}
          onRefresh={() => { void refreshObservations(); }}
        />
        {/* Available Models */}
        <h3 id="model-assets" style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: 'var(--text-secondary)' }}>
          Available Models
        </h3>
        <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', marginBottom: '24px' }}>
          {models.map(model => (
            <div
              key={model.path}
              className="recording-item"
              style={{
                cursor: 'pointer',
                border: selectedModelPath === model.path ? '1px solid var(--accent)' : undefined,
                background: selectedModelPath === model.path ? 'var(--accent-bg)' : undefined,
              }}
              onClick={() => { setSelectedModelPath(model.path); setCacheStatus(null); }}
            >
              <div className="recording-item-icon" style={{
                background: model.available ? 'var(--accent-bg)' : 'var(--danger-bg)',
                color: model.available ? 'var(--accent)' : 'var(--danger)',
              }}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                  <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />
                  <polyline points="3.27 6.96 12 12.01 20.73 6.96" />
                  <line x1="12" y1="22.08" x2="12" y2="12" />
                </svg>
              </div>
              <div className="recording-item-info">
                <div className="recording-item-title">{model.name}</div>
                <div className="recording-item-meta">
                  {model.quantization === 'manifest-verified' ? 'Manifest-listed' : model.quantization} · {model.context_length.toLocaleString()} ctx
                  {model.context_verified ? '' : ' (unverified)'} · {model.file_size_gb.toFixed(1)} GB
                  {!model.available && ' · Not found on disk'}
                </div>
              </div>
              {/* Discovery reports presence, not a digest verification. */}
              <span className={`hw-badge ${model.available ? 'available' : 'unavailable'}`}>
                {model.available ? 'Present' : 'Missing'}
              </span>
              {localInstall && (() => {
                const report = localReports.find(r => r.path === model.path);
                return report ? <span title={`${report.label} — ${report.reasons.join('; ')}`} style={{ marginLeft: 8 }}><LocalStateBadge state={report.state} /></span> : null;
              })()}
            </div>
          ))}
          {models.length === 0 && (
            <div className="empty-state">
              <h3>No models found</h3>
              <p>{localInstall ? 'No declared model file is present in the local installation root. The local setup above explains which publisher approvals are missing for downloads. Your vault remains usable.' : 'This explicit legacy package view reports existing model files; it does not qualify them for the new local setup.'}</p>
            </div>
          )}
        </div>

        {/* Fast local cache — stream the model off the slow USB drive once,
            then launch from the host SSD/NVMe. The copy is digest-verified
            against the manifest in a single pass, so the cache never serves
            bytes the drive wouldn't vouch for. */}
        {selectedModelPath && cacheStatus !== null && (
          <div style={{
            marginBottom: '24px',
            padding: '12px 16px',
            background: cacheStatus.staged ? 'var(--success-bg)' : 'var(--bg-secondary)',
            border: `1px solid ${cacheStatus.staged ? 'var(--success-border, rgba(52,211,153,0.3))' : 'var(--border)'}`,
            borderRadius: 'var(--radius-md)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: '12px',
            flexWrap: 'wrap',
          }}>
            <div>
              <div style={{ fontSize: '13px', fontWeight: 600 }}>
                {cacheStatus.staged ? 'Staged host cache' : 'Host cache not staged'}
              </div>
              <div style={{ fontSize: '11px', color: 'var(--text-secondary)', marginTop: '2px' }}>
                {cacheStatus.staged
                  ? `Cached path: ${cacheStatus.cached_path} (${(cacheStatus.size_bytes ?? 0) / (1024 * 1024 * 1024)} GB). The cache marker is not a fresh digest check.`
                  : 'Optional: copy the existing model to the host disk. Staging verifies SHA-256 against the manifest; it does not download or load a model.'}
              </div>
            </div>
            {cacheStatus.staged ? (
              <button className="btn btn-secondary" disabled={stagingCache} onClick={stageToHostCache}>
                {stagingCache ? 'Staging…' : 'Re-stage'}
              </button>
            ) : (
              <button className="btn btn-primary" disabled={stagingCache} onClick={stageToHostCache}>
                {stagingCache ? 'Staging… (may copy multi-GB)' : 'Stage to host cache'}
              </button>
            )}
          </div>
        )}

        {/* Acceleration Backend */}
        <h3 style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: 'var(--text-secondary)' }}>
          Acceleration
        </h3>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(160px, 1fr))', gap: '8px', marginBottom: '24px' }}>
          {(['CUDA', 'METAL', 'VULKAN', 'CPU'] as AccelerationBackend[]).map(backend => {
            const isAvailable = accelBackends.includes(backend);
            const isBest = backend === bestBackend;
            return (
              <div key={backend} style={{
                padding: '12px',
                background: isBest ? 'var(--accent-bg)' : 'var(--bg-secondary)',
                border: `1px solid ${isBest ? 'var(--accent-border)' : 'var(--border)'}`,
                borderRadius: 'var(--radius-md)',
                textAlign: 'center',
              }}>
                <div style={{ fontSize: '13px', fontWeight: 600 }}>{backend}</div>
                <div style={{ fontSize: '11px', color: isAvailable ? 'var(--success)' : 'var(--text-muted)', marginTop: '4px' }}>
                  {isAvailable ? 'Detected — not load-tested' : 'Unknown / not detected'}
                </div>
              </div>
            );
          })}
        </div>

        {/* Model Configuration */}
        {config && (
          <>
            <h3 id="model-configuration" style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: 'var(--text-secondary)' }}>
              Configuration
            </h3>
            <div className="settings-section" style={{ marginBottom: '24px' }}>
              <div className="settings-section-body">
                <div className="settings-row">
                  <div>
                    <div className="settings-row-label">Context Size</div>
                    <div className="settings-row-desc">
                      Maximum context window for generation. 16K/32K need a quantized KV cache
                      (below) to fit small VRAM — pick q8_0 there.
                    </div>
                  </div>
                  <select
                    value={config.context_size}
                    onChange={e => {
                      const contextSize = Number(e.target.value);
                      // Host-adaptive default: large contexts switch the KV
                      // cache to q8_0 unless the user already chose something.
                      const nextConfig = { ...config, context_size: contextSize };
                      if (contextSize >= 16384 && !config.cache_type_v && !config.cache_type_k) {
                        nextConfig.cache_type_k = 'q8_0';
                        nextConfig.cache_type_v = 'q8_0';
                      }
                      setConfig(nextConfig);
                    }}
                  >
                    <option value={2048}>2048</option>
                    <option value={4096}>4096</option>
                    <option value={8192}>8192</option>
                    <option value={16384}>16384</option>
                    <option value={32768}>32768</option>
                    {/* The artifact's own trained context, offered whenever it
                        exceeds the fixed ladder — the model defines the
                        ceiling, not this list. */}
                    {!!contextBudget?.native_context && contextBudget.native_context > 32768 && (
                      <option value={contextBudget.native_context}>
                        {contextBudget.native_context} (model native)
                      </option>
                    )}
                  </select>
                </div>
                {contextBudget && (
                  <div
                    className="settings-row"
                    style={{
                      background: 'var(--bg-tertiary)',
                      borderRadius: 'var(--radius-sm)',
                      margin: '0 12px 12px',
                      padding: '10px 12px',
                    }}
                  >
                    <div>
                      <div className="settings-row-label" style={{ fontSize: '12px' }}>
                        Adaptive context budget
                      </div>
                      <div className="settings-row-desc">
                        Granted {contextBudget.granted_context.toLocaleString()} tokens · native{' '}
                        {contextBudget.native_context
                          ? `${contextBudget.native_context.toLocaleString()} (read from the artifact)`
                          : 'unverified (artifact metadata unreadable)'}
                        {' '}· KV cache ≈{' '}
                        {contextBudget.kv_estimate_bytes
                          ? `${(contextBudget.kv_estimate_bytes / 2 ** 30).toFixed(1)} GiB`
                          : 'unverified'}
                        {contextBudget.reasons.length > 0 &&
                          ` · ${contextBudget.reasons[contextBudget.reasons.length - 1]}`}
                      </div>
                    </div>
                  </div>
                )}
                <div className="settings-row">
                  <div>
                    <div className="settings-row-label">KV Cache (K)</div>
                    <div className="settings-row-desc">
                      Quantize the K cache to fit long contexts in small VRAM (q8_0 ≈ half of f16)
                    </div>
                  </div>
                  <select
                    value={config.cache_type_k ?? ''}
                    onChange={e => setConfig({ ...config, cache_type_k: e.target.value || undefined })}
                  >
                    <option value="">Server default (f16)</option>
                    <option value="q8_0">q8_0 (recommended ≥16K ctx)</option>
                    <option value="q4_0">q4_0 (smallest)</option>
                  </select>
                </div>
                <div className="settings-row">
                  <div>
                    <div className="settings-row-label">KV Cache (V)</div>
                    <div className="settings-row-desc">
                      Quantize the V cache — requires flash attention (auto by default)
                    </div>
                  </div>
                  <select
                    value={config.cache_type_v ?? ''}
                    onChange={e => {
                      const cacheTypeV = e.target.value || undefined;
                      const nextConfig = { ...config, cache_type_v: cacheTypeV };
                      // A quantized V cache needs flash attention — llama-server
                      // refuses the combination otherwise, so never let the UI
                      // build it. "Auto" stays untouched; an explicit "off"
                      // becomes "on" the moment a quantized V cache is picked.
                      if (cacheTypeV && config.flash_attention === false) {
                        nextConfig.flash_attention = true;
                      }
                      setConfig(nextConfig);
                    }}
                  >
                    <option value="">Server default (f16)</option>
                    <option value="q8_0">q8_0 (recommended ≥16K ctx)</option>
                    <option value="q4_0">q4_0 (smallest)</option>
                  </select>
                </div>
                <div className="settings-row">
                  <div>
                    <div className="settings-row-label">Flash Attention</div>
                    <div className="settings-row-desc">Required for quantized V cache; auto picks what the backend supports</div>
                  </div>
                  <select
                    value={config.flash_attention === undefined ? '' : config.flash_attention ? 'on' : 'off'}
                    onChange={e =>
                      setConfig({
                        ...config,
                        flash_attention: e.target.value === '' ? undefined : e.target.value === 'on',
                      })
                    }
                  >
                    <option value="">Auto (server default)</option>
                    <option value="on">On</option>
                    <option value="off">Off</option>
                  </select>
                </div>
                <div className="settings-row">
                  <div>
                    <div className="settings-row-label">GPU Layers</div>
                    <div className="settings-row-desc">Number of layers to offload to GPU (-1 = all)</div>
                  </div>
                  <select value={config.gpu_layers} onChange={e => setConfig({ ...config, gpu_layers: Number(e.target.value) })}>
                    <option value={-1}>All (-1)</option>
                    <option value={0}>CPU Only (0)</option>
                    <option value={10}>10 layers</option>
                    <option value={20}>20 layers</option>
                    <option value={30}>30 layers</option>
                  </select>
                </div>
                <div className="settings-row">
                  <div>
                    <div className="settings-row-label">Temperature</div>
                    <div className="settings-row-desc">Controls randomness (0 = deterministic)</div>
                  </div>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                    <input
                      type="range" min="0" max="2" step="0.1" value={config.temperature}
                      onChange={e => setConfig({ ...config, temperature: Number(e.target.value) })}
                      style={{ width: '120px' }}
                    />
                    <span style={{ fontFamily: 'var(--font-mono)', fontSize: '13px' }}>{config.temperature.toFixed(1)}</span>
                  </div>
                </div>
                <div className="settings-row">
                  <div>
                    <div className="settings-row-label">Max Tokens</div>
                    <div className="settings-row-desc">Maximum tokens per response</div>
                  </div>
                  <select value={config.max_tokens} onChange={e => setConfig({ ...config, max_tokens: Number(e.target.value) })}>
                    <option value={1024}>1024</option>
                    <option value={2048}>2048</option>
                    <option value={4096}>4096</option>
                    <option value={8192}>8192</option>
                  </select>
                </div>
              </div>
            </div>
          </>
        )}

        {/* Actions */}
        <div id="model-actions" style={{ display: 'flex', gap: '12px', marginBottom: '24px' }}>
          <button
            className="btn btn-primary"
            disabled={!selectedModelPath || !config || modelStatus === 'LOADING' || loadingModel}
            onClick={async () => {
              if (!config || !selectedModelPath) return;
              const operation = ++modelOperation.current;
              setLoadingModel(true);
              setModelStatus('LOADING');
              setError(null);
              try {
                const vaultInfo = await tauriApi.detectVault();
                const vaultRoot = vaultInfo.detected ? vaultInfo.vault_root : '';
                if (!vaultRoot) {
                  throw new Error('No UnoOne storage root detected. Unlock the local vault or insert the Pocket USB to load the model.');
                }

                window.dispatchEvent(new Event('unoone:model-manual-control'));
                // Local mode keeps the previous server running until the new
                // one passes its native inference smoke (backend rule); the
                // legacy drive lane stops first, unchanged.
                if (!localInstall) await tauriApi.stopModelServer();
                if (operation !== modelOperation.current) return;
                // The displayed cache status may belong to a previous selection.
                const selectedCache = await tauriApi.modelCacheStatus(selectedModelPath, vaultRoot).catch(() => null);

                const nextConfig: ModelConfig = {
                  ...config,
                  // Launch from the digest-verified host cache when the model
                  // is staged there — same bytes (manifest sha256 key), read
                  // from the host disk instead of the slow USB drive.
                  model_path: selectedCache?.staged && selectedCache.cached_path
                    ? selectedCache.cached_path
                    : selectedModelPath,
                  mmproj_path: models.find(model => model.path === selectedModelPath)?.mmproj_path,
                };

                // If the auto-discovered mmproj does not exist, clear it rather than
                // guess. Vision commands will error clearly if mmproj is required.
                if (!(await tauriApi.checkFileExists(nextConfig.mmproj_path ?? ''))) {
                  nextConfig.mmproj_path = undefined;
                }

                if (operation !== modelOperation.current) return;
                const port = await tauriApi.startModelServer(nextConfig, vaultRoot);
                if (operation !== modelOperation.current) return;
                const health = await tauriApi.checkModelHealth();
                if (operation !== modelOperation.current) return;
                if (!health.model_id) throw new Error('The loaded model has no verified identity.');
                setModelStatus('LOADED');
                setConfig(nextConfig);
                setRuntimeConfig(nextConfig);
                if (localInstall) {
                  try { setLocalReports((await fetchAssessment()).local_models ?? []); } catch { /* badges keep their last native value */ }
                }
                console.log('[ModelManager] llama-server started on port', port);
              } catch (e: unknown) {
                if (operation === modelOperation.current) {
                  setError(e instanceof Error ? e.message : String(e));
                  setModelStatus('ERROR');
                }
              } finally {
                if (operation === modelOperation.current) setLoadingModel(false);
              }
            }}
          >
            {modelStatus === 'LOADING' || loadingModel ? 'Loading…' : 'Load Model'}
          </button>

          <button
            className="btn btn-secondary"
            disabled={modelStatus !== 'LOADED' && modelStatus !== 'LOADING' && !loadingModel}
            onClick={async () => {
              setModelStatus('NOT_LOADED');
              const operation = ++modelOperation.current;
              setError(null);
              try {
                window.dispatchEvent(new Event('unoone:model-manual-control'));
                await tauriApi.stopModelServer();
              } catch (e: unknown) {
                if (operation === modelOperation.current) {
                  setError(`Unload failed: ${e instanceof Error ? e.message : String(e)}`);
                  setModelStatus('ERROR');
                }
              } finally {
                if (operation === modelOperation.current) setLoadingModel(false);
              }
            }}
          >
            {loadingModel || modelStatus === 'LOADING' ? 'Cancel loading' : 'Unload Model'}
          </button>

          <button
            className="btn btn-secondary"
            onClick={async () => {
              setError(null);
              try {
                const health = await tauriApi.checkModelHealth();
                setHealthNotice(`Health response (not workflow qualification): ${JSON.stringify(health)}`);
              } catch (e: any) {
                setHealthNotice(`Health check: ${e?.message || 'No inference backend responding'}`);
              }
            }}
          >
            Check Health
          </button>
        </div>

        {healthNotice && <p role="status" style={{ fontSize: '12px', overflowWrap: 'anywhere' }}>{healthNotice}</p>}
        {error && (
          <div
            role="alert"
            style={{
              marginBottom: '24px',
              padding: '12px',
              background: 'var(--error-bg)',
              color: 'var(--error-text)',
              borderRadius: 'var(--radius-sm)',
              fontSize: '13px',
              wordBreak: 'break-word',
            }}
          >
            {error}
            <button className="btn btn-secondary btn-sm" onClick={() => window.location.reload()} style={{ marginLeft: '12px' }}>Retry</button>
          </div>
        )}

        <div style={{ padding: '16px', background: 'var(--bg-secondary)', border: '1px solid var(--border)', borderRadius: 'var(--radius-md)' }}>
          <h4 style={{ fontSize: '13px', fontWeight: 600, marginBottom: '8px' }}>Execution boundaries</h4>
          <p style={{ fontSize: '12px', color: 'var(--text-secondary)', lineHeight: 1.6 }}>{TRUSTED_HOST_DISCLOSURE}</p>
          <p style={{ fontSize: '12px', color: 'var(--text-secondary)', lineHeight: 1.6 }}>{CODING_ISOLATION_DISCLOSURE}</p>
          <p style={{ fontSize: '12px' }}>Configured security level: <strong>{securityLevel}</strong>. This setting is not a safety certification.</p>
        </div>
      </div>
    </div>
  );
}


