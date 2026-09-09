import { useState, useRef, useEffect, useCallback } from 'react';
import { tauriApi } from '../lib/tauri';
import type { ConversationTurn as TauriConversationTurn, Content } from '../lib/tauri';

interface ChatMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp: number;
  steps?: AgentStep[];
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
}

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
  const fileInputRef = useRef<HTMLInputElement>(null);
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

  const handleAttachImages = (files: FileList | null) => {
    if (!files) return;
    const readers: Promise<string>[] = [];
    for (const file of Array.from(files)) {
      if (pendingImages.length + readers.length >= 4) break;
      if (!file.type.startsWith('image/')) continue;
      readers.push(
        new Promise(resolve => {
          const reader = new FileReader();
          reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '');
          reader.onerror = () => resolve('');
          reader.readAsDataURL(file);
        }),
      );
    }
    void Promise.all(readers).then(dataUrls => {
      const valid = dataUrls.filter(url => url.startsWith('data:image/'));
      if (valid.length === 0) return;
      setPendingImages(prev => [...prev, ...valid].slice(0, 4));
    });
    if (fileInputRef.current) fileInputRef.current.value = '';
  };

  const removePendingImage = (index: number) => {
    setPendingImages(prev => prev.filter((_, i) => i !== index));
  };

  const handleSend = async () => {
    if (!input.trim() || isGenerating) return;

    const images = pendingImages;
    const userMessage: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: images.length > 0 ? `${input.trim()}\n\n[${images.length} image(s) attached]` : input.trim(),
      timestamp: Date.now(),
    };

    setMessages(prev => [...prev, userMessage]);
    setInput('');
    setPendingImages([]);
    setIsGenerating(true);
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
          input.trim(),
          conversationHistory,
          conversationIdRef.current,
          fullAccess,
          images,
        );
        // Harness returns counts, not structured per-tool steps. Surface the
        // real route + counts as an honest telemetry line (no fabricated tool
        // names).
        const telemetry =
          harness.tool_calls > 0 || harness.steps > 1
            ? `Harness ${harness.route} · ${harness.steps} step(s) · ${harness.tool_calls} tool call(s) · ${harness.elapsed_ms}ms`
            : null;
        assistantMessage = {
          id: crypto.randomUUID(),
          role: 'assistant',
          content: harness.output,
          timestamp: Date.now(),
          steps: telemetry ? [{ type: 'Thinking', text: telemetry }] : undefined,
        };
      } catch (harnessErr) {
        // Rollback to the legacy ReAct agent. A "command not registered" style
        // error means this build simply lacks the bridge; anything else is a
        // real bridge failure worth surfacing in the console.
        const harnessMsg = harnessErr instanceof Error ? harnessErr.message : String(harnessErr);
        if (!/harness_chat|not.*registered|no such command|not found|unavailable/i.test(harnessMsg)) {
          console.warn('Harness bridge fell back to legacy agent:', harnessMsg);
        }
        const result = await tauriApi.agentChat(input.trim(), conversationHistory);
        assistantMessage = {
          id: crypto.randomUUID(),
          role: 'assistant',
          content: result.final_text,
          timestamp: Date.now(),
          steps: result.steps,
        };
      }
      setMessages(prev => [...prev, assistantMessage]);
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      if (errorMsg.includes('Failed to connect') || errorMsg.includes('ECONNREFUSED') || errorMsg.includes('llama-server')) {
        setServerError('Cannot connect to Gemma 4. Load the model in Settings → Model Manager.');
      } else {
        setServerError(errorMsg);
      }
    } finally {
      setIsGenerating(false);
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
            <div className="chat-bubble">
              <span className="spinner" />
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
                {' '}(workspace: %USERPROFILE%\UnoOneAgent · audited + budgeted)
              </span>
            </span>
          </label>
        </div>
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
        <div className="chat-input-row">
          <input
            ref={fileInputRef}
            type="file"
            accept="image/png,image/jpeg,image/webp,image/gif"
            multiple
            style={{ display: 'none' }}
            onChange={e => handleAttachImages(e.target.files)}
          />
          <button
            className="btn"
            onClick={() => fileInputRef.current?.click()}
            disabled={isGenerating || modelStatus === 'not_loaded' || pendingImages.length >= 4}
            title="Attach images (up to 4) — the model sees them via its mmproj vision encoder"
            style={{ padding: '8px' }}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
            </svg>
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
