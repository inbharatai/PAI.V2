import { useState, useEffect } from 'react';
import { tauriApi, type ModelInfo, type ModelConfig, type ModelStatus, type AccelerationBackend, type SecurityLevel, type ModelCacheStatus } from '../lib/tauri';

export function ModelManager() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [selectedModelPath, setSelectedModelPath] = useState<string>('');
  const [modelStatus, setModelStatus] = useState<ModelStatus>('NOT_LOADED');
  const [accelBackends, setAccelBackends] = useState<AccelerationBackend[]>([]);
  const [config, setConfig] = useState<ModelConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [securityLevel, setSecurityLevel] = useState<SecurityLevel>('STANDARD');
  const [vaultRoot, setVaultRoot] = useState<string>('');
  const [cacheStatus, setCacheStatus] = useState<ModelCacheStatus | null>(null);
  const [stagingCache, setStagingCache] = useState(false);

  useEffect(() => {
    async function load() {
      try {
        // Detect vault root from USB pendrive, not hardcoded path
        const vaultInfo = await tauriApi.detectVault();
        const vaultRoot = vaultInfo.detected ? vaultInfo.vault_root : '';
        setVaultRoot(vaultRoot);

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
        setConfig(modelConfig);
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

  // Probe the host-disk model cache for the selected model. Cheap on purpose:
  // the backend only reads the manifest and stats two files, never hashes
  // the multi-GB model.
  useEffect(() => {
    let cancelled = false;
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
    setStagingCache(true);
    setError(null);
    try {
      const status = await tauriApi.stageModelCache(selectedModelPath, vaultRoot);
      setCacheStatus(status);
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

  if (error) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', padding: '48px', gap: '12px' }}>
        <h3 style={{ color: 'var(--danger)' }}>Error</h3>
        <p style={{ color: 'var(--text-secondary)', textAlign: 'center' }}>{error}</p>
        <button onClick={() => { setError(null); setLoading(true); window.location.reload(); }}>Retry</button>
      </div>
    );
  }

  const bestBackend = accelBackends[0] || 'CPU';

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
            {modelStatus}
          </span>
        </div>
      </div>

      <div className="main-body">
        {/* Available Models */}
        <h3 style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: 'var(--text-secondary)' }}>
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
              onClick={() => setSelectedModelPath(model.path)}
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
                  {model.quantization} · {model.context_length} ctx · {model.file_size_gb.toFixed(1)} GB
                  {!model.available && ' · Not downloaded'}
                </div>
              </div>
              <span className={`hw-badge ${model.available ? 'available' : 'unavailable'}`}>
                {model.available ? 'Ready' : 'Missing'}
              </span>
            </div>
          ))}
          {models.length === 0 && (
            <div className="empty-state">
              <h3>No models found</h3>
              <p>Download Gemma 4 12B Q4_K_M GGUF to your Pocket USB's MODELS directory.</p>
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
                {cacheStatus.staged ? '⚡ Staged to fast local cache' : 'Slow drive launch'}
              </div>
              <div style={{ fontSize: '11px', color: 'var(--text-secondary)', marginTop: '2px' }}>
                {cacheStatus.staged
                  ? `Next launch loads from ${cacheStatus.cached_path} (${(cacheStatus.size_bytes ?? 0) / (1024 * 1024 * 1024)} GB) instead of the USB drive`
                  : 'Stage the model to the host disk once so future launches skip the slow USB read. The copy is sha256-verified against the manifest.'}
              </div>
            </div>
            {cacheStatus.staged ? (
              <button className="btn btn-secondary" disabled={stagingCache} onClick={stageToHostCache}>
                {stagingCache ? 'Re-checking…' : 'Re-stage'}
              </button>
            ) : (
              <button className="btn btn-primary" disabled={stagingCache} onClick={stageToHostCache}>
                {stagingCache ? 'Staging… (one multi-GB pass)' : 'Stage to fast local cache'}
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
                  {isAvailable ? (isBest ? '✓ Best' : 'Available') : 'Not Available'}
                </div>
              </div>
            );
          })}
        </div>

        {/* Model Configuration */}
        {config && (
          <>
            <h3 style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: 'var(--text-secondary)' }}>
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
                  </select>
                </div>
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
        <div style={{ display: 'flex', gap: '12px', marginBottom: '24px' }}>
          <button
            className="btn btn-primary"
            disabled={!selectedModelPath || !config || modelStatus === 'LOADING'}
            onClick={async () => {
              if (!config || !selectedModelPath) return;
              setModelStatus('LOADING');
              setError(null);
              try {
                const vaultInfo = await tauriApi.detectVault();
                const vaultRoot = vaultInfo.detected ? vaultInfo.vault_root : '';
                if (!vaultRoot) {
                  throw new Error('No UnoOne vault detected. Insert the Pocket USB to load the model.');
                }

                const nextConfig: ModelConfig = {
                  ...config,
                  // Launch from the digest-verified host cache when the model
                  // is staged there — same bytes (manifest sha256 key), read
                  // from the host disk instead of the slow USB drive.
                  model_path: cacheStatus?.staged && cacheStatus.cached_path
                    ? cacheStatus.cached_path
                    : selectedModelPath,
                  mmproj_path: models.find(model => model.path === selectedModelPath)?.mmproj_path,
                };

                // If the auto-discovered mmproj does not exist, clear it rather than
                // guess. Vision commands will error clearly if mmproj is required.
                if (!(await tauriApi.checkFileExists(nextConfig.mmproj_path ?? ''))) {
                  nextConfig.mmproj_path = undefined;
                }

                const port = await tauriApi.startModelServer(nextConfig, vaultRoot);
                setModelStatus('LOADED');
                setConfig(nextConfig);
                console.log('[ModelManager] llama-server started on port', port);
              } catch (e: any) {
                setError(e?.message || 'Failed to start model server');
                setModelStatus('ERROR');
              }
            }}
          >
            {modelStatus === 'LOADING' ? 'Loading…' : 'Load Model'}
          </button>

          <button
            className="btn btn-secondary"
            disabled={modelStatus !== 'LOADED'}
            onClick={async () => {
              setModelStatus('NOT_LOADED');
              setError(null);
              try {
                await tauriApi.stopModelServer();
              } catch (e: any) {
                setError(`Unload failed: ${e?.message || 'Unknown error'}`);
                setModelStatus('ERROR');
              }
            }}
          >
            Unload Model
          </button>

          <button
            className="btn btn-secondary"
            onClick={async () => {
              setError(null);
              try {
                const health = await tauriApi.checkModelHealth();
                setError(`Health: ${JSON.stringify(health)}`);
              } catch (e: any) {
                setError(`Health check: ${e?.message || 'No inference backend responding'}`);
              }
            }}
          >
            Check Health
          </button>
        </div>

        {error && (
          <div
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
          </div>
        )}

        {/* Safety Pipeline Info */}
        <div style={{ padding: '16px', background: 'var(--success-bg)', border: '1px solid rgba(52, 211, 153, 0.3)', borderRadius: 'var(--radius-md)' }}>
          <h4 style={{ fontSize: '13px', fontWeight: 600, color: 'var(--success)', marginBottom: '8px' }}>
            🛡️ Safety Pipeline
          </h4>
          <p style={{ fontSize: '12px', color: 'var(--text-secondary)', lineHeight: 1.6 }}>
            All model output goes through the canonical safety pipeline:<br />
            <strong>Model → Parser → ToolAction → SafetyGuard → Execution</strong><br />
            Raw model output never executes tools directly. Security level: <strong>{securityLevel}</strong>
          </p>
        </div>
      </div>
    </div>
  );
}

