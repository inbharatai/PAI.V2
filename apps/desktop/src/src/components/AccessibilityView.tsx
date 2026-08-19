import { useState, useEffect, useCallback, useRef } from 'react';
import { tauriApi, type AccessibilityStatus, type AccessibilitySettingsInput } from '../lib/tauri';

export function AccessibilityView() {
  const [status, setStatus] = useState<AccessibilityStatus | null>(null);
  const [_isLoading, setIsLoading] = useState(true);
  const [vaultRoot, setVaultRoot] = useState('');
  const [highContrast, setHighContrast] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const [fontScale, setFontScale] = useState(1.0);
  const [sttLanguage, setSttLanguage] = useState('en');
  const [ttsLanguage, setTtsLanguage] = useState('en');

  // Phase 4: Vision/OCR/camera feature toggles — now wired to local UX
  const [cameraBlindAid, setCameraBlindAid] = useState(false);
  const [screenReaderDescription, setScreenReaderDescription] = useState(false);
  const [ocrExtraction, setOcrExtraction] = useState(false);

  // Camera preview state (WebView getUserMedia)
  const [cameraActive, setCameraActive] = useState(false);
  const [cameraError, setCameraError] = useState('');
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const streamRef = useRef<MediaStream | null>(null);

  // Vision lab state
  const [imagePath, setImagePath] = useState('');
  const [visionResult, setVisionResult] = useState('');
  const [visionError, setVisionError] = useState('');
  const [isProcessingVision, setIsProcessingVision] = useState(false);
  const [snapshots, setSnapshots] = useState<string[]>([]);

  // Phase 5: Voice lab state
  const [voiceStatus, setVoiceStatus] = useState('');
  const [voiceError, setVoiceError] = useState('');
  const [isCheckingVoice, setIsCheckingVoice] = useState(false);
  const [ttsText, setTtsText] = useState('');
  const [ttsResult, setTtsResult] = useState('');
  const [ttsAudioPath, setTtsAudioPath] = useState('');
  const [ttsError, setTtsError] = useState('');
  const [isSynthesizing, setIsSynthesizing] = useState(false);
  const [sttPath, setSttPath] = useState('');
  const [sttResult, setSttResult] = useState('');
  const [sttError, setSttError] = useState('');
  const [isTranscribing, setIsTranscribing] = useState(false);

  async function saveAccessibilitySettings(next: AccessibilitySettingsInput) {
    if (!vaultRoot) return;
    try {
      await tauriApi.setAccessibilityStatus(next, vaultRoot);
    } catch (err) {
      console.error('Failed to save accessibility settings:', err);
    }
  }

  const loadStatus = useCallback(async () => {
    setIsLoading(true);
    try {
      let detectedRoot = '';
      try {
        const vaultInfo = await tauriApi.detectVault();
        if (vaultInfo.detected) {
          detectedRoot = vaultInfo.vault_root;
          setVaultRoot(detectedRoot);
        }
      } catch {
        // Vault detection may fail in dev/test; continue with OS defaults
      }

      const accessibilityStatus = await tauriApi.getAccessibilityStatus();
      setStatus(accessibilityStatus);

      if (detectedRoot) {
        try {
          const persisted = await tauriApi.getAccessibilitySettings(detectedRoot);
          setHighContrast(persisted.high_contrast);
          setReducedMotion(persisted.reduced_motion);
          setFontScale(persisted.font_scale);
          setSttLanguage(persisted.stt_language);
          setTtsLanguage(persisted.tts_language);
        } catch {
          // Fall back to OS-detected values
          setHighContrast(accessibilityStatus.high_contrast);
          setReducedMotion(accessibilityStatus.reduced_motion);
        }
      } else {
        setHighContrast(accessibilityStatus.high_contrast);
        setReducedMotion(accessibilityStatus.reduced_motion);
      }
    } catch {
      // Tauri not available — use defaults
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    loadStatus();
  }, [loadStatus]);

  // Apply font scale to document
  useEffect(() => {
    document.documentElement.style.fontSize = `${fontScale * 100}%`;
    return () => { document.documentElement.style.fontSize = ''; };
  }, [fontScale]);

  // Apply high contrast
  useEffect(() => {
    if (highContrast) {
      document.documentElement.classList.add('high-contrast');
    } else {
      document.documentElement.classList.remove('high-contrast');
    }
  }, [highContrast]);

  // Apply reduced motion
  useEffect(() => {
    if (reducedMotion) {
      document.documentElement.classList.add('reduced-motion');
    } else {
      document.documentElement.classList.remove('reduced-motion');
    }
  }, [reducedMotion]);

  function stopCamera() {
    if (streamRef.current) {
      streamRef.current.getTracks().forEach(track => track.stop());
      streamRef.current = null;
    }
    if (videoRef.current) {
      videoRef.current.srcObject = null;
    }
    setCameraActive(false);
  }

  async function startCamera() {
    setCameraError('');
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ video: true });
      streamRef.current = stream;
      if (videoRef.current) {
        videoRef.current.srcObject = stream;
      }
      setCameraActive(true);
    } catch (err) {
      setCameraError(`Camera access failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  function captureSnapshot() {
    const video = videoRef.current;
    if (!video || video.videoWidth === 0) return;

    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    ctx.drawImage(video, 0, 0);
    const dataUrl = canvas.toDataURL('image/jpeg', 0.9);
    setSnapshots(prev => [dataUrl, ...prev].slice(0, 8));
  }

  useEffect(() => {
    if (!cameraBlindAid) {
      stopCamera();
    }
    return () => {
      if (streamRef.current) {
        streamRef.current.getTracks().forEach(track => track.stop());
      }
    };
  }, [cameraBlindAid]);

  async function runOcr() {
    if (!imagePath.trim()) return;
    setIsProcessingVision(true);
    setVisionError('');
    setVisionResult('');
    try {
      const result = await tauriApi.performOcr(imagePath.trim());
      setVisionResult(result.text);
    } catch (err) {
      setVisionError(`OCR failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsProcessingVision(false);
    }
  }

  async function runDescribe() {
    if (!imagePath.trim()) return;
    setIsProcessingVision(true);
    setVisionError('');
    setVisionResult('');
    try {
      const result = await tauriApi.describeImage(imagePath.trim());
      setVisionResult(result.description);
    } catch (err) {
      setVisionError(`Describe failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsProcessingVision(false);
    }
  }

  async function checkVoiceStatus() {
    if (!vaultRoot) return;
    setIsCheckingVoice(true);
    setVoiceError('');
    setVoiceStatus('');
    try {
      const status = await tauriApi.getVoiceStatus(vaultRoot, sttLanguage);
      setVoiceStatus(`STT: ${status.stt} • TTS: ${status.tts} • Language: ${status.language}`);
    } catch (err) {
      setVoiceError(`Voice status failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsCheckingVoice(false);
    }
  }

  async function runTts() {
    if (!vaultRoot || !ttsText.trim()) return;
    setIsSynthesizing(true);
    setTtsError('');
    setTtsResult('');
    setTtsAudioPath('');
    try {
      const result = await tauriApi.synthesizeSpeech(ttsText.trim(), vaultRoot, ttsLanguage);
      if (result.error) {
        setTtsError(result.error);
      } else {
        setTtsResult(`Status: ${result.status} • Path: ${result.audio_path ?? 'none'} • Duration: ${result.duration_seconds?.toFixed(2) ?? '?'}s • Sample rate: ${result.sample_rate} Hz`);
        if (result.audio_path) {
          setTtsAudioPath(result.audio_path);
        }
      }
    } catch (err) {
      setTtsError(`TTS failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsSynthesizing(false);
    }
  }

  async function runStt() {
    if (!vaultRoot || !sttPath.trim()) return;
    setIsTranscribing(true);
    setSttError('');
    setSttResult('');
    try {
      const result = await tauriApi.transcribeAudio(sttPath.trim(), vaultRoot, sttLanguage);
      setSttResult(`Status: ${result.status} • ${result.text}`);
    } catch (err) {
      setSttError(`STT failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsTranscribing(false);
    }
  }

  return (
    <div>
      <div className="main-header">
        <h2>Accessibility</h2>
        <div className="main-header-actions">
          <button className="btn btn-secondary btn-sm" onClick={loadStatus}>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <polyline points="1 4 1 10 7 10" />
              <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
            </svg>
            Refresh
          </button>
        </div>
      </div>

      <div className="main-body">
        <div className="settings-view">
          {/* Vision — backend functions exist; desktop UX toggles now wired */}
          <div className="settings-section">
            <div className="settings-section-header">👁️ Blind View (Vision Assist)</div>
            <div className="settings-section-body">
              <p style={{ fontSize: '13px', color: 'var(--text-secondary)', marginBottom: '16px' }}>
                Blind View uses Gemma 4 12B vision to describe images, detected objects, and screen content
                for visually impaired users. Camera feed is processed locally — nothing leaves the device.
              </p>

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Camera Blind Aid</div>
                  <div className="settings-row-desc">Live camera preview via WebView — backend ready; desktop UX now wired</div>
                </div>
                <label style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={cameraBlindAid}
                    onChange={e => setCameraBlindAid(e.target.checked)}
                    style={{ width: '16px', height: '16px' }}
                  />
                </label>
              </div>

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Screen Reader Description</div>
                  <div className="settings-row-desc">Describe on-screen content via mmproj when model is loaded — UX now wired</div>
                </div>
                <label style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={screenReaderDescription}
                    onChange={e => setScreenReaderDescription(e.target.checked)}
                    style={{ width: '16px', height: '16px' }}
                  />
                </label>
              </div>

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">OCR Text Extraction</div>
                  <div className="settings-row-desc">Extract text from images and documents via Gemma mmproj — UX now wired</div>
                </div>
                <label style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={ocrExtraction}
                    onChange={e => setOcrExtraction(e.target.checked)}
                    style={{ width: '16px', height: '16px' }}
                  />
                </label>
              </div>

              {(cameraBlindAid || screenReaderDescription || ocrExtraction) && (
                <div
                  style={{
                    marginTop: '20px',
                    padding: '16px',
                    border: '1px solid var(--border)',
                    borderRadius: 'var(--radius-md)',
                    background: 'var(--bg-secondary)',
                  }}
                >
                  <div style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px' }}>
                    Vision Lab
                  </div>

                  {cameraBlindAid && (
                    <div style={{ marginBottom: '20px' }}>
                      <div style={{ display: 'flex', gap: '8px', marginBottom: '12px' }}>
                        <button
                          className="btn btn-primary btn-sm"
                          onClick={startCamera}
                          disabled={cameraActive}
                        >
                          {cameraActive ? 'Camera On' : 'Start Camera'}
                        </button>
                        <button
                          className="btn btn-secondary btn-sm"
                          onClick={stopCamera}
                          disabled={!cameraActive}
                        >
                          Stop Camera
                        </button>
                        <button
                          className="btn btn-secondary btn-sm"
                          onClick={captureSnapshot}
                          disabled={!cameraActive}
                        >
                          Capture Snapshot
                        </button>
                      </div>

                      {cameraError && (
                        <div
                          style={{
                            marginBottom: '12px',
                            padding: '8px 12px',
                            background: 'var(--error-bg)',
                            color: 'var(--error-text)',
                            borderRadius: 'var(--radius-sm)',
                            fontSize: '13px',
                          }}
                        >
                          {cameraError}
                        </div>
                      )}

                      <div
                        style={{
                          position: 'relative',
                          width: '320px',
                          maxWidth: '100%',
                          aspectRatio: '4 / 3',
                          background: '#000',
                          borderRadius: 'var(--radius-sm)',
                          overflow: 'hidden',
                        }}
                      >
                        <video
                          ref={videoRef}
                          autoPlay
                          playsInline
                          muted
                          style={{ width: '100%', height: '100%', objectFit: 'cover' }}
                        />
                        {!cameraActive && (
                          <div
                            style={{
                              position: 'absolute',
                              inset: 0,
                              display: 'flex',
                              alignItems: 'center',
                              justifyContent: 'center',
                              color: 'var(--text-muted)',
                              fontSize: '13px',
                            }}
                          >
                            Camera preview off
                          </div>
                        )}
                      </div>

                      {snapshots.length > 0 && (
                        <div style={{ marginTop: '12px' }}>
                          <div style={{ fontSize: '12px', color: 'var(--text-secondary)', marginBottom: '8px' }}>
                            Captured snapshots (preview only)
                          </div>
                          <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
                            {snapshots.map((src, idx) => (
                              <img
                                key={idx}
                                src={src}
                                alt={`Snapshot ${idx + 1}`}
                                style={{ width: '80px', height: '60px', objectFit: 'cover', borderRadius: '4px' }}
                              />
                            ))}
                          </div>
                        </div>
                      )}
                    </div>
                  )}

                  {(screenReaderDescription || ocrExtraction) && (
                    <div>
                      <label style={{ display: 'block', fontSize: '13px', marginBottom: '6px' }}>
                        Image file path
                      </label>
                      <input
                        type="text"
                        value={imagePath}
                        onChange={e => setImagePath(e.target.value)}
                        placeholder="C:\\path\\to\\image.png"
                        style={{
                          width: '100%',
                          padding: '8px 12px',
                          fontSize: '13px',
                          borderRadius: 'var(--radius-sm)',
                          border: '1px solid var(--border)',
                          background: 'var(--bg-primary)',
                          color: 'var(--text-primary)',
                          marginBottom: '12px',
                        }}
                      />

                      <div style={{ display: 'flex', gap: '8px', marginBottom: '12px' }}>
                        {ocrExtraction && (
                          <button
                            className="btn btn-primary btn-sm"
                            onClick={runOcr}
                            disabled={isProcessingVision || !imagePath.trim()}
                          >
                            Run OCR
                          </button>
                        )}
                        {screenReaderDescription && (
                          <button
                            className="btn btn-secondary btn-sm"
                            onClick={runDescribe}
                            disabled={isProcessingVision || !imagePath.trim()}
                          >
                            Describe Image
                          </button>
                        )}
                      </div>

                      {visionError && (
                        <div
                          style={{
                            marginBottom: '12px',
                            padding: '8px 12px',
                            background: 'var(--error-bg)',
                            color: 'var(--error-text)',
                            borderRadius: 'var(--radius-sm)',
                            fontSize: '13px',
                          }}
                        >
                          {visionError}
                        </div>
                      )}

                      {isProcessingVision && (
                        <div style={{ fontSize: '13px', color: 'var(--text-secondary)', marginBottom: '12px' }}>
                          Running vision model…
                        </div>
                      )}

                      {visionResult && (
                        <div
                          style={{
                            padding: '12px',
                            background: 'var(--bg-primary)',
                            borderRadius: 'var(--radius-sm)',
                            border: '1px solid var(--border)',
                            fontSize: '13px',
                            lineHeight: 1.5,
                            whiteSpace: 'pre-wrap',
                            maxHeight: '240px',
                            overflow: 'auto',
                          }}
                        >
                          {visionResult}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          </div>

          {/* Display — these settings work now and are persisted when a vault is unlocked */}
          <div className="settings-section">
            <div className="settings-section-header">🖥️ Display</div>
            <div className="settings-section-body">
              {status?.screen_reader_detected && (
                <div style={{ marginBottom: '12px', padding: '8px 12px', background: 'rgba(34,197,94,0.1)', borderRadius: 'var(--radius-sm)', fontSize: '13px', color: 'var(--success)' }}>
                  ✅ Screen reader detected: {status.screen_reader_name}
                </div>
              )}

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">High Contrast</div>
                  <div className="settings-row-desc">Increase contrast for better visibility</div>
                </div>
                <label style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={highContrast}
                    onChange={e => {
                      const next = e.target.checked;
                      setHighContrast(next);
                      saveAccessibilitySettings({
                        high_contrast: next,
                        reduced_motion: reducedMotion,
                        font_scale: fontScale,
                        stt_language: sttLanguage,
                        tts_language: ttsLanguage,
                      });
                    }}
                    style={{ width: '16px', height: '16px' }}
                  />
                </label>
              </div>

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Reduced Motion</div>
                  <div className="settings-row-desc">Minimize animations and transitions</div>
                </div>
                <label style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={reducedMotion}
                    onChange={e => {
                      const next = e.target.checked;
                      setReducedMotion(next);
                      saveAccessibilitySettings({
                        high_contrast: highContrast,
                        reduced_motion: next,
                        font_scale: fontScale,
                        stt_language: sttLanguage,
                        tts_language: ttsLanguage,
                      });
                    }}
                    style={{ width: '16px', height: '16px' }}
                  />
                </label>
              </div>

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Font Scale</div>
                  <div className="settings-row-desc">Adjust text size throughout the application</div>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                  <input
                    type="range"
                    min="0.8"
                    max="2.0"
                    step="0.1"
                    value={fontScale}
                    onChange={e => {
                      const next = Number(e.target.value);
                      setFontScale(next);
                      saveAccessibilitySettings({
                        high_contrast: highContrast,
                        reduced_motion: reducedMotion,
                        font_scale: next,
                        stt_language: sttLanguage,
                        tts_language: ttsLanguage,
                      });
                    }}
                    style={{ width: '120px' }}
                  />
                  <span style={{ fontFamily: 'var(--font-mono)', fontSize: '13px' }}>
                    {fontScale.toFixed(1)}x
                  </span>
                </div>
              </div>
            </div>
          </div>

          {/* Voice */}
          <div className="settings-section">
            <div className="settings-section-header">🔊 Voice & Audio</div>
            <div className="settings-section-body">
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Screen Reader Support</div>
                  <div className="settings-row-desc">Announce UI changes to screen readers (NVDA, JAWS, VoiceOver)</div>
                </div>
                <label style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={status?.screen_reader_detected ?? false}
                    disabled
                    style={{ width: '16px', height: '16px' }}
                  />
                </label>
              </div>

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">TTS Language</div>
                  <div className="settings-row-desc">Language for text-to-speech output — Piper runtime now wired</div>
                </div>
                <select
                  value={ttsLanguage}
                  onChange={e => {
                    const next = e.target.value;
                    setTtsLanguage(next);
                    saveAccessibilitySettings({
                      high_contrast: highContrast,
                      reduced_motion: reducedMotion,
                      font_scale: fontScale,
                      stt_language: sttLanguage,
                      tts_language: next,
                    });
                  }}
                >
                  <option value="en">English</option>
                  <option value="hi">हिन्दी (Hindi)</option>
                  <option value="bn">বাংলা (Bengali)</option>
                  <option value="ta">தமிழ் (Tamil)</option>
                  <option value="te">తెలుగు (Telugu)</option>
                  <option value="kn">ಕನ್ನಡ (Kannada)</option>
                  <option value="ml">മലയാളം (Malayalam)</option>
                </select>
              </div>

              <div className="settings-row">
                <div>
                  <div className="settings-row-label">STT Language</div>
                  <div className="settings-row-desc">Language for speech-to-text recognition — Whisper runtime now wired</div>
                </div>
                <select
                  value={sttLanguage}
                  onChange={e => {
                    const next = e.target.value;
                    setSttLanguage(next);
                    saveAccessibilitySettings({
                      high_contrast: highContrast,
                      reduced_motion: reducedMotion,
                      font_scale: fontScale,
                      stt_language: next,
                      tts_language: ttsLanguage,
                    });
                  }}
                >
                  <option value="en">English</option>
                  <option value="hi">हिन्दी (Hindi)</option>
                  <option value="bn">বাংলা (Bengali)</option>
                  <option value="ta">தமிழ் (Tamil)</option>
                  <option value="te">తెలుగు (Telugu)</option>
                  <option value="kn">ಕನ್ನಡ (Kannada)</option>
                  <option value="ml">മലയാളം (Malayalam)</option>
                </select>
              </div>

              {/* Voice Lab */}
              <div
                style={{
                  marginTop: '20px',
                  padding: '16px',
                  border: '1px solid var(--border)',
                  borderRadius: 'var(--radius-md)',
                  background: 'var(--bg-secondary)',
                }}
              >
                <div style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px' }}>
                  Voice Lab
                </div>

                <div style={{ display: 'flex', gap: '8px', marginBottom: '12px' }}>
                  <button
                    className="btn btn-secondary btn-sm"
                    onClick={checkVoiceStatus}
                    disabled={isCheckingVoice || !vaultRoot}
                  >
                    {isCheckingVoice ? 'Checking…' : 'Check Voice Status'}
                  </button>
                </div>

                {voiceError && (
                  <div
                    style={{
                      marginBottom: '12px',
                      padding: '8px 12px',
                      background: 'var(--error-bg)',
                      color: 'var(--error-text)',
                      borderRadius: 'var(--radius-sm)',
                      fontSize: '13px',
                    }}
                  >
                    {voiceError}
                  </div>
                )}

                {voiceStatus && (
                  <div
                    style={{
                      marginBottom: '16px',
                      padding: '8px 12px',
                      background: 'var(--bg-primary)',
                      borderRadius: 'var(--radius-sm)',
                      border: '1px solid var(--border)',
                      fontSize: '13px',
                    }}
                  >
                    {voiceStatus}
                  </div>
                )}

                <div style={{ marginBottom: '16px' }}>
                  <label style={{ display: 'block', fontSize: '13px', marginBottom: '6px' }}>
                    Text-to-speech
                  </label>
                  <textarea
                    value={ttsText}
                    onChange={e => setTtsText(e.target.value)}
                    placeholder="Type text to synthesize with Piper…"
                    rows={3}
                    style={{
                      width: '100%',
                      padding: '8px 12px',
                      fontSize: '13px',
                      borderRadius: 'var(--radius-sm)',
                      border: '1px solid var(--border)',
                      background: 'var(--bg-primary)',
                      color: 'var(--text-primary)',
                      marginBottom: '8px',
                      resize: 'vertical',
                    }}
                  />
                  <button
                    className="btn btn-primary btn-sm"
                    onClick={runTts}
                    disabled={isSynthesizing || !vaultRoot || !ttsText.trim()}
                  >
                    {isSynthesizing ? 'Synthesizing…' : 'Synthesize Speech'}
                  </button>

                  {ttsError && (
                    <div
                      style={{
                        marginTop: '8px',
                        padding: '8px 12px',
                        background: 'var(--error-bg)',
                        color: 'var(--error-text)',
                        borderRadius: 'var(--radius-sm)',
                        fontSize: '13px',
                      }}
                    >
                      {ttsError}
                    </div>
                  )}

                  {ttsResult && (
                    <div
                      style={{
                        marginTop: '8px',
                        padding: '8px 12px',
                        background: 'var(--bg-primary)',
                        borderRadius: 'var(--radius-sm)',
                        border: '1px solid var(--border)',
                        fontSize: '13px',
                        wordBreak: 'break-word',
                      }}
                    >
                      {ttsResult}
                    </div>
                  )}

                  {ttsAudioPath && (
                    <audio
                      controls
                      src={tauriApi.convertFileSrc(ttsAudioPath)}
                      style={{
                        marginTop: '12px',
                        width: '100%',
                        borderRadius: 'var(--radius-sm)',
                      }}
                      aria-label="Synthesized speech playback"
                    >
                      Your browser does not support the audio element.
                    </audio>
                  )}
                </div>

                <div>
                  <label style={{ display: 'block', fontSize: '13px', marginBottom: '6px' }}>
                    Speech-to-text audio file path
                  </label>
                  <input
                    type="text"
                    value={sttPath}
                    onChange={e => setSttPath(e.target.value)}
                    placeholder="C:\\path\\to\\recording.wav"
                    style={{
                      width: '100%',
                      padding: '8px 12px',
                      fontSize: '13px',
                      borderRadius: 'var(--radius-sm)',
                      border: '1px solid var(--border)',
                      background: 'var(--bg-primary)',
                      color: 'var(--text-primary)',
                      marginBottom: '8px',
                    }}
                  />
                  <button
                    className="btn btn-secondary btn-sm"
                    onClick={runStt}
                    disabled={isTranscribing || !vaultRoot || !sttPath.trim()}
                  >
                    {isTranscribing ? 'Transcribing…' : 'Transcribe Audio'}
                  </button>

                  {sttError && (
                    <div
                      style={{
                        marginTop: '8px',
                        padding: '8px 12px',
                        background: 'var(--error-bg)',
                        color: 'var(--error-text)',
                        borderRadius: 'var(--radius-sm)',
                        fontSize: '13px',
                      }}
                    >
                      {sttError}
                    </div>
                  )}

                  {sttResult && (
                    <div
                      style={{
                        marginTop: '8px',
                        padding: '8px 12px',
                        background: 'var(--bg-primary)',
                        borderRadius: 'var(--radius-sm)',
                        border: '1px solid var(--border)',
                        fontSize: '13px',
                        wordBreak: 'break-word',
                      }}
                    >
                      {sttResult}
                    </div>
                  )}
                </div>
              </div>
            </div>
          </div>

          {/* Keyboard */}
          <div className="settings-section">
            <div className="settings-section-header">⌨️ Keyboard Navigation</div>
            <div className="settings-section-body">
              <div style={{ fontSize: '13px', color: 'var(--text-secondary)', lineHeight: 1.6 }}>
                <p><strong>Shortcuts:</strong></p>
                <ul style={{ paddingLeft: '20px', marginTop: '8px' }}>
                  <li><kbd style={{ padding: '2px 6px', background: 'var(--bg-tertiary)', borderRadius: '4px', fontFamily: 'var(--font-mono)', fontSize: '12px' }}>Ctrl+L</kbd> — Lock vault</li>
                  <li><kbd style={{ padding: '2px 6px', background: 'var(--bg-tertiary)', borderRadius: '4px', fontFamily: 'var(--font-mono)', fontSize: '12px' }}>Ctrl+N</kbd> — New chat</li>
                  <li><kbd style={{ padding: '2px 6px', background: 'var(--bg-tertiary)', borderRadius: '4px', fontFamily: 'var(--font-mono)', fontSize: '12px' }}>Ctrl+R</kbd> — Start/stop recording</li>
                  <li><kbd style={{ padding: '2px 6px', background: 'var(--bg-tertiary)', borderRadius: '4px', fontSize: '12px', fontFamily: 'var(--font-mono)' }}>Ctrl+M</kbd> — Toggle microphone</li>
                  <li><kbd style={{ padding: '2px 6px', background: 'var(--bg-tertiary)', borderRadius: '4px', fontFamily: 'var(--font-mono)', fontSize: '12px' }}>Ctrl+1-8</kbd> — Switch views</li>
                  <li><kbd style={{ padding: '2px 6px', background: 'var(--bg-tertiary)', borderRadius: '4px', fontFamily: 'var(--font-mono)', fontSize: '12px' }}>Escape</kbd> — Cancel current action</li>
                </ul>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
