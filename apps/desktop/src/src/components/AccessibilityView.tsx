import { useState, useEffect, useCallback, useRef } from 'react';
import { tauriApi, type AccessibilityStatus, type AccessibilitySettingsInput } from '../lib/tauri';

/** Live-caught 2026-09-13 (defect #23): a vision invoke whose IPC response was
 * dropped never settled, wedging the UI on "Running vision model…" forever
 * with no error and no way to recover without remounting the view. Every
 * long-running vision invoke is bounded by this wrapper so the user always
 * gets either a result or an honest timeout error, and the buttons re-enable.
 * The backend may still complete the work; this only bounds how long the UI
 * will wait silently. */
function withVisionTimeout<T>(promise: Promise<T>, ms: number, what: string): Promise<T> {
  return Promise.race([
    promise,
    new Promise<never>((_, reject) =>
      setTimeout(
        () => reject(new Error(`${what} did not finish within ${Math.round(ms / 1000)}s. Try again — if this keeps happening, restart the app.`)),
        ms
      )
    ),
  ]);
}

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
  // Shared speech player (voice lab + spoken blind-aid descriptions).
  const speechAudioRef = useRef<HTMLAudioElement | null>(null);
  // Phone-parity live narration state (2026-09-14) — declared up here because
  // the camera cleanup effect below stops narration when the blind aid is
  // toggled off.
  const [narrationOn, setNarrationOn] = useState(false);
  const [narrationStatus, setNarrationStatus] = useState('');

  // Vision lab state
  const [imagePath, setImagePath] = useState('');
  const [visionResult, setVisionResult] = useState('');
  const [visionError, setVisionError] = useState('');
  // Spoken-output status for the blind-aid describe flow (defect #24): shown
  // next to the vision result so a blind user is told when speech failed —
  // the text result alone is invisible to them.
  const [speechNotice, setSpeechNotice] = useState('');
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

  /** True while the camera stream has at least one live video track. */
  function hasLiveCamera(): boolean {
    const stream = streamRef.current;
    return !!stream && stream.getVideoTracks().some(track => track.readyState === 'live');
  }

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
    // Live-caught 2026-09-12 (defect #20): a stale/dead stream wedged the
    // camera — "Camera On" disabled the Start button while the track had
    // already ended, with no way to restart and no feedback. Release any
    // previous stream and always acquire a fresh one.
    if (streamRef.current) {
      streamRef.current.getTracks().forEach(track => track.stop());
      streamRef.current = null;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ video: true });
      streamRef.current = stream;
      // If the device is unplugged or claimed by another app, surface it
      // immediately instead of leaving a silent dead preview.
      stream.getVideoTracks().forEach(track => {
        track.addEventListener('ended', () => {
          setCameraActive(false);
          setCameraError(
            'Camera stream ended — the device may have been disconnected or claimed by another app. Press Start Camera to retry.'
          );
        });
      });
      if (videoRef.current) {
        videoRef.current.srcObject = stream;
        try {
          await videoRef.current.play();
        } catch {
          // Autoplay policy hiccup; muted video recovers on its own.
        }
      }
      setCameraActive(true);
    } catch (err) {
      setCameraError(`Camera access failed: ${err instanceof Error ? err.message : String(err)}`);
      setCameraActive(false);
    }
  }

  async function captureSnapshot() {
    const video = videoRef.current;
    // Live-caught 2026-09-12 (defect #20): this used to return silently when
    // the frame was not ready — a blind user pressed Capture and nothing
    // happened, with no explanation. Always give feedback.
    if (!video || video.videoWidth === 0 || !hasLiveCamera()) {
      setCameraError(
        'Camera is not ready yet. Press Start Camera and wait for the preview to show live video, then capture again.'
      );
      setCameraActive(false);
      return;
    }

    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext('2d');
    if (!ctx) {
      setCameraError('Cannot capture: the browser did not provide a 2D canvas.');
      return;
    }

    ctx.drawImage(video, 0, 0);
    const dataUrl = canvas.toDataURL('image/jpeg', 0.9);
    setSnapshots(prev => [dataUrl, ...prev].slice(0, 8));

    // Live-caught 2026-09-12 (defect #19): a blind aid's snapshot must reach
    // the vision pipeline, not die as a preview-only DOM thumbnail. Persist
    // the frame to disk so describe/OCR can read it; when the Screen Reader
    // Description assist is on, describe it immediately and speak it.
    setCameraError('');
    try {
      const savedPath = await tauriApi.saveVisionSnapshot(dataUrl);
      setImagePath(savedPath);
      if (screenReaderDescription) {
        // Blind-aid flow: short spoken-style summary, not the long detailed
        // description — this is what gets voiced.
        await runDescribeOn(savedPath, 'scene_summary');
      }
    } catch (err) {
      setCameraError(`Snapshot save failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  /**
   * Live-caught 2026-09-12 (defect #20): the Screen Reader Description assist
   * only ever described camera snapshots — a blind user could not ask "what is
   * on my screen right now?". Capture the app's main window and feed it
   * through the same describe-and-speak path.
   */
  async function describeScreen() {
    setVisionError('');
    try {
      const screenPath = await withVisionTimeout(
        tauriApi.captureScreenSnapshot(),
        30_000,
        'Screen capture'
      );
      setImagePath(screenPath);
      await runDescribeOn(screenPath);
    } catch (err) {
      setVisionError(`Screen capture failed: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  useEffect(() => {
    if (!cameraBlindAid) {
      setNarrationOn(false);
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
      const result = await withVisionTimeout(
        tauriApi.performOcr(imagePath.trim()),
        300_000,
        'OCR'
      );
      setVisionResult(result.text);
    } catch (err) {
      setVisionError(`OCR failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsProcessingVision(false);
    }
  }

  async function runDescribe() {
    if (!imagePath.trim()) return;
    // Manual describe keeps the long detailed description; the blind-aid
    // flows (capture auto-describe, live narration, what's-in-front) use the
    // short spoken-style scene_summary mode.
    await runDescribeOn(imagePath.trim());
  }

  /** Describe a saved image; in the blind-aid flow the result is SPOKEN, not
   * just printed — a blind user cannot read the text box. Pass
   * mode='scene_summary' for the short phone-parity narration voice. */
  async function runDescribeOn(path: string, mode?: string) {
    setIsProcessingVision(true);
    setVisionError('');
    setVisionResult('');
    try {
      // Live-caught 2026-09-13 (defect #23): an invoke whose IPC response was
      // dropped never settled — the button stayed wedged on "Running vision
      // model…" forever with no error. Every vision invoke is now bounded so
      // the user always gets either a result or an honest error.
      const result = await withVisionTimeout(
        tauriApi.describeImage(path, mode),
        300_000,
        'Image description'
      );
      setVisionResult(result.description);
      await speakText(result.description);
    } catch (err) {
      setVisionError(`Describe failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsProcessingVision(false);
    }
  }

  /** Hand a question and/or a captured frame to the chat panel (2026-09-14,
   * OCR/blind-aid alignment): every capability ends in the one conversation,
   * where the full agent — not just the vision describe — can act on it. */
  function askInChat(detail: { text?: string; imageDataUrl?: string }) {
    window.dispatchEvent(new CustomEvent('unoone:ask-in-chat', { detail }));
  }

  // ---- Phone-parity live blind aid (2026-09-14, task #55) ----
  // The phone's BlindAidManager continuously analyzes frames, throttles
  // spoken scene summaries, and resets all state per session. The desktop
  // now mirrors that: while narration is on, capture a frame roughly every
  // 25s, describe it in the short spoken-style "scene_summary" mode, and
  // speak it only when the scene meaningfully changed from the last spoken
  // summary. Three consecutive errors stop the loop with an honest message
  // instead of a silent wedge (same posture as defect #23); all loop state
  // is session-only.
  const lastSpokenRef = useRef('');
  const narrationBusyRef = useRef(false);
  const narrationErrorsRef = useRef(0);
  const speakRef = useRef<((text: string) => Promise<void>) | null>(null);
  useEffect(() => {
    speakRef.current = speakText;
  });

  /** Word-overlap similarity between two normalized scene summaries, 0..1. */
  function sceneSimilarity(a: string, b: string): number {
    const wa = new Set(a.toLowerCase().split(/\s+/).filter(Boolean));
    const wb = new Set(b.toLowerCase().split(/\s+/).filter(Boolean));
    if (wa.size === 0 || wb.size === 0) return 1;
    let shared = 0;
    for (const w of wa) if (wb.has(w)) shared += 1;
    return shared / Math.max(wa.size, wb.size);
  }

  /** Capture one live frame, describe it in the spoken-style mode, and speak
   * it when the scene changed. Throws on any failure so the loop's error
   * backstop can count it. */
  async function narrateOnce(): Promise<void> {
    const video = videoRef.current;
    if (!video || video.videoWidth === 0 || !hasLiveCamera()) {
      throw new Error('Camera is not live.');
    }
    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('The browser did not provide a 2D canvas.');
    ctx.drawImage(video, 0, 0);
    const dataUrl = canvas.toDataURL('image/jpeg', 0.9);
    const savedPath = await tauriApi.saveVisionSnapshot(dataUrl);
    const result = await withVisionTimeout(
      tauriApi.describeImage(savedPath, 'scene_summary'),
      120_000,
      'Scene narration'
    );
    const scene = result.description.trim();
    if (!scene) throw new Error('The vision model returned an empty description.');
    if (lastSpokenRef.current && sceneSimilarity(scene, lastSpokenRef.current) > 0.8) {
      // Same scene as the last spoken summary — the phone's narrator
      // throttles repeats the same way; the blind user is not re-told what
      // they just heard.
      return;
    }
    lastSpokenRef.current = scene;
    setVisionResult(scene);
    await speakRef.current?.(scene);
  }

  useEffect(() => {
    if (!narrationOn) return;
    let cancelled = false;
    const PERIOD = 25_000;
    const tick = async () => {
      if (cancelled || narrationBusyRef.current) return;
      narrationBusyRef.current = true;
      try {
        await narrateOnce();
        narrationErrorsRef.current = 0;
        setNarrationStatus('Narrating — I will speak when what is in front of you changes.');
      } catch (err) {
        narrationErrorsRef.current += 1;
        const msg = err instanceof Error ? err.message : String(err);
        setNarrationStatus(`Scene narration problem: ${msg}`);
        if (narrationErrorsRef.current >= 3) {
          setNarrationOn(false);
          setNarrationStatus('Live narration stopped after repeated problems. Fix the issue above, then press Narrate My Surroundings again.');
        }
      } finally {
        narrationBusyRef.current = false;
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), PERIOD);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      // Session-only state: a stopped loop forgets the last spoken scene.
      lastSpokenRef.current = '';
      narrationErrorsRef.current = 0;
    };
  }, [narrationOn]);

  /** One-press "what's in front of me": capture, describe in the short
   * spoken-style mode, and speak — regardless of the assist toggles. */
  async function whatsInFront() {
    if (isProcessingVision || narrationBusyRef.current) return;
    setIsProcessingVision(true);
    setVisionError('');
    try {
      if (!hasLiveCamera()) {
        await startCamera();
        const video = videoRef.current;
        for (let i = 0; i < 30 && (!video || video.videoWidth === 0); i++) {
          await new Promise(resolve => setTimeout(resolve, 100));
        }
      }
      const video = videoRef.current;
      if (!video || video.videoWidth === 0 || !hasLiveCamera()) {
        throw new Error('Camera could not start. Check the camera connection and try again.');
      }
      const canvas = document.createElement('canvas');
      canvas.width = video.videoWidth;
      canvas.height = video.videoHeight;
      const ctx = canvas.getContext('2d');
      if (!ctx) throw new Error('The browser did not provide a 2D canvas.');
      ctx.drawImage(video, 0, 0);
      const dataUrl = canvas.toDataURL('image/jpeg', 0.9);
      setSnapshots(prev => [dataUrl, ...prev].slice(0, 8));
      const savedPath = await tauriApi.saveVisionSnapshot(dataUrl);
      setImagePath(savedPath);
      const result = await withVisionTimeout(
        tauriApi.describeImage(savedPath, 'scene_summary'),
        120_000,
        'Scene description'
      );
      setVisionResult(result.description);
      await speakText(result.description);
    } catch (err) {
      setVisionError(`What's-in-front failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsProcessingVision(false);
    }
  }

  /** Largest text spoken in the blind-aid auto-speak lane. The audio runtime
   * has a hard 180s inference deadline (live-caught 2026-09-13, defect #25:
   * a 1847-char Hindi description hit "local audio runtime exceeded 180
   * seconds"; omnivoice runs ~0.3-0.4 s/char on CPU), so a full screen
   * description cannot be synthesized in one call. ~280 chars finishes in
   * roughly two minutes worst-case — the first sentences of a description
   * are what a blind user needs immediately; the full text is right above. */
  const SPOKEN_EXCERPT_LIMIT = 280;

  /** Trim text to the spoken excerpt limit, ending on a sentence boundary
   * when one exists in range. Returns the excerpt and whether it was cut. */
  function spokenExcerpt(text: string): { excerpt: string; trimmed: boolean } {
    const trimmedText = text.trim();
    if (trimmedText.length <= SPOKEN_EXCERPT_LIMIT) return { excerpt: trimmedText, trimmed: false };
    const head = trimmedText.slice(0, SPOKEN_EXCERPT_LIMIT);
    const cut = Math.max(
      head.lastIndexOf('. '),
      head.lastIndexOf('! '),
      head.lastIndexOf('? '),
      head.lastIndexOf('\n')
    );
    const excerpt = cut > 40 ? head.slice(0, cut + 1) : head;
    return { excerpt, trimmed: true };
  }

  /** Speak text through the vault speech engine, reusing the voice-lab
   * player element so the description is audible immediately. */
  async function speakText(text: string) {
    if (!vaultRoot || !text.trim()) return;
    setSpeechNotice('');
    const { excerpt, trimmed } = spokenExcerpt(text);
    try {
      // Live-caught 2026-09-13 (defect #24): a ~1400-char description takes
      // well over 90s to synthesize on CPU; the old 90s bound rejected
      // mid-synthesis and the empty catch below swallowed it — the blind
      // user got text but never heard it, with no error. The bound now
      // matches the describe bound (the two stages take comparable time).
      const result = await withVisionTimeout(
        tauriApi.synthesizeSpeech(excerpt, vaultRoot, ttsLanguage),
        300_000,
        'Speech synthesis'
      );
      if (result.audio_path) {
        setTtsAudioPath(result.audio_path);
        if (trimmed) {
          setSpeechNotice('Spoken the beginning of the description — the complete text is shown above.');
        }
        requestAnimationFrame(() => {
          const el = speechAudioRef.current;
          if (el) {
            el.src = tauriApi.convertFileSrc(result.audio_path as string);
            el.play().catch(() => {
              // Autoplay is allowed app-wide (--autoplay-policy=
              // no-user-gesture-required, defect #21) so the description is
              // spoken even after the long describe+synthesize chain; this
              // catch only covers unusual WebView refusals, where the audio
              // element's controls remain the manual fallback.
            });
          }
        });
      } else {
        // Live-caught 2026-09-13 (defect #25): the backend reports the real
        // cause here (e.g. "local audio runtime exceeded 180 seconds") —
        // surface it instead of a generic "no audio" (the old wording hid
        // the deadline from both the user and the acceptance tests).
        setSpeechNotice(`Spoken description failed: ${result.error || 'synthesis returned no audio'}`);
      }
    } catch (err) {
      // Speech is an enhancement for the description; the visible text
      // result is the durable output, so a TTS failure is non-fatal — but a
      // blind user relying on spoken output must be TOLD it failed (defect
      // #24: this used to be a silent swallow).
      setSpeechNotice(`Spoken description unavailable: ${err instanceof Error ? err.message : String(err)}`);
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
                      {/* Phone-parity one-press scene query (2026-09-14): the
                          user's phone blind aid answers "what is in front of
                          me" from one big button; the desktop now does the
                          same, in the short spoken-style voice. */}
                      <button
                        className="btn btn-primary"
                        onClick={() => void whatsInFront()}
                        disabled={isProcessingVision || narrationOn}
                        title="Capture what the camera sees right now, describe it in spoken style, and speak it aloud"
                        style={{ width: '100%', padding: '14px 16px', fontSize: '15px', marginBottom: '12px' }}
                      >
                        👁️ What's in front of me?
                      </button>
                      <div style={{ display: 'flex', gap: '8px', marginBottom: '12px' }}>
                        <button
                          className="btn btn-primary btn-sm"
                          onClick={startCamera}
                          disabled={cameraActive && hasLiveCamera()}
                        >
                          {cameraActive && hasLiveCamera() ? 'Camera On' : 'Start Camera'}
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
                        {/* Phone-parity live narration (2026-09-14): the
                            phone's BlindAidManager keeps watching and speaks
                            scene changes; the desktop loop does the same,
                            with change-detection so it does not repeat
                            itself. */}
                        <button
                          className="btn btn-sm"
                          onClick={() => {
                            setNarrationStatus('');
                            setNarrationOn(on => !on);
                          }}
                          disabled={!cameraActive && !narrationOn}
                          style={narrationOn ? { borderColor: 'var(--success)', color: 'var(--success)' } : undefined}
                          title="Continuously watch the camera and speak what changes in front of you"
                        >
                          {narrationOn ? 'Stop Narrating' : 'Narrate My Surroundings'}
                        </button>
                      </div>

                      {narrationStatus && (
                        <div
                          role="status"
                          style={{
                            marginBottom: '12px',
                            padding: '8px 12px',
                            background: 'var(--bg-primary)',
                            color: 'var(--text-secondary)',
                            borderRadius: 'var(--radius-sm)',
                            border: '1px solid var(--border)',
                            fontSize: '13px',
                          }}
                        >
                          {narrationStatus}
                        </div>
                      )}

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
                            Captured snapshots (saved to the vision pipeline; Describe speaks them when Screen Reader Description is on)
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
                          {/* Chat alignment (2026-09-14): the newest frame can
                              continue in the chat panel, where the full agent
                              can act on it, not just describe it. */}
                          <button
                            className="btn btn-secondary btn-sm"
                            style={{ marginTop: '8px' }}
                            onClick={() =>
                              askInChat({
                                imageDataUrl: snapshots[0],
                                text: 'What is in front of me? Describe the scene briefly and read out any visible text.',
                              })
                            }
                            disabled={!snapshots[0]}
                            title="Send the newest snapshot to the chat panel with a scene question"
                          >
                            Ask in Chat
                          </button>
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
                        {screenReaderDescription && (
                          <button
                            className="btn btn-success btn-sm"
                            onClick={describeScreen}
                            disabled={isProcessingVision}
                            title="Capture this app window and describe what is on screen"
                          >
                            Describe Screen
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

                      {/* Chat alignment (2026-09-14): OCR text and vision
                          results do not dead-end in this panel — hand them to
                          the chat panel where the full agent (reading,
                          reasoning, tools) can act on them. */}
                      {visionResult && (
                        <button
                          className="btn btn-secondary btn-sm"
                          style={{ marginTop: '8px' }}
                          onClick={() =>
                            askInChat({
                              text: `I extracted this with the vision lane (OCR/describe) on this device:\n\n${visionResult.slice(0, 4000)}\n\nWhat is it? Answer in plain language.`,
                            })
                          }
                          title="Continue with this text in the chat panel"
                        >
                          Ask in Chat
                        </button>
                      )}

                      {speechNotice && (
                        <div
                          role="status"
                          style={{
                            marginTop: '8px',
                            padding: '8px 12px',
                            background: 'var(--bg-primary)',
                            borderRadius: 'var(--radius-sm)',
                            border: '1px solid var(--border)',
                            fontSize: '13px',
                          }}
                        >
                          {speechNotice}
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
                      ref={speechAudioRef}
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
