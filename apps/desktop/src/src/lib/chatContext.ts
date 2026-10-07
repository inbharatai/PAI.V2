/** Pure, deterministic Stage 1 model-context selection. Displayed messages and
 * encrypted MESSAGE records are never changed here. No runtime imports: this
 * module also runs in Node's native TypeScript-strip test runner. */
export interface ChatContextMetadata {
  provenance: 'archive' | 'live';
  session_id: string;
  pair_id: string;
  task_id?: string;
  /** Typed/transcribed input, never the composed attachment payload. */
  user_text?: string;
  has_attachments?: boolean;
  /** A stopped UI bubble is not a completed assistant response. */
  cancelled?: boolean;
}

export interface ContextMessage {
  id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  context?: ChatContextMetadata;
}

export interface ActiveChatTask {
  id: string;
  session_id: string;
  topicTokens: readonly string[];
  /** Only the bounded archive actually selected, not the whole vault/session. */
  archiveMessageIds: readonly string[];
}

export interface ContextLimits {
  maxTurns: number;
  maxChars: number;
  maxMessageChars: number;
}

export const DEFAULT_CONTEXT_LIMITS: Readonly<ContextLimits> = Object.freeze({
  maxTurns: 20,
  maxChars: 24000,
  maxMessageChars: 6000,
});

export type ContextReason = 'greeting' | 'new-task' | 'active-followup'
  | 'named-continuation' | 'continuation-not-found' | 'continuation-ambiguous' | 'empty-input';

export interface ChatContextSelection {
  history: { role: 'user' | 'assistant'; content: string }[];
  activeTask: ActiveChatTask;
  reason: ContextReason;
  note: string;
  truncated: boolean;
  selectedMessageIds: string[];
}

export interface ChatContextRequest {
  userText: string;
  hasAttachments: boolean;
  messages: readonly ContextMessage[];
  activeTask: ActiveChatTask | null;
  sessionId: string;
  /** Supplied by the caller; the selector never generates time/random IDs. */
  newTaskId: string;
  limits?: Partial<ContextLimits>;
}

interface MessagePair {
  user: ContextMessage;
  assistant: ContextMessage;
  index: number;
}
interface ArchivedTask {
  pairs: MessagePair[];
  tokens: string[];
}

// NFKC handles full-width input; Unicode token boundaries retain named topics
// such as Café Étoile. No locale-sensitive casing or wall-clock recency scoring.
const normalize = (text: string) => text.normalize('NFKC').toLowerCase().trim();
const words = (text: string) => normalize(text).match(/[\p{L}\p{N}]+/gu) ?? [];
const STOP_WORDS = new Set(('a an the our my your their this that these those it its we i me you us '
  + 'to for of on in at with and or as is are was be have has do does can could would please '
  + 'continue continuing resume restart revisit return back pick up carry proceed yes yeah yep okay ok '
  + 'go ahead fix make more less shorter longer again now next about what how why when '
  + 'plan project task work conversation previous old last same new another create build write implement '
  + 'develop help add update analyze explain tell show need want').split(/\s+/));
const topicTokens = (text: string) => [...new Set(words(text).filter(word => !STOP_WORDS.has(word)))].slice(0, 64);
const userTextOf = (message: ContextMessage) => message.context?.user_text
  ?? (message.context?.has_attachments ? '' : message.content);
const overlaps = (left: readonly string[], right: readonly string[]) => left.filter(token => right.includes(token)).length;

const GREETINGS = new Set(('hi|hello|hey|hi there|hello there|hey there|greetings|howdy|namaste|namaskar|'
  + 'नमस्ते|नमस्कार|good morning|good afternoon|good evening|hola|bonjour|こんにちは|你好').split('|'));

/** Greeting-only contract shared with Rust chat_context.rs. Do NOT use NFKC
 * here: compatibility alphabets and other punctuation are not greeting forms. */
export function normalizeGreetingText(text: string): string {
  return text.replace(/[\uFF01-\uFF5E]/gu, char => String.fromCharCode(char.charCodeAt(0) - 0xFEE0))
    .replace(/\u3000/gu, ' ')
    .toLowerCase()
    .replace(/[\x21-\x2F\x3A-\x40\x5B-\x60\x7B-\x7E。！？，、；：…—–·«»“”‘’¿¡\u2600-\u27BF\u{1F300}-\u{1FAFF}]/gu, '')
    .replace(/\uFE0F|\u200D/gu, '')
    .replace(/[\p{White_Space}\uFEFF]+/gu, ' ').trim();
}

function isGreeting(text: string): boolean {
  // A greeting must be the entire typed input, not a prefix or attachment.
  return GREETINGS.has(normalizeGreetingText(text));
}

function isContinuation(text: string): boolean {
  return /(?:^|\s)(?:continue|resume|restart|revisit|proceed|pick\s+up|carry\s+on)(?:\s|$)/u.test(words(text).join(' '));
}

function isDeictic(text: string): boolean {
  const clean = words(text).filter(word => word !== 'please').join(' ');
  return /^(?:yes|yeah|yep|ok|okay|sure|thanks|thank you|continue|proceed|go ahead|carry on|do it|fix it|try again|what next|and then|make it (?:shorter|longer|better)|yes (?:continue|go ahead))$/u.test(clean);
}

function continuationName(text: string): { text: string; timeFramingRemoved: boolean } {
  const clean = normalize(text);
  // Bounded request framing only, not a date query. Do not erase time words
  // globally: a typed task may actually be named Yesterday Atlas, for example.
  const name = isContinuation(clean) ? clean.replace(/\s+(?:from|on)\s+(?:yesterday|today|(?:last|this)\s+(?:night|morning|afternoon|evening|week|month))\s*[.!?。！？]*$/u, '') : clean;
  return { text: name, timeFramingRemoved: name !== clean };
}

function matchesNamedAnchor(tokens: readonly string[], anchor: readonly string[]): boolean {
  // Require the full informative name and at least two adjacent name tokens.
  // No assistant words, attachment words, synonyms, or recency tie-breaking.
  return tokens.length >= 2 && overlaps(tokens, anchor) === tokens.length
    && tokens.some((token, index) => index > 0
      && anchor.indexOf(token) === anchor.indexOf(tokens[index - 1]) + 1);
}

function isTopicFollowup(text: string, userTopics: readonly string[], assistantTopics: readonly string[] = []): boolean {
  const tokens = topicTokens(text);
  if (!tokens.length) return false;
  const clean = words(text).join(' ');
  if (/(?:^|\s)(?:instead|unrelated|(?:separate|different|new|another)\s+(?:task|project)|switch topics)(?:\s|$)/u.test(clean)) return false;
  const newRequest = /^(?:(?:please|now|can|could|would|will|you|help|me|to|i|want|need|let|us|lets)\s+)*(?:build|create|write|implement|develop|start|plan)\s/u.test(clean);
  // New feature vocabulary is allowed when the edited target is referential;
  // a standalone build/create request below still resets even with shared nouns.
  const edit = /(?:^|\s)(?:add|update|change|fix|make|preserve|include|improve|implement|build|create|develop|validate|test|tests|testing|refactor|remove|extend|write)(?:\s|$)/u.test(clean);
  // A bare reference ends the target or leads into a clause. A determiner
  // followed by a noun ("for this wedding/customer") names another object;
  // it must go through the verified named-target check, never this shortcut.
  const bareReferenceEnd = '(?:$|\\s+(?:and|but|then|please|now|again|so|with|without)\\b)';
  const referentialEdit = edit && (
    new RegExp('(?:^|\\s)(?:to|in|on|for|of|into)\\s+(?:it|this|that)' + bareReferenceEnd, 'u').test(clean)
    || new RegExp('(?:^|\\s)(?:make|fix|update|change|improve|refactor|extend|test)\\s+(?:it|this|that)' + bareReferenceEnd, 'u').test(clean));
  // Explicit named targets are independent of how many new implementation
  // words follow them. A shared framework noun alone is not a named target.
  const target = normalize(text).match(/(?:^|\s)(?:for|to|in|on)\s+([^,;:.\n]+)/u)?.[1]
    ?.split(/\b(?:add|update|change|fix|make|preserve|include|improve|implement|validate|refactor|remove|extend)\b/u)[0];
  const namedTarget = Boolean(target && matchesNamedAnchor(topicTokens(target), userTopics));
  if (referentialEdit || (namedTarget && edit)) return true;
  if (newRequest) return false;
  const topics = [...userTopics, ...assistantTopics];
  const common = overlaps(tokens, topics);
  return common > 0 && common / tokens.length >= 0.5;
}

function completedPairs(messages: readonly ContextMessage[]): MessagePair[] {
  const pairs: MessagePair[] = [];
  // Only explicit adjacent user/assistant pairs with matching provenance can
  // enter context. Orphans, unknown legacy metadata, system and cancelled UI
  // messages remain visible but cannot accidentally become model instructions.
  for (let index = 0; index < messages.length - 1; index++) {
    const user = messages[index];
    const assistant = messages[index + 1];
    const u = user.context;
    const a = assistant.context;
    if (user.role !== 'user' || assistant.role !== 'assistant' || !u || !a
      || !u.pair_id || !u.session_id || !['archive', 'live'].includes(u.provenance)
      || u.cancelled || a.cancelled || u.provenance !== a.provenance
      || u.session_id !== a.session_id || u.pair_id !== a.pair_id || u.task_id !== a.task_id
      || !user.content.trim() || !assistant.content.trim()) continue;
    pairs.push({ user, assistant, index });
    index++;
  }
  return pairs;
}

function archivedTasks(pairs: readonly MessagePair[]): ArchivedTask[] {
  const tasks: ArchivedTask[] = [];
  // session_id isolates provenance, but is NOT a task ID. Even the same old
  // session can contain a new unrelated task. Legacy MESSAGE records have no
  // task ID; reconstruct a conservative boundary using typed user turns only.
  const lastBySession = new Map<string, ArchivedTask>();
  for (const pair of pairs) {
    const meta = pair.user.context!;
    if (meta.provenance !== 'archive') continue;
    const text = userTextOf(pair.user);
    const last = lastBySession.get(meta.session_id);
    const lastMeta = last?.pairs[0].user.context;
    const sameExplicitTask = Boolean(meta.task_id && meta.task_id === lastMeta?.task_id);
    const relatedLegacy = !meta.task_id && !lastMeta?.task_id && !isGreeting(text)
      && (isDeictic(text) || (last && isTopicFollowup(text, last.tokens)));
    if (last && (sameExplicitTask || relatedLegacy)) {
      last.pairs.push(pair);
    } else {
      const task = { pairs: [pair], tokens: topicTokens(text) };
      tasks.push(task);
      lastBySession.set(meta.session_id, task);
    }
  }
  return tasks;
}

function limitsOf(overrides?: Partial<ContextLimits>): ContextLimits {
  const limit = (key: keyof ContextLimits) => {
    const value = overrides?.[key];
    return typeof value === 'number' && Number.isFinite(value) && value >= 0
      ? Math.min(DEFAULT_CONTEXT_LIMITS[key], Math.floor(value)) : DEFAULT_CONTEXT_LIMITS[key];
  };
  return { maxTurns: limit('maxTurns'), maxChars: limit('maxChars'), maxMessageChars: limit('maxMessageChars') };
}

function clip(text: string, cap: number): string {
  if (text.length <= cap) return text;
  const marker = '… [context truncated]';
  const suffix = cap > marker.length ? marker : '';
  let end = Math.max(0, cap - suffix.length);
  // Do not split a UTF-16 surrogate pair at the budget boundary.
  if (end > 0 && /[\uD800-\uDBFF]/u.test(text[end - 1])) end--;
  // A marker alone is not retained source text (e.g. a leading surrogate
  // needs two units). Let the pair bounder omit both sides instead.
  return end > 0 ? text.slice(0, end) + suffix : '';
}

function boundPairs(pairs: readonly MessagePair[], limits: ContextLimits) {
  const kept: { pair: MessagePair; userContent: string; assistantContent: string }[] = [];
  let remaining = limits.maxChars;
  let truncated = false;
  for (let index = pairs.length - 1; index >= 0; index--) {
    if ((kept.length + 1) * 2 > limits.maxTurns || remaining < 2 || limits.maxMessageChars < 1) {
      truncated = true;
      break;
    }
    const pair = pairs[index];
    const userContent = clip(pair.user.content, Math.min(limits.maxMessageChars, Math.floor(remaining / 2)));
    const assistantContent = clip(pair.assistant.content, Math.min(limits.maxMessageChars, remaining - userContent.length));
    truncated ||= userContent !== pair.user.content || assistantContent !== pair.assistant.content;
    // Clipping must not manufacture an empty member of a completed pair. Do
    // not substitute stale older pairs if this newest remaining pair cannot fit.
    if (!userContent.trim() || !assistantContent.trim()) {
      truncated = true;
      break;
    }
    kept.unshift({ pair, userContent, assistantContent });
    remaining -= userContent.length + assistantContent.length;
  }
  return { kept, truncated };
}

export function selectChatContext(request: ChatContextRequest): ChatContextSelection {
  const { messages, userText, hasAttachments, sessionId, newTaskId } = request;
  const pairs = completedPairs(messages);
  const active = request.activeTask?.session_id === sessionId ? request.activeTask : null;
  const archiveIds = new Set(active?.archiveMessageIds ?? []);
  const activePairs = active ? pairs.filter(pair => {
    const meta = pair.user.context!;
    return meta.provenance === 'archive'
      ? archiveIds.has(pair.user.id) && archiveIds.has(pair.assistant.id)
      : meta.session_id === sessionId && meta.task_id === active.id;
  }) : [];
  // Named-resume authority is strictly user-typed. Assistant details are a
  // separate, lower-authority hint for ordinary local followups only; echoes
  // from an attachment-bearing pair never acquire even that hint authority.
  const userAnchors = [active?.topicTokens ?? [], ...activePairs.map(pair => topicTokens(userTextOf(pair.user)))];
  const activeUserTopics = [...new Set(userAnchors.flat())].slice(0, 128);
  const assistantTopics = [...new Set(activePairs.slice(-2)
    .filter(pair => pair.user.context!.has_attachments === false)
    .flatMap(pair => topicTokens(pair.assistant.content)))].slice(0, 64);
  const name = continuationName(userText);
  const typedTopics = topicTokens(name.text);
  let reason: ContextReason = 'new-task';
  let candidates: MessagePair[] = [];
  let keepActive = false;
  let topics = typedTopics;

  if (!userText.trim() && !hasAttachments) {
    reason = 'empty-input';
  } else if (!hasAttachments && isGreeting(userText)) {
    reason = 'greeting';
  } else if (active && activePairs.length > 0 && (isDeictic(userText)
    || (isContinuation(userText)
      ? userAnchors.some(anchor => matchesNamedAnchor(typedTopics, anchor))
      : isTopicFollowup(userText, activeUserTopics, assistantTopics)))) {
    reason = 'active-followup';
    keepActive = true;
    candidates = activePairs;
    topics = [...new Set([...active.topicTokens, ...typedTopics])].slice(0, 64);
  } else if (isContinuation(userText) || isDeictic(userText)) {
    // Require at least two informative, contiguous user-typed name tokens in
    // an archive task's user anchor. Never search assistant prose or payloads,
    // never choose the newest task on an unknown/ambiguous match. One-word or
    // synonymous names conservatively require a clearer user instruction.
    const matching = archivedTasks(pairs).filter(task => matchesNamedAnchor(typedTopics, task.tokens));
    if (matching.length === 1) {
      reason = 'named-continuation';
      candidates = matching[0].pairs;
      topics = matching[0].tokens;
    } else {
      reason = matching.length > 1 ? 'continuation-ambiguous' : 'continuation-not-found';
    }
  }

  const { kept, truncated } = boundPairs(candidates, limitsOf(request.limits));
  const history = kept.flatMap(({ userContent, assistantContent }) => [
    { role: 'user' as const, content: userContent },
    { role: 'assistant' as const, content: assistantContent },
  ]);
  const selectedMessageIds = kept.flatMap(({ pair }) => [pair.user.id, pair.assistant.id]);
  const activeTask: ActiveChatTask = {
    id: keepActive ? active!.id : newTaskId,
    session_id: sessionId,
    topicTokens: [...topics],
    archiveMessageIds: kept.filter(({ pair }) => pair.user.context!.provenance === 'archive')
      .flatMap(({ pair }) => [pair.user.id, pair.assistant.id]),
  };
  const explanations: Record<ContextReason, string> = {
    greeting: 'standalone greeting; no prior task history sent',
    'new-task': 'new task boundary; no prior task history sent',
    'empty-input': 'no typed task; no prior task history sent',
    'active-followup': 'within-task followup; only the active task is in context',
    'named-continuation': 'explicit named continuation; only one matching archived task is in context',
    'continuation-not-found': 'no clear archived task name match; no archive sent — specify the task name',
    'continuation-ambiguous': 'ambiguous archived task name; no archive sent — clarify the task/session',
  };
  const note = `Context: ${explanations[reason]} (${history.length} messages).`
    + (name.timeFramingRemoved ? ' Bounded time framing removed from the task name; no date filtering performed.' : '')
    + (truncated ? ' History truncated to bounded recent pairs/characters.' : '')
    + ' Visible history and encrypted records are unchanged.';
  return { history, activeTask, reason, note, truncated, selectedMessageIds };
}
