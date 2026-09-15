import { useState, useRef, useEffect, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { tauriApi } from '../lib/tauri';
import type { ConversationTurn as TauriConversationTurn, Content } from '../lib/tauri';

/** Live agent activity streamed from the backend while a run is in flight
 * (2026-09-14: the panel previously showed a bare spinner for minutes while
 * the model wrote whole files — a real run looked frozen). */
interface AgentProgressEvent {
  phase: 'call' | 'result';
  tool: string;
  detail: string;
  code_preview: string | null;
  /** Local wall-clock "HH:MM:SS" stamped by the backend at emit time
   * (defect #37, live-caught 2026-09-14: the user could not tell when each
   * step happened — other agent tools like Codex/GLM timestamp activity). */
  at: string;
}

/** Report agent-run activity so App.tsx can defer the window-blur auto-lock
 * while a task is in flight (defect #31, live-caught 2026-09-14: the 5-min
 * blur lock stopped the model server and killed a long coding run at 3/4
 * files). Dispatched as a window event; App listens and never locks
 * mid-run. */
const setAgentActivity = (active: boolean) => {
  window.dispatchEvent(new CustomEvent('unoone:agent-activity', { detail: { active } }));
};

interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: number;
  steps?: AgentStep[];
  // Set when the user has this assistant message spoken back (STS out).
  audioUrl?: string;
}

/** Non-image attachment staged for the next turn (text/doc content). */
interface PendingFile {
  name: string;
  kind: string;
  truncated: boolean;
  text: string;
}

interface AgentStep {
  type: 'Thinking' | 'ToolCall' | 'ToolResult' | 'InvalidToolCall' | 'SafetyBlock' | 'FinalResponse';
  tool?: string;
  args?: Record<string, unknown>;
  result?: string;
  reason?: string;
  text?: string;
  confidence?: number | null;
  approved?: boolean;
  /** Local wall-clock "HH:MM:SS" from the backend event (defect #37). */
  at?: string;
}

/** Extensions parsed by the backend's audited document extractors. */
const PARSED_DOC_EXTS = ['pdf', 'docx', 'xlsx', 'pptx'];
/** Text-like attachments the WebView reads directly (bounded, client-side). */
const TEXT_FILE_EXTS = [
  'txt', 'md', 'markdown', 'csv', 'tsv', 'json', 'log', 'xml', 'yaml', 'yml',
  'toml', 'ini', 'html', 'htm', 'css', 'js', 'jsx', 'ts', 'tsx', 'py', 'rs',
  'go', 'java', 'kt', 'c', 'h', 'cpp', 'hpp', 'cs', 'sh', 'ps1', 'bat', 'sql',
];
const extOf = (name: string) => name.split('.').pop()?.toLowerCase() || '';

export function ChatView() {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [isGenerating, setIsGenerating] = useState(false);
  // 'loading' = the model server is provably on its way up (startup phase
  // still inside the pre-Ready model path). check_model_health rejects with
  // "manager not initialized" during that whole window, so a plain failure
  // must NOT be shown as "no model loaded" — that's the bug where the chat
  // claimed nothing was loaded while Gemma was actually up.
  const [modelStatus, setModelStatus] = useState<'unknown' | 'loaded' | 'loading' | 'not_loaded' | 'error'>('unknown');
  const [serverError, setServerError] = useState('');
  const [expandedSteps, setExpandedSteps] = useState<Set<string>>(new Set());
  // Pending image attachments for the next turn, as data URLs. The backend
  // re-validates (media type allowlist, base64 decode, 8 MiB per image, 4
  // images max) and hashes each one — this state only previews and transports.
  const [pendingImages, setPendingImages] = useState<string[]>([]);
  // Pending non-image attachments (txt/md/csv/json/code read client-side,
  // pdf/docx/xlsx/pptx parsed by the backend's audited extractors).
  const [pendingFiles, setPendingFiles] = useState<PendingFile[]>([]);
  const [attachError, setAttachError] = useState('');
  const fileInputRef = useRef<HTMLInputElement>(null);
  // STS in: mic capture rides the same audited recording pipeline the
  // Recordings view uses, at TRANSCRIPT_ONLY privacy — audio is transcribed
  // then destroyed, only the encrypted transcript is kept.
  const [vaultRoot, setVaultRoot] = useState('');
  const [isRecording, setIsRecording] = useState(false);
  const [recordSeconds, setRecordSeconds] = useState(0);
  const [micError, setMicError] = useState('');
  const recordTimerRef = useRef<number | undefined>(undefined);
  // STS out: synthesize assistant replies through the offline speech lane.
  const [speakingMessageId, setSpeakingMessageId] = useState<string | null>(null);
  const [autoSpeak, setAutoSpeak] = useState<boolean>(() => {
    try { return localStorage.getItem('unoone.autoSpeak') === 'on'; } catch { return false; }
  });
  const [speechLang, setSpeechLang] = useState<string>(() => {
    try { return localStorage.getItem('unoone.speechLang') || 'en'; } catch { return 'en'; }
  });
  // Stable Harness conversation namespace for this chat session. Harness memory
  // is long-term only; canonical chat history stays in UNOONE MESSAGE records
  // passed in as read-only context, never duplicated into Harness memory.
  const conversationIdRef = useRef<string>(crypto.randomUUID());
  const messagesEndRef = useRef<HTMLDivElement>(null);
  // Full access is the default the user directed: the agent may read/write
  // the host workspace, run allowlisted commands and drive the browser.
  // Every call still passes the audited, budgeted harness pipeline; turning
  // this off drops back to the read-only vault chat lane.
  const [fullAccess, setFullAccess] = useState<boolean>(() => {
    try {
      return localStorage.getItem('unoone.fullAccess') !== 'off';
    } catch {
      return true;
    }
  });
  const toggleFullAccess = (enabled: boolean) => {
    setFullAccess(enabled);
    try {
      localStorage.setItem('unoone.fullAccess', enabled ? 'on' : 'off');
    } catch {
      // Storage unavailable — the toggle still applies for this session.
    }
  };

  // Live agent activity (2026-09-14): the backend emits one event per tool
  // call/result while the run is in flight. Rendered live in the generating
  // bubble, then folded into the message's step pill when the run lands.
  const [liveProgress, setLiveProgress] = useState<AgentProgressEvent[]>([]);
  // Mirror of liveProgress for read-after-await: a state read inside handleSend
  // after the harness call resolves would see the empty snapshot captured at
  // render time, because the events land during the awaited call. The ref is
  // the source of truth for folding; the state drives the live rendering.
  const liveProgressRef = useRef<AgentProgressEvent[]>([]);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<AgentProgressEvent>('agent-progress', event => {
      setLiveProgress(prev => [...prev.slice(-49), event.payload]);
      liveProgressRef.current = [...liveProgressRef.current.slice(-49), event.payload];
    }).then(fn => {
      unlisten = fn;
    }).catch(() => {
      // Without the event stream the run still works; only the live feed is missing.
    });
    return () => unlisten?.();
  }, []);

  // Cross-panel bridge (2026-09-14 OCR/blind-aid alignment): the
  // Accessibility lane hands its frame and follow-up question to this, the
  // one panel, so every capability ends in a single conversation.
  useEffect(() => {
    const onAsk = (e: Event) => {
      const detail = (e as CustomEvent<{ text?: string; imageDataUrl?: string }>).detail || {};
      if (detail.imageDataUrl && detail.imageDataUrl.startsWith('data:image/')) {
        setPendingImages(prev =>
          prev.includes(detail.imageDataUrl!) ? prev : [...prev, detail.imageDataUrl!].slice(0, 4)
        );
      }
      if (detail.text) {
        setInput(prev => (prev.trim() ? prev : detail.text!));
      }
    };
    window.addEventListener('unoone:ask-in-chat', onAsk);
    return () => window.removeEventListener('unoone:ask-in-chat', onAsk);
  }, []);

  /** One-press camera frame → pending image (goes through the audited
   * vision-attachment lane), with a blind-aid style question prefilled. */
  const [isCapturingCamera, setIsCapturingCamera] = useState(false);
  const captureFromCamera = async () => {
    if (isCapturingCamera) return;
    setIsCapturingCamera(true);
    setAttachError('');
    let stream: MediaStream | null = null;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ video: true });
      const video = document.createElement('video');
      video.srcObject = stream;
      video.muted = true;
      video.playsInline = true;
      await video.play();
      // Give the sensor a moment to settle exposure before grabbing the frame.
      const deadline = Date.now() + 3000;
      while (video.videoWidth === 0 && Date.now() < deadline) {
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      const canvas = document.createElement('canvas');
      canvas.width = video.videoWidth;
      canvas.height = video.videoHeight;
      if (canvas.width === 0 || canvas.height === 0) {
        throw new Error('the camera did not produce a frame in time. Try again.');
      }
      canvas.getContext('2d')?.drawImage(video, 0, 0);
      const dataUrl = canvas.toDataURL('image/jpeg', 0.9);
      if (!dataUrl.startsWith('data:image/')) {
        throw new Error('camera capture produced no usable frame. Try again.');
      }
      setPendingImages(prev => [...prev, dataUrl].slice(0, 4));
      setInput(prev =>
        prev.trim()
          ? prev
          : 'What is in front of me? Describe the scene briefly and read out any visible text.'
      );
    } catch (err) {
      setAttachError(`Camera access failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      stream?.getTracks().forEach(track => track.stop());
      setIsCapturingCamera(false);
    }
  };

  /** Capture the app window and attach it — the screen-reader "what is on my
   * screen" flow in the chat panel. The snapshot is read back through the
   * asset protocol so it rides the same audited image lane. */
  const [isCapturingScreen, setIsCapturingScreen] = useState(false);
  const captureScreenToChat = async () => {
    if (isCapturingScreen) return;
    setIsCapturingScreen(true);
    setAttachError('');
    try {
      const path = await tauriApi.captureScreenSnapshot();
      const res = await fetch(tauriApi.convertFileSrc(path));
      if (!res.ok) throw new Error(`could not read the screenshot (${res.status})`);
      const blob = await res.blob();
      const dataUrl = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '');
        reader.onerror = () => reject(new Error('could not decode the screenshot'));
        reader.readAsDataURL(blob);
      });
      if (!dataUrl.startsWith('data:image/')) throw new Error('screenshot came back empty');
      setPendingImages(prev => [...prev, dataUrl].slice(0, 4));
      setInput(prev =>
        prev.trim()
          ? prev
          : 'What is on my screen? Describe the content briefly and read out any visible text.'
      );
    } catch (err) {
      setAttachError(`Screen capture failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsCapturingScreen(false);
    }
  };

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const checkModelStatus = useCallback(async () => {
    try {
      await tauriApi.checkModelHealth();
      setModelStatus('loaded');
      setServerError('');
      return true;
    } catch (err) {
      console.error('[ChatView] check_model_health failed:', err);
      // Before declaring "no model loaded", ask the startup coordinator
      // whether the model path is still in progress. During asset sweep,
      // backend selection and model start/verify, the health command has
      // no manager to report on yet — that is "loading", not "not loaded".
      try {
        const status = await tauriApi.getStartupStatus();
        const modelPathPhases = [
          'STARTING',
          'VALIDATING_PAI',
          'PAI_CONNECTED',
          'CHECKING_ASSETS',
          'WAITING_FOR_UNLOCK',
          'UNLOCKING',
          'SCANNING_HOST',
          'SELECTING_BACKEND',
          'STARTING_MODEL',
          'VERIFYING_MODEL',
        ];
        if (modelPathPhases.includes(status.phase)) {
          setModelStatus('loading');
          return false;
        }
      } catch {
        // Startup status unavailable — fall through to not_loaded below.
      }
      setModelStatus('not_loaded');
      return false;
    }
  }, []);

  useEffect(() => {
    // Poll until the model server is actually up (it now starts only after
    // the background asset sweep completes), instead of probing once and
    // leaving the input permanently disabled.
    let cancelled = false;
    let timer: number | undefined;

    const poll = async () => {
      const ok = await checkModelStatus();
      if (cancelled) return;
      if (!ok) {
        timer = window.setTimeout(poll, 1000);
      }
    };

    void poll();

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [checkModelStatus]);

  // Voice input/output need the vault root (recording pipeline + speech
  // lane). The chat itself does not — so detection failure is silent and
  // only degrades the mic/speaker buttons.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const info = await tauriApi.detectVault();
        if (!cancelled && info.detected) setVaultRoot(info.vault_root);
      } catch {
        // Vault detection unavailable — mic/speak surface their own error.
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // The real expanded agent workspace root for the full-access label (defect
  // #36, live-caught 2026-09-14: the label showed a literal
  // "%USERPROFILE%\UnoOneAgent" and the agent's answers said only "in your
  // workspace" — the user could not find the files the tool built). Falls
  // back to the literal pattern so the label degrades, never blanks.
  const [workspaceRoot, setWorkspaceRoot] = useState('%USERPROFILE%\\UnoOneAgent');
  useEffect(() => {
    let cancelled = false;
    void tauriApi.getWorkspaceRoot()
      .then(root => { if (!cancelled && root) setWorkspaceRoot(root); })
      .catch(() => { /* label keeps the literal pattern */ });
    return () => { cancelled = true; };
  }, []);

  // Never leak the recording timer.
  useEffect(() => () => {
    if (recordTimerRef.current !== undefined) window.clearInterval(recordTimerRef.current);
  }, []);

  const handleAttachImages = (files: FileList | null) => {
    if (!files) return;
    setAttachError('');
    const images: Promise<string>[] = [];
    const docs: Promise<PendingFile | null>[] = [];
    for (const file of Array.from(files)) {
      if (file.type.startsWith('image/')) {
        if (pendingImages.length + images.length >= 4) continue;
        images.push(
          new Promise(resolve => {
            const reader = new FileReader();
            reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '');
            reader.onerror = () => resolve('');
            reader.readAsDataURL(file);
          }),
        );
      } else if (pendingFiles.length + docs.length >= 4) {
        continue;
      } else if (PARSED_DOC_EXTS.includes(extOf(file.name))) {
        // PDF/DOCX/XLSX/PPTX — parsed by the backend's audited extractors.
        docs.push(
          new Promise(resolve => {
            const reader = new FileReader();
            reader.onload = async () => {
              const b64 = String(reader.result || '').split(',')[1] || '';
              try {
                const parsed = await tauriApi.parseAttachedDocument(file.name, b64);
                resolve({ name: file.name, kind: parsed.kind, truncated: parsed.truncated, text: parsed.text });
              } catch (err) {
                setAttachError(`${file.name}: ${err instanceof Error ? err.message : String(err)}`);
                resolve(null);
              }
            };
            reader.onerror = () => { setAttachError(`${file.name}: could not be read`); resolve(null); };
            reader.readAsDataURL(file);
          }),
        );
      } else if (TEXT_FILE_EXTS.includes(extOf(file.name))) {
        // Text-like files — read client-side, bounded.
        if (file.size > 256 * 1024) {
          setAttachError(`${file.name} is over the 256 KB text-attachment limit`);
          continue;
        }
        docs.push(
          new Promise(resolve => {
            const reader = new FileReader();
            reader.onload = () => {
              const text = String(reader.result || '');
              resolve({ name: file.name, kind: 'text', truncated: false, text });
            };
            reader.onerror = () => { setAttachError(`${file.name}: could not be read`); resolve(null); };
            reader.readAsText(file);
          }),
        );
      } else {
        setAttachError(`${file.name}: unsupported attachment type (images, ${PARSED_DOC_EXTS.join('/')}, and text/code files are supported)`);
      }
    }
    void Promise.all(images).then(dataUrls => {
      const valid = dataUrls.filter(url => url.startsWith('data:image/'));
      if (valid.length === 0) return;
      setPendingImages(prev => [...prev, ...valid].slice(0, 4));
    });
    void Promise.all(docs).then(parsed => {
      const ok = parsed.filter((p): p is PendingFile => p !== null);
      if (ok.length === 0) return;
      setPendingFiles(prev => [...prev, ...ok].slice(0, 4));
    });
    if (fileInputRef.current) fileInputRef.current.value = '';
  };

  const removePendingImage = (index: number) => {
    setPendingImages(prev => prev.filter((_, i) => i !== index));
  };

  const removePendingFile = (index: number) => {
    setPendingFiles(prev => prev.filter((_, i) => i !== index));
  };

  /** STS in — mic via the audited recording pipeline (TRANSCRIPT_ONLY). */
  const toggleMic = async () => {
    setMicError('');
    try {
      if (!isRecording) {
        if (!vaultRoot) { setMicError('Vault not detected — voice input needs the Pocket USB.'); return; }
        // The ASR model serves a narrower set than the TTS voice (Qwen3-ASR:
        // en/hi/Hinglish). For any other voice language the mic runs in
        // `auto` — the engine detects what was spoken instead of failing.
        const asrServed = speechLang === 'en' || speechLang === 'hi' || speechLang === 'hinglish';
        await tauriApi.startRecording('VOICE_MEMO', 'TRANSCRIPT_ONLY', vaultRoot, asrServed ? speechLang : 'auto');
        setIsRecording(true);
        setRecordSeconds(0);
        recordTimerRef.current = window.setInterval(() => setRecordSeconds(s => s + 1), 1000);
      } else {
        const session = await tauriApi.stopRecording();
        if (recordTimerRef.current !== undefined) { window.clearInterval(recordTimerRef.current); recordTimerRef.current = undefined; }
        setIsRecording(false);
        const recordId = session.transcript_path?.replace('vault://records/', '');
        if (!recordId) {
          setMicError('No transcript was produced — nothing was heard. Try again closer to the mic, or type your message.');
          return;
        }
        const transcript = await tauriApi.vaultReadRecord(recordId);
        const text = transcript.trim();
        if (!text) {
          setMicError('The transcript came back empty. Try again, or type your message.');
          return;
        }
        setInput(prev => (prev.trim() ? `${prev.trim()} ${text}` : text));
      }
    } catch (err) {
      if (recordTimerRef.current !== undefined) { window.clearInterval(recordTimerRef.current); recordTimerRef.current = undefined; }
      setIsRecording(false);
      setMicError(err instanceof Error ? err.message : String(err));
    }
  };

  /** STS out — speak an assistant reply through the offline TTS lane. */
  const speakMessage = async (msg: ChatMessage) => {
    if (!vaultRoot) { setMicError('Vault not detected — speech output needs the Pocket USB.'); return; }
    try {
      setSpeakingMessageId(msg.id);
      // Long replies are spoken up to a sensible cap; the tail notes the cut.
      const ttsText = msg.content.length > 2000
        ? `${msg.content.slice(0, 2000)}… [reply truncated for speech]`
        : msg.content;
      const result = await tauriApi.synthesizeSpeech(ttsText, vaultRoot, speechLang);
      if (result.error || !result.audio_path) {
        setMicError(result.error || 'Speech synthesis returned no audio.');
        return;
      }
      setMessages(prev => prev.map(m => (m.id === msg.id ? { ...m, audioUrl: tauriApi.convertFileSrc(result.audio_path!) } : m)));
    } catch (err) {
      setMicError(err instanceof Error ? err.message : String(err));
    } finally {
      setSpeakingMessageId(null);
    }
  };

  const handleSend = async () => {
    if (!input.trim() || isGenerating) return;

    const images = pendingImages;
    const files = pendingFiles;
    // Non-image attachments travel as labelled text blocks appended to the
    // prompt (bounded by the parsers / 256 KB client cap upstream), so the
    // model sees their contents directly in this turn.
    const fileBlocks = files.map(f =>
      `\n\n[attached file: ${f.name}${f.kind !== 'text' ? ` (${f.kind})` : ''}${f.truncated ? ' — content truncated' : ''}]\n${f.text}`
    );
    const composedPrompt = `${input.trim()}${fileBlocks.join('')}${
      images.length > 0 ? `\n\n[${images.length} image(s) attached]` : ''
    }`;
    const userMessage: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: composedPrompt,
      timestamp: Date.now(),
    };

    setMessages(prev => [...prev, userMessage]);
    setInput('');
    setPendingImages([]);
    setPendingFiles([]);
    setIsGenerating(true);
    setAgentActivity(true);
    setLiveProgress([]);
    liveProgressRef.current = [];
    setServerError('');

    try {
      const conversationHistory: TauriConversationTurn[] = messages
        .filter(m => m.role === 'user' || m.role === 'assistant')
        .map(msg => ({ role: msg.role as 'user' | 'assistant' | 'tool', content: msg.content as Content }));

      // Production text plane: the unified Harness routes L0/L1/L2/L3 and runs
      // the single agent loop against the verified 127.0.0.1 llama-server. The
      // legacy agent_chat is retained as an explicit rollback until on-device
      // acceptance proves parity (see harness_bridge.rs migration posture).
      let assistantMessage: ChatMessage;
      try {
        const harness = await tauriApi.harnessChat(
          composedPrompt,
          conversationHistory,
          conversationIdRef.current,
          fullAccess,
          images,
        );
        // Harness returns counts, not structured per-tool steps — but the
        // live-progress stream recorded the real tool activity. Fold it into
        // the message's step pill (expandable), with the route + counts as
        // an honest telemetry line.
        const telemetry =
          harness.tool_calls > 0 || harness.steps > 1
            ? `Harness ${harness.route} · ${harness.steps} step(s) · ${harness.tool_calls} tool call(s) · ${harness.elapsed_ms}ms`
            : null;
        const progressSteps: AgentStep[] = liveProgressRef.current.map(ev =>
          ev.phase === 'call'
            ? { type: 'ToolCall', tool: ev.tool, text: ev.detail, at: ev.at }
            : { type: 'ToolResult', tool: ev.tool, result: ev.detail, at: ev.at }
        );
        if (telemetry) {
          progressSteps.unshift({ type: 'Thinking', text: telemetry });
        }
        assistantMessage = {
          id: crypto.randomUUID(),
          role: 'assistant',
          content: harness.output,
          timestamp: Date.now(),
          steps: progressSteps.length > 0 ? progressSteps : undefined,
        };
      } catch (harnessErr) {
        const harnessMsg = harnessErr instanceof Error ? harnessErr.message : String(harnessErr);
        if (fullAccess) {
          // Defect #32 (live-caught 2026-09-14): with full access on, the
          // silent fallback to the read-only legacy agent made the model
          // TRUTHFULLY refuse the task ("As an AI assistant, I do not have
          // direct access to your local file system…") and paste code
          // instead of building it — the user watched a build request turn
          // into a tutorial. The downgrade is never silent now: surface the
          // real pipeline failure and let the user retry. (The legacy
          // fallback stays available for the read-only lane, where the two
          // paths are capability-equivalent.)
          throw new Error(
            `Agent pipeline stopped: ${harnessMsg}. The task was NOT run — no files were written and no commands were executed. Retry once the model is back (its state is in the Model Manager, or reload the app).`
          );
        }
        // Rollback to the legacy ReAct agent — read-only lane only. This
        // lane change must never be silent: the legacy agent has a smaller,
        // vault-read-only toolset, so an answer produced here can truthfully
        // describe fewer abilities than the enabled session. Surface the
        // fallback and its reason as a step the user can read.
        console.warn('Harness bridge fell back to legacy agent:', harnessMsg);
        const result = await tauriApi.agentChat(composedPrompt, conversationHistory);
        const fallbackStep = {
          type: 'Thinking' as const,
          text: `Fell back to the read-only legacy agent (the primary agent pipeline stopped: ${harnessMsg}). This fallback can only read vault records — its answers may understate what this session can do.`,
        };
        assistantMessage = {
          id: crypto.randomUUID(),
          role: 'assistant',
          content: result.final_text,
          timestamp: Date.now(),
          steps: [fallbackStep, ...(result.steps ?? [])],
        };
      }
      setMessages(prev => [...prev, assistantMessage]);
      // STS out: with auto-speak on, the reply is voiced as it lands.
      if (autoSpeak && vaultRoot) void speakMessage(assistantMessage);
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      if (errorMsg.includes('Failed to connect') || errorMsg.includes('ECONNREFUSED') || errorMsg.includes('llama-server')) {
        setServerError('Cannot connect to Gemma 4. Load the model in Settings → Model Manager.');
      } else {
        setServerError(errorMsg);
      }
    } finally {
      setIsGenerating(false);
      setAgentActivity(false);
      liveProgressRef.current = [];
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const toggleSteps = (msgId: string) => {
    setExpandedSteps(prev => {
      const next = new Set(prev);
      if (next.has(msgId)) next.delete(msgId);
      else next.add(msgId);
      return next;
    });
  };

  const formatTime = (ts: number) =>
    new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

  /** Brief summary of tool-use steps (like Gemini's "Used: search" pill) */
  const stepSummary = (steps: AgentStep[]): string | null => {
    const toolNames = steps
      .filter(s => s.type === 'ToolCall' && s.tool)
      .map(s => s.tool!);
    if (toolNames.length === 0) return null;
    return `Used: ${toolNames.join(', ')}`;
  };

  const renderExpandedSteps = (steps: AgentStep[]) => (
    <div style={{
      marginTop: '8px',
      padding: '8px 12px',
      background: 'var(--bg-tertiary, #1a1a2e)',
      borderRadius: '8px',
      fontSize: '12px',
      lineHeight: '1.5',
    }}>
      {steps.map((step, i) => {
        switch (step.type) {
          case 'ToolCall':
            return (
              <div key={i} style={{ color: 'var(--info, #60a5fa)', padding: '2px 0' }}>
                <strong>→ {step.tool}</strong>
                {step.at && (
                  <span style={{ color: 'var(--text-muted, #666)', marginLeft: '6px', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: '11px' }}>
                    {step.at}
                  </span>
                )}
                {step.args && Object.keys(step.args).length > 0 && (
                  <span style={{ color: 'var(--text-secondary, #888)', marginLeft: '6px' }}>
                    {JSON.stringify(step.args).slice(0, 100)}
                  </span>
                )}
              </div>
            );
          case 'ToolResult':
            return (
              <div key={i} style={{ color: 'var(--success, #4ade80)', padding: '2px 0' }}>
                <strong>✓ {step.tool}</strong>
                {step.at && (
                  <span style={{ color: 'var(--text-muted, #666)', marginLeft: '6px', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: '11px' }}>
                    {step.at}
                  </span>
                )}
                <span style={{ color: 'var(--text-secondary, #888)', marginLeft: '6px' }}>
                  {step.result?.slice(0, 120)}{step.result && step.result.length > 120 ? '…' : ''}
                </span>
              </div>
            );
          case 'InvalidToolCall':
            return (
              <div key={i} style={{ color: 'var(--warning, #fbbf24)', padding: '2px 0' }}>
                <strong>⚠ Invalid call: {step.tool}</strong> — {step.reason}
              </div>
            );
          case 'SafetyBlock':
            return (
              <div key={i} style={{ color: 'var(--danger, #f87171)', padding: '2px 0' }}>
                <strong>🛡 Blocked: {step.tool}</strong> — {step.reason}
              </div>
            );
          case 'Thinking':
            return (
              <div key={i} style={{ color: 'var(--text-secondary, #888)', fontStyle: 'italic', padding: '2px 0' }}>
                💭 {step.text?.slice(0, 200)}{step.text && step.text.length > 200 ? '…' : ''}
              </div>
            );
          default:
            return null;
        }
      })}
    </div>
  );

  return (
    <div className="chat-view">
      <div className="chat-messages">
        {messages.length === 0 && (
          <div className="empty-state">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ width: '48px', height: '48px', opacity: 0.5 }}>
              <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
            </svg>
            <h3 style={{ marginTop: '12px', fontWeight: 500 }}>UnoOne</h3>
            <p style={{ color: 'var(--text-secondary, #888)', fontSize: '14px' }}>
              Private AI, running on your encrypted USB. Ask anything.
            </p>
            {(modelStatus === 'loading' || modelStatus === 'unknown') && (
              <div style={{ marginTop: '12px', padding: '12px 16px', background: 'var(--bg-tertiary, #1a1a2e)', borderRadius: '8px', fontSize: '13px', color: 'var(--text-secondary, #888)' }}>
                Model loading… validating the USB package and starting Gemma 4. First load can take a few minutes.
              </div>
            )}
            {modelStatus === 'not_loaded' && (
              <div style={{ marginTop: '12px', padding: '12px 16px', background: 'var(--bg-tertiary, #1a1a2e)', borderRadius: '8px', fontSize: '13px', color: 'var(--text-secondary, #888)' }}>
                No model loaded. Open <strong>Model Manager</strong> to load Gemma 4.
              </div>
            )}
            {modelStatus === 'loaded' && (
              <div style={{ marginTop: '12px', padding: '12px 16px', background: 'rgba(34,197,94,0.08)', borderRadius: '8px', fontSize: '13px', color: 'var(--success, #4ade80)' }}>
                Model ready
              </div>
            )}
          </div>
        )}

        {messages.map(msg => (
          <div key={msg.id} className={`chat-message ${msg.role}`}>
            <div className="chat-avatar">
              {msg.role === 'user' ? 'U' : 'G'}
            </div>
            <div className="chat-bubble">
              <div style={{ whiteSpace: 'pre-wrap', lineHeight: '1.6' }}>{msg.content}</div>

              {msg.role === 'assistant' && (
                <div style={{ marginTop: '6px', display: 'flex', alignItems: 'center', gap: '8px' }}>
                  {!msg.audioUrl ? (
                    <button
                      onClick={() => void speakMessage(msg)}
                      disabled={speakingMessageId === msg.id}
                      title="Speak this reply (offline TTS)"
                      style={{
                        background: 'none',
                        border: '1px solid var(--border-color, #333)',
                        borderRadius: '12px',
                        padding: '3px 10px',
                        fontSize: '11px',
                        color: 'var(--text-secondary, #888)',
                        cursor: speakingMessageId === msg.id ? 'default' : 'pointer',
                      }}
                    >
                      {speakingMessageId === msg.id ? '🔊 synthesizing…' : '🔊 Speak'}
                    </button>
                  ) : (
                    <audio
                      controls
                      src={msg.audioUrl}
                      style={{ width: '100%', maxWidth: '360px', height: '32px' }}
                      aria-label="Spoken reply playback"
                    />
                  )}
                </div>
              )}

              {msg.steps && msg.steps.length > 0 && (
                <div style={{ marginTop: '8px' }}>
                  {/* Collapsible step summary — like Gemini's "Used: tool" pill */}
                  <button
                    onClick={() => toggleSteps(msg.id)}
                    style={{
                      background: 'none',
                      border: '1px solid var(--border-color, #333)',
                      borderRadius: '12px',
                      padding: '3px 10px',
                      fontSize: '11px',
                      color: 'var(--text-secondary, #888)',
                      cursor: 'pointer',
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: '4px',
                    }}
                  >
                    <span>{stepSummary(msg.steps)}</span>
                    <span style={{ fontSize: '9px' }}>
                      {expandedSteps.has(msg.id) ? '▲' : '▼'}
                    </span>
                  </button>
                  {expandedSteps.has(msg.id) && renderExpandedSteps(msg.steps)}
                </div>
              )}

              <div style={{ fontSize: '10px', color: 'var(--text-muted, #666)', marginTop: '4px' }}>
                {formatTime(msg.timestamp)}
              </div>
            </div>
          </div>
        ))}

        {isGenerating && (
          <div className="chat-message assistant">
            <div className="chat-avatar">G</div>
            <div className="chat-bubble" style={{ minWidth: '200px' }}>
              {liveProgress.length === 0 ? (
                <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                  <span className="spinner" />
                  <span style={{ fontSize: '13px', color: 'var(--text-muted, #666)' }}>Thinking…</span>
                </div>
              ) : (
                <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '4px' }}>
                    <span className="spinner" />
                    <span style={{ fontSize: '13px', color: 'var(--text-secondary, #888)' }}>Working…</span>
                  </div>
                  {liveProgress.slice(-8).map((ev, i) => (
                    <div key={i} style={{ fontSize: '12px', color: 'var(--text-secondary, #888)' }}>
                      {ev.at && (
                        <span style={{ color: 'var(--text-muted, #666)', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: '11px', marginRight: '6px' }}>
                          {ev.at}
                        </span>
                      )}
                      {ev.phase === 'call' ? (
                        <span>
                          <span style={{ color: 'var(--text-muted, #666)' }}>→ </span>
                          {ev.detail || ev.tool}
                        </span>
                      ) : (
                        <span>
                          <span style={{ color: 'var(--accent, #4ade80)' }}>✓ </span>
                          {ev.detail || ev.tool}
                        </span>
                      )}
                      {ev.code_preview && (
                        <pre style={{
                          margin: '4px 0 2px 14px',
                          padding: '6px 8px',
                          background: 'var(--surface-secondary, #1a1a1a)',
                          borderRadius: '6px',
                          fontSize: '11px',
                          overflowX: 'auto',
                          maxHeight: '140px',
                          fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
                          whiteSpace: 'pre-wrap',
                        }}>
                          {ev.code_preview}
                        </pre>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      {serverError && (
        <div style={{
          padding: '8px 16px',
          background: 'rgba(239,68,68,0.08)',
          borderTop: '1px solid rgba(239,68,68,0.2)',
          fontSize: '13px',
          color: 'var(--danger, #f87171)',
        }}>
          {serverError}
        </div>
      )}

      <div className="chat-input-area">
        <div style={{
          display: 'flex',
          alignItems: 'center',
          gap: '8px',
          marginBottom: '8px',
          fontSize: '12px',
          color: 'var(--text-secondary, #888)',
        }}>
          <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', userSelect: 'none' }}>
            <input
              type="checkbox"
              checked={fullAccess}
              onChange={e => toggleFullAccess(e.target.checked)}
              disabled={isGenerating}
            />
            <span>
              Full access — read/write files, run commands, drive the browser
              <span style={{ color: 'var(--text-muted, #666)' }}>
                {' '}(workspace: {workspaceRoot} · audited + budgeted)
              </span>
            </span>
          </label>
          <label style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer', userSelect: 'none' }}>
            <input
              type="checkbox"
              checked={autoSpeak}
              onChange={e => {
                setAutoSpeak(e.target.checked);
                try { localStorage.setItem('unoone.autoSpeak', e.target.checked ? 'on' : 'off'); } catch { /* session-only */ }
              }}
            />
            <span>Speak replies aloud (offline TTS)</span>
          </label>
          <label style={{ display: 'flex', alignItems: 'center', gap: '6px', userSelect: 'none' }}>
            <span>Voice language</span>
            <select
              value={speechLang}
              onChange={e => {
                setSpeechLang(e.target.value);
                try { localStorage.setItem('unoone.speechLang', e.target.value); } catch { /* session-only */ }
              }}
              style={{ fontSize: '12px' }}
            >
              <option value="en">English</option>
              <option value="hi">हिन्दी</option>
              <option value="hinglish">Hinglish</option>
              <option value="as">অসমীয়া</option>
              <option value="bn">বাংলা</option>
              <option value="gu">ગુજરાતી</option>
              <option value="kn">ಕನ್ನಡ</option>
              <option value="ml">മലയാളം</option>
              <option value="mr">मराठी</option>
              <option value="ne">नेपाली</option>
              <option value="or">ଓଡ଼ିଆ</option>
              <option value="pa">ਪੰਜਾਬੀ</option>
              <option value="sa">संस्कृतम्</option>
              <option value="ta">தமிழ்</option>
              <option value="te">తెలుగు</option>
              <option value="ur">اردو</option>
            </select>
          </label>
        </div>
        {(micError || attachError) && (
          <div style={{
            padding: '6px 12px',
            marginBottom: '8px',
            background: 'rgba(239,68,68,0.08)',
            borderRadius: '8px',
            fontSize: '12px',
            color: 'var(--danger, #f87171)',
          }}>
            {micError || attachError}
          </div>
        )}
        {pendingImages.length > 0 && (
          <div style={{ display: 'flex', gap: '8px', marginBottom: '8px', flexWrap: 'wrap' }}>
            {pendingImages.map((dataUrl, i) => (
              <div key={i} style={{ position: 'relative' }}>
                <img
                  src={dataUrl}
                  alt={`attachment ${i + 1}`}
                  style={{
                    width: '64px',
                    height: '64px',
                    objectFit: 'cover',
                    borderRadius: '6px',
                    border: '1px solid var(--border-color, #333)',
                  }}
                />
                <button
                  onClick={() => removePendingImage(i)}
                  title="Remove image"
                  style={{
                    position: 'absolute',
                    top: '-6px',
                    right: '-6px',
                    width: '18px',
                    height: '18px',
                    borderRadius: '50%',
                    border: 'none',
                    background: 'var(--danger, #f87171)',
                    color: '#fff',
                    fontSize: '11px',
                    lineHeight: 1,
                    cursor: 'pointer',
                    padding: 0,
                  }}
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        )}
        {pendingFiles.length > 0 && (
          <div style={{ display: 'flex', gap: '8px', marginBottom: '8px', flexWrap: 'wrap' }}>
            {pendingFiles.map((file, i) => (
              <div key={`${file.name}-${i}`} style={{
                display: 'flex', alignItems: 'center', gap: '6px',
                padding: '4px 10px', borderRadius: '8px',
                border: '1px solid var(--border-color, #333)',
                fontSize: '12px', color: 'var(--text-secondary, #888)',
                maxWidth: '280px',
              }}>
                <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  📄 {file.name}{file.truncated ? ' (truncated)' : ''}
                </span>
                <button
                  onClick={() => removePendingFile(i)}
                  title="Remove attachment"
                  style={{ background: 'none', border: 'none', color: 'var(--danger, #f87171)', cursor: 'pointer', padding: 0, fontSize: '13px' }}
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="chat-input-row">
          <input
            ref={fileInputRef}
            type="file"
            accept="image/png,image/jpeg,image/webp,image/gif,.pdf,.docx,.xlsx,.pptx,.txt,.md,.markdown,.csv,.tsv,.json,.log,.xml,.yaml,.yml,.toml,.ini,.html,.htm,.css,.js,.jsx,.ts,.tsx,.py,.rs,.go,.java,.kt,.c,.h,.cpp,.hpp,.cs,.sh,.ps1,.bat,.sql"
            multiple
            style={{ display: 'none' }}
            onChange={e => handleAttachImages(e.target.files)}
          />
          <button
            className="btn"
            onClick={() => fileInputRef.current?.click()}
            disabled={isGenerating || modelStatus === 'not_loaded' || (pendingImages.length >= 4 && pendingFiles.length >= 4)}
            title="Attach images, PDF/DOCX/XLSX/PPTX or text/code files (up to 4 each) — images go to the vision encoder, documents are parsed and their text is shown to the model"
            style={{ padding: '8px' }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
            </svg>
          </button>
          <button
            className="btn"
            onClick={() => void captureFromCamera()}
            disabled={isGenerating || modelStatus === 'not_loaded' || isCapturingCamera}
            title="Capture one frame from the camera and ask the vision model about it"
            style={{
              padding: '8px',
              ...(isCapturingCamera ? { background: 'rgba(74,222,128,0.15)', borderColor: 'var(--accent, #4ade80)' } : {}),
            }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z" />
              <circle cx="12" cy="13" r="4" />
            </svg>
          </button>
          <button
            className="btn"
            onClick={() => void captureScreenToChat()}
            disabled={isGenerating || modelStatus === 'not_loaded' || isCapturingScreen}
            title="Capture the screen and ask the vision model about it"
            style={{
              padding: '8px',
              ...(isCapturingScreen ? { background: 'rgba(74,222,128,0.15)', borderColor: 'var(--accent, #4ade80)' } : {}),
            }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
              <line x1="8" y1="21" x2="16" y2="21" />
              <line x1="12" y1="17" x2="12" y2="21" />
            </svg>
          </button>
          <button
            className="btn"
            onClick={() => void toggleMic()}
            disabled={isGenerating && !isRecording}
            title={isRecording ? 'Stop and transcribe — your words drop into the box (audio is destroyed, only the encrypted transcript is kept)' : 'Speak your message — recorded at Transcript Only privacy, transcribed on-device'}
            style={{
              padding: '8px',
              ...(isRecording ? { background: 'rgba(239,68,68,0.15)', borderColor: 'var(--danger, #f87171)' } : {}),
            }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
              <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
              <line x1="12" y1="19" x2="12" y2="23" />
              <line x1="8" y1="23" x2="16" y2="23" />
            </svg>
            {isRecording && (
              <span style={{ marginLeft: '6px', fontSize: '12px', color: 'var(--danger, #f87171)' }}>
                {Math.floor(recordSeconds / 60)}:{String(recordSeconds % 60).padStart(2, '0')}
              </span>
            )}
          </button>
          <textarea
            className="chat-input"
            placeholder={
              modelStatus === 'not_loaded'
                ? 'Load a model first…'
                : 'Message UnoOne…'
            }
            value={input}
            onChange={e => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            rows={1}
            disabled={isGenerating || modelStatus === 'not_loaded'}
          />
          <button
            className="btn btn-primary"
            onClick={handleSend}
            disabled={!input.trim() || isGenerating || modelStatus === 'not_loaded'}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <line x1="22" y1="2" x2="11" y2="13" />
              <polygon points="22 2 15 22 11 13 2 9 22 2" />
            </svg>
          </button>
        </div>
      </div>
    </div>
  );
}
