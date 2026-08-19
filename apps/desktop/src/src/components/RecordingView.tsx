import { useState, useEffect, useRef, useCallback } from 'react';
import { tauriApi, type RecordingSession } from '../lib/tauri';

type RecordingType = 'VOICE_MEMO' | 'MEETING' | 'LECTURE' | 'INTERVIEW' | 'NOTE';
type PrivacyLevel = 'FULL' | 'TRANSCRIPT_ONLY' | 'SUMMARY_ONLY' | 'PRIVATE_SESSION';

interface Recording {
  id: string;
  title: string;
  date: string;
  duration: string;
  status: 'draft' | 'transcribed' | 'summarized';
  type: RecordingType;
  privacy: PrivacyLevel;
}

const RECORDING_TYPE_LABELS: Record<RecordingType, string> = {
  VOICE_MEMO: 'Voice Memo',
  MEETING: 'Meeting',
  LECTURE: 'Lecture',
  INTERVIEW: 'Interview',
  NOTE: 'Quick Note',
};

// Only modes the backend can actually honour are selectable.
// SUMMARY_ONLY is disabled: no summarisation capability exists anywhere in the
// product, so keeping it selectable would silently retain nothing while the
// UI implies a summary. The backend reports SUMMARIZATION_NOT_IMPLEMENTED; we
// do not present an option that cannot keep its promise.
const PRIVACY_LABELS: Record<PrivacyLevel, string> = {
  FULL: 'Full (audio + transcript)',
  TRANSCRIPT_ONLY: 'Transcript Only (no audio saved)',
  SUMMARY_ONLY: 'Summary Only — unavailable (summarisation not implemented)',
  PRIVATE_SESSION: 'Private Session (nothing saved)',
};

const PRIVACY_DISABLED: PrivacyLevel[] = ['SUMMARY_ONLY'];

function mapSessionToRecording(session: RecordingSession): Recording {
  const date = session.started_at
    ? new Date(session.started_at).toLocaleDateString()
    : new Date().toLocaleDateString();

  return {
    id: session.id,
    title: session.title,
    date,
    duration: formatTime(session.duration_seconds),
    status: 'draft',
    type: session.recording_type as RecordingType,
    privacy: session.privacy_level as PrivacyLevel,
  };
}

const formatTime = (seconds: number): string => {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  return `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
};

export function RecordingView() {
  const [isRecording, setIsRecording] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [recordingType, setRecordingType] = useState<RecordingType>('VOICE_MEMO');
  const [privacyLevel, setPrivacyLevel] = useState<PrivacyLevel>('FULL');
  const [bookmarks, setBookmarks] = useState<number>(0);
  const [vaultRoot, setVaultRoot] = useState('');
  const [error, setError] = useState('');
  const timerRef = useRef<number | null>(null);

  // Detect vault root
  const detectVault = useCallback(async () => {
    try {
      const info = await tauriApi.detectVault();
      if (info.detected) {
        setVaultRoot(info.vault_root);
      }
    } catch {
      // Use default vault root
    }
  }, []);

  useEffect(() => {
    detectVault();
  }, [detectVault]);

  useEffect(() => {
    if (isRecording && !isPaused) {
      timerRef.current = window.setInterval(() => {
        setElapsed(prev => prev + 1);
      }, 1000);
    } else {
      if (timerRef.current) {
        clearInterval(timerRef.current);
        timerRef.current = null;
      }
    }
    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [isRecording, isPaused]);

  const handleRecord = async () => {
    setError('');
    if (isRecording) {
      // Stop recording via Tauri API — must succeed to save
      let session: RecordingSession;
      try {
        session = await tauriApi.stopRecording();
      } catch (e) {
        setError(`Failed to stop recording: ${e instanceof Error ? e.message : String(e)}`);
        return; // Do NOT update UI if backend failed
      }
      setIsRecording(false);
      setIsPaused(false);
      // Surface the truthful outcome: what was actually persisted and any
      // warning the backend emitted (e.g. SUMMARIZATION_NOT_IMPLEMENTED,
      // NO_TRANSCRIPT_PRODUCED).
      if (session.outcome) {
        if (session.outcome.warnings.length > 0) {
          setError(`${session.outcome.user_message} [${session.outcome.warnings.join(', ')}]`);
        } else {
          setError(session.outcome.user_message);
        }
      }
      setRecordings(prev => [mapSessionToRecording(session), ...prev]);
      setElapsed(0);
      setBookmarks(0);
    } else {
      // Start recording — must succeed to enter recording state
      try {
        await tauriApi.startRecording(recordingType, privacyLevel, vaultRoot);
        setIsRecording(true);
        setElapsed(0);
        setBookmarks(0);
      } catch (e) {
        setError(`Failed to start recording: ${e instanceof Error ? e.message : String(e)}`);
        // Do NOT enter recording state if backend failed
      }
    }
  };

  const handlePause = async () => {
    setError('');
    if (isPaused) {
      try {
        await tauriApi.resumeRecording();
        setIsPaused(false);
      } catch (e) {
        setError(`Failed to resume: ${e instanceof Error ? e.message : String(e)}`);
      }
    } else {
      try {
        await tauriApi.pauseRecording();
        setIsPaused(true);
      } catch (e) {
        setError(`Failed to pause: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
  };

  const handleBookmark = async () => {
    setError('');
    try {
      await tauriApi.addBookmark(null);
      setBookmarks(prev => prev + 1);
    } catch (e) {
      setError(`Failed to add bookmark: ${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const handleCancel = () => {
    setIsRecording(false);
    setIsPaused(false);
    setElapsed(0);
    setBookmarks(0);
  };

  return (
    <div className="recording-view">
      <div className="main-header">
        <h2>Recordings</h2>
        <div className="main-header-actions">
          <button className="btn btn-secondary btn-sm">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
            Import Audio
          </button>
        </div>
      </div>

      {error && (
        <div style={{
          padding: '8px 16px',
          background: 'rgba(239,68,68,0.1)',
          borderTop: '1px solid rgba(239,68,68,0.3)',
          fontSize: '13px',
          color: 'var(--danger)',
        }}>
          {error}
        </div>
      )}

      <div className="main-body" style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '24px', paddingTop: '24px' }}>
        {/* Recording type and privacy selectors */}
        {!isRecording && (
          <div style={{ display: 'flex', gap: '12px', width: '100%', maxWidth: '600px' }}>
            <div className="input-group" style={{ flex: 1 }}>
              <label>Type</label>
              <select value={recordingType} onChange={e => setRecordingType(e.target.value as RecordingType)}>
                {Object.entries(RECORDING_TYPE_LABELS).map(([key, label]) => (
                  <option key={key} value={key}>{label}</option>
                ))}
              </select>
            </div>
            <div className="input-group" style={{ flex: 1 }}>
              <label>Privacy</label>
              <select value={privacyLevel} onChange={e => setPrivacyLevel(e.target.value as PrivacyLevel)}>
                {Object.entries(PRIVACY_LABELS).map(([key, label]) => (
                  <option key={key} value={key} disabled={PRIVACY_DISABLED.includes(key as PrivacyLevel)}>
                    {label}
                    {PRIVACY_DISABLED.includes(key as PrivacyLevel) ? '' : ''}
                  </option>
                ))}
              </select>
            </div>
          </div>
        )}

        {/* Recording controls */}
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '16px' }}>
          <button
            className={`record-btn ${isRecording ? 'recording' : ''}`}
            onClick={handleRecord}
            aria-label={isRecording ? 'Stop recording' : 'Start recording'}
          >
            <div className="record-btn-inner" />
          </button>

          <div className="recording-timer">
            {isRecording ? formatTime(elapsed) : '00:00:00'}
          </div>

          {isRecording && bookmarks > 0 && (
            <div style={{ fontSize: '12px', color: 'var(--text-muted)' }}>
              🔖 {bookmarks} bookmark{bookmarks !== 1 ? 's' : ''}
            </div>
          )}

          <p style={{ color: 'var(--text-muted)', fontSize: '13px' }}>
            {isRecording ? (isPaused ? 'Paused — tap resume to continue' : 'Recording…') : 'Tap to start recording'}
          </p>

          {isRecording && (
            <div style={{ display: 'flex', gap: '8px' }}>
              <button className="btn btn-secondary btn-sm" onClick={handlePause}>
                {isPaused ? (
                  <>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <polygon points="5 3 19 12 5 21 5 3" />
                    </svg>
                    Resume
                  </>
                ) : (
                  <>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <rect x="6" y="4" width="4" height="16" />
                      <rect x="14" y="4" width="4" height="16" />
                    </svg>
                    Pause
                  </>
                )}
              </button>
              <button className="btn btn-secondary btn-sm" onClick={handleBookmark}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                  <path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" />
                </svg>
                Bookmark
              </button>
              <button className="btn btn-danger btn-sm" onClick={handleCancel}>
                Cancel
              </button>
            </div>
          )}
        </div>

        {/* Recording info during recording */}
        {isRecording && (
          <div style={{ padding: '12px 16px', background: 'var(--bg-tertiary)', borderRadius: 'var(--radius-sm)', fontSize: '12px', color: 'var(--text-secondary)', width: '100%', maxWidth: '600px' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span>Type: {RECORDING_TYPE_LABELS[recordingType]}</span>
              <span>Privacy: {privacyLevel}</span>
            </div>
          </div>
        )}

        {recordings.length > 0 && (
          <div style={{ width: '100%', maxWidth: '600px' }}>
            <h3 style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: 'var(--text-secondary)' }}>
              Recent Recordings
            </h3>
            <div className="recording-list">
              {recordings.map(rec => (
                <div key={rec.id} className="recording-item">
                  <div className="recording-item-icon">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <polygon points="5 3 19 12 5 21 5 3" />
                    </svg>
                  </div>
                  <div className="recording-item-info">
                    <div className="recording-item-title">{rec.title}</div>
                    <div className="recording-item-meta">
                      {rec.date} · {RECORDING_TYPE_LABELS[rec.type]} · {rec.status}
                    </div>
                  </div>
                  <div className="recording-item-duration">{rec.duration}</div>
                </div>
              ))}
            </div>
          </div>
        )}

        {!isRecording && recordings.length === 0 && (
          <div className="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
              <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
              <line x1="12" y1="19" x2="12" y2="23" />
              <line x1="8" y1="23" x2="16" y2="23" />
            </svg>
            <h3>No recordings yet</h3>
            <p>Tap the record button to start capturing audio. All recordings are encrypted and saved to your Pocket USB.</p>
          </div>
        )}

        {/* STT/TTS info */}
        <div style={{ padding: '16px', background: 'var(--bg-secondary)', border: '1px solid var(--border)', borderRadius: 'var(--radius-md)', width: '100%', maxWidth: '600px' }}>
          <h4 style={{ fontSize: '13px', fontWeight: 600, color: 'var(--text-secondary)', marginBottom: '8px' }}>
            🎙️ Speech Processing Pipeline
          </h4>
          <div style={{ fontSize: '12px', color: 'var(--text-muted)', lineHeight: 1.6 }}>
            <p><strong>STT:</strong> Whisper (bundled build only) — offline, on-device transcription</p>
            <p><strong>TTS:</strong> Piper (bundled build only) — offline synthesis</p>
            <p><strong>Summarization:</strong> not yet implemented — SUMMARY_ONLY is disabled until it exists</p>
            <p style={{ marginTop: '8px', color: 'var(--accent)' }}>Pipeline: Audio → Encrypted vault → STT → Transcript → Vault (summary step pending)</p>
          </div>
        </div>
      </div>
    </div>
  );
}