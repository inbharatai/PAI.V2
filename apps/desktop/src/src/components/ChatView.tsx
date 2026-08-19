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
  const [modelStatus, setModelStatus] = useState<'unknown' | 'loaded' | 'not_loaded' | 'error'>('unknown');
  const [serverError, setServerError] = useState('');
  const [expandedSteps, setExpandedSteps] = useState<Set<string>>(new Set());
  // Stable Harness conversation namespace for this chat session. Harness memory
  // is long-term only; canonical chat history stays in UNOONE MESSAGE records
  // passed in as read-only context, never duplicated into Harness memory.
  const conversationIdRef = useRef<string>(crypto.randomUUID());
  const messagesEndRef = useRef<HTMLDivElement>(null);

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
      setModelStatus('not_loaded');
      return false;
    }
  }, []);

  useEffect(() => {
    checkModelStatus();
  }, [checkModelStatus]);

  const handleSend = async () => {
    if (!input.trim() || isGenerating) return;

    const userMessage: ChatMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: input.trim(),
      timestamp: Date.now(),
    };

    setMessages(prev => [...prev, userMessage]);
    setInput('');
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
          false,
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
        <div className="chat-input-row">
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