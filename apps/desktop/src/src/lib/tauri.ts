/**
 * UnoOne Power — Tauri API bindings
 * Type-safe bridge between React frontend and Rust backend
 *
 * In production (Tauri runtime available), all calls go to the Rust backend.
 * In development without Tauri, calls throw errors — no mock data.
 */

import { invoke as tauriInvoke, convertFileSrc } from '@tauri-apps/api/core';

export interface VaultInfo {
  detected: boolean;
  vault_root: string;
  vault_id: string;
  startup_state: StartupPhase;
  validation_failures: ValidationFailure[];
}

export type StartupPhase =
  | 'STARTING'
  | 'WAITING_FOR_PAI'
  | 'VALIDATING_PAI'
  | 'PAI_INVALID'
  | 'PAI_CONNECTED'
  | 'CHECKING_ASSETS'
  | 'WAITING_FOR_UNLOCK'
  | 'UNLOCKING'
  | 'SCANNING_HOST'
  | 'SELECTING_BACKEND'
  | 'STARTING_MODEL'
  | 'VERIFYING_MODEL'
  | 'READY'
  | 'LIMITED_MODE'
  | 'DISCONNECTED'
  | 'ERROR'
  | 'SHUTTING_DOWN';

export interface ValidationFailure {
  code: string;
  path?: string;
  message: string;
}

export interface StartupStatus {
  phase: StartupPhase;
  vault_root?: string;
  vault_id?: string;
  validation_failures: ValidationFailure[];
}

export interface VaultUnlockResult {
  success: boolean;
  vault_id: string;
  error: string;
}

export interface VaultSetupResult {
  success: boolean;
  vault_id: string;
  recovery_key: string;
  error: string;
}

export interface HardwareProfile {
  total_ram_gb: number;
  available_ram_gb: number;
  cpu_count: number;
  cpu_speed_ghz: number;
  gpu_name: string;
  gpu_vram_gb: number;
  os_name: string;
  os_version: string;
  has_cuda: boolean;
  has_metal: boolean;
  has_vulkan: boolean;
  usb_speed: string;
}

export interface VaultStatus {
  is_connected: boolean;
  is_unlocked: boolean;
  vault_id: string;
  profile_name: string;
  used_space_gb: number;
  total_space_gb: number;
}

export interface ModelInfo {
  name: string;
  model_type: string;
  quantization: string;
  file_size_gb: number;
  context_length: number;
  available: boolean;
  path: string;
  mmproj_path?: string;
}

export interface ModelConfig {
  model_path: string;
  context_size: number;
  batch_size: number;
  threads: number;
  gpu_layers: number;
  temperature: number;
  top_p: number;
  top_k: number;
  repeat_penalty: number;
  max_tokens: number;
  mmproj_path?: string;
}

export type Content = string | ContentPart[];

export interface ContentPart {
  type: 'text' | 'image_url';
  text?: string;
  image_url?: { url: string };
}

export interface ConversationTurn {
  role: 'user' | 'assistant' | 'tool';
  content: Content;
  tool_calls?: ToolCallResult[];
  tool_call_id?: string;
}

export interface ToolCallResult {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

export interface InferenceRequest {
  prompt: string;
  system_prompt?: string;
  conversation_history: ConversationTurn[];
  max_tokens?: number;
  temperature?: number;
  stop_sequences?: string[];
}

export interface InferenceResponse {
  text: string;
  tokens_generated: number;
  tokens_per_second: number;
  model_id: string;
}

export type AccelerationBackend = 'CUDA' | 'METAL' | 'VULKAN' | 'CPU';
export type SecurityLevel = 'STANDARD' | 'RELAXED' | 'OFF';
export type ModelStatus = 'NOT_LOADED' | 'LOADING' | 'LOADED' | 'GENERATING' | 'ERROR';
export type FeatureStatus =
  | 'VERIFIED_WORKING'
  | 'BUILDS_NOT_RUNTIME_TESTED'
  | 'IMPLEMENTED_NOT_TESTED'
  | 'PARTIALLY_IMPLEMENTED'
  | 'NOT_IMPLEMENTED'
  | 'BLOCKED_BY_ENVIRONMENT'
  | 'FAILED';

export interface ToolAction {
  action_id: string;
  tool_name: string;
  parameters: Record<string, unknown>;
  confidence: number;
  raw_output: string;
}

export interface SafetyVerdict {
  action_id: string;
  approved: boolean;
  reason: string;
  risk_level: 'SAFE' | 'LOW' | 'MEDIUM' | 'HIGH' | 'CRITICAL';
  modified_parameters: Record<string, unknown> | null;
}

export interface RecordingBookmark {
  timestamp_seconds: number;
  label: string | null;
}

export interface RecordingOutcome {
  privacy_level: string;
  samples_captured: number;
  persisted_audio: boolean;
  persisted_transcript: boolean;
  persisted_summary: boolean;
  retention_verified: boolean;
  temp_audio_deletion_confirmed: boolean | null;
  warnings: string[];
  user_message: string;
}

export interface RecordingSession {
  id: string;
  title: string;
  state: 'IDLE' | 'RECORDING' | 'PAUSED' | 'PROCESSING' | 'STOPPED' | 'ERROR';
  recording_type: string;
  privacy_level: string;
  started_at: string | null;
  stopped_at: string | null;
  duration_seconds: number;
  bookmarks: RecordingBookmark[];
  source_platform: string;
  source_device_id: string;
  audio_path: string | null;
  transcript_path: string | null;
  summary_path: string | null;
  sample_rate: number;
  channels: number;
  outcome: RecordingOutcome | null;
}

export interface DocumentMetadata {
  id: string;
  title: string;
  document_type: string;
  file_path: string;
  file_size_bytes: number;
  created_at: string;
  modified_at: string;
  source_platform: string;
  tags: string[];
  page_count: number | null;
  word_count: number | null;
}

export interface AccessibilityStatus {
  screen_reader_detected: boolean;
  high_contrast: boolean;
  reduced_motion: boolean;
  font_scale: number;
  screen_reader_name: string;
}

export interface OcrResult {
  text: string;
  confidence: number | null;
  language: string;
  processing_time_ms: number;
}

export interface BlindViewResult {
  description: string;
  objects: DetectedObject[];
  confidence: number | null;
}

export interface DetectedObject {
  label: string;
  confidence: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface CameraInfo {
  devices: Array<{ name: string; device_id: string; status: string }>;
  capture_backend: string;
}

export interface AgentStep {
  type: 'Thinking' | 'ToolCall' | 'ToolResult' | 'InvalidToolCall' | 'SafetyBlock' | 'FinalResponse';
  tool?: string;
  args?: Record<string, unknown>;
  result?: string;
  reason?: string;
  text?: string;
  confidence?: number | null;
  approved?: boolean;
}

export interface AgentResult {
  final_text: string;
  steps: AgentStep[];
  iterations: number;
}

// Unified Harness text plane (production path). The legacy agent_chat remains
// as an explicit rollback until on-device acceptance proves parity. Harness
// returns a deterministic route + step/tool counts; the canonical chat history
// stays in UNOONE MESSAGE records (passed in as read-only context, not
// duplicated into Harness memory).
export interface HarnessChatResult {
  session_id: string;
  route: string;
  route_reason: string;
  output: string;
  steps: number;
  tool_calls: number;
  event_count: number;
  elapsed_ms: number;
  model_id: string;
  memory_namespace: string;
}

// InBharat Audio adapter status. production_ready is true only when the real
// audio.cpp CLI passes its hash-bound readiness + acceptance gate; otherwise
// the legacy Whisper/Piper voice path is retained.
export interface BharatAudioStatus {
  configured: boolean;
  enabled: boolean;
  production_ready: boolean;
  reason: string;
  upstream_commit: string | null;
  asr_family: string | null;
  tts_family: string | null;
}

export interface SecurityVerificationResult {
  vault_id: string;
  manifest_valid: boolean;
  hmac_valid: boolean;
  entries_verified: number;
  entries_failed: number;
  total_entries: number;
  errors: string[];
}

export interface VoiceStatus {
  stt: string;
  tts: string;
  language: string;
}

export interface AppSettings {
  security_level: string;
  auto_lock_minutes: number;
  model_name: string;
  temperature: number;
  max_tokens: number;
  context_size: number;
  gpu_layers: number;
}

export interface AccessibilitySettingsInput {
  high_contrast: boolean;
  reduced_motion: boolean;
  font_scale: number;
  stt_language: string;
  tts_language: string;
}

export interface VaultDomainCounts {
  memories: number;
  chats: number;
  recordings: number;
  documents: number;
  settings: number;
  audit: number;
}

export interface DesktopCapabilityProfile {
  vault: FeatureStatus;
  recording: FeatureStatus;
  browser: FeatureStatus;
  vision: FeatureStatus;
  voice: FeatureStatus;
  model: FeatureStatus;
  agent: FeatureStatus;
  documents: FeatureStatus;
  security: FeatureStatus;
  hardware: FeatureStatus;
  accessibility: FeatureStatus;
  usb: FeatureStatus;
  generated_at_utc: string;
  notes: string[];
}

export interface BrowserConfig {
  headless?: boolean;
  user_data_dir?: string;
  proxy?: string;
  viewport_width?: number;
  viewport_height?: number;
  disable_images?: boolean;
  disable_javascript?: boolean;
  accept_languages?: string;
  user_agent?: string;
}

// Typed browser actions. There is deliberately NO ExecuteScript variant:
// arbitrary model-generated script execution was a defect, not a feature.
export type BrowserAction =
  | { type: 'Navigate'; url: string }
  | { type: 'Back' }
  | { type: 'Forward' }
  | { type: 'Reload' }
  | { type: 'ExtractPageText' }
  | { type: 'ExtractElementText'; selector: string }
  | { type: 'Click'; selector: string }
  | { type: 'Type'; selector: string; text: string }
  | { type: 'FillForm'; fields: Array<{ selector: string; value: string }> }
  | { type: 'Scroll'; direction: 'Up' | 'Down'; amount: number }
  | { type: 'Wait'; milliseconds: number }
  | { type: 'GetPageInfo' }
  | { type: 'Screenshot' }
  | { type: 'Close' }
  | { type: 'ClearSession' };

export interface BrowserActionResult {
  success: boolean;
  verified: boolean;
  data: unknown;
  error: string | null;
  user_message: string;
  current_url: string | null;
  current_title: string | null;
  screenshot_path: string | null;
  screenshot_sha256: string | null;
}

// Tauri invoke — works in Tauri runtime only
//
// Tauri 2 maps JS argument keys to Rust parameters using camelCase by
// default: a Rust parameter `vault_root` must be sent as `vaultRoot`.
// The desktop API surface historically used snake_case keys, which made
// every command with a multi-word argument fail at runtime with
// "missing required key". Convert top-level keys once, here, so all
// call sites keep their snake_case shape and match the Rust side.
function snakeToCamelKey(key: string): string {
  return key.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const converted = args
    ? Object.fromEntries(Object.entries(args).map(([k, v]) => [snakeToCamelKey(k), v]))
    : args;
  return tauriInvoke<T>(command, converted);
}

export const tauriApi = {
  // Vault
  detectVault: () => invoke<VaultInfo>('detect_vault'),
  getStartupStatus: () => invoke<StartupStatus>('get_startup_status'),
  setStartupLimited: () => invoke<void>('set_startup_limited'),
  unlockVault: (password: string, vaultRoot: string) =>
    invoke<VaultUnlockResult>('unlock_vault', { password, vault_root: vaultRoot }),
  // Prototype dev bypass — only exists in binaries built with the dev-bypass cargo feature.
  devBypassUnlock: () => invoke<VaultUnlockResult>('dev_bypass_unlock'),
  setupVault: (password: string, profileName: string | null, vaultRoot: string) =>
    invoke<VaultSetupResult>('setup_vault', { password, profile_name: profileName, vault_root: vaultRoot }),
  lockVault: () => invoke<void>('lock_vault'),
  getHardwareProfile: () => invoke<HardwareProfile>('get_hardware_profile'),
  getVaultStatus: () => invoke<VaultStatus>('get_vault_status'),

  // Model management
  listModels: (vaultRoot: string) => invoke<ModelInfo[]>('list_models', { vault_root: vaultRoot }),
  detectAcceleration: () => invoke<AccelerationBackend[]>('detect_acceleration'),
  getModelConfig: () => invoke<ModelConfig>('get_model_config'),
  getModelStatus: () => invoke<ModelStatus>('get_model_status'),
  startModelServer: (config: ModelConfig, vaultRoot: string) =>
    invoke<number>('start_model_server', { config, vault_root: vaultRoot }),
  stopModelServer: () => invoke<void>('stop_model_server'),
  checkFileExists: (path: string) => invoke<boolean>('check_file_exists', { path }),

  // Safety guard
  getSecurityLevel: () => invoke<SecurityLevel>('get_security_level'),
  setSecurityLevel: (level: SecurityLevel) => invoke<string>('set_security_level', { level }),
  reviewToolAction: (action: ToolAction, securityLevel: SecurityLevel) =>
    invoke<SafetyVerdict>('review_tool_action', { action, security_level: securityLevel }),

  // Recording
  startRecording: (recordingType: string, privacyLevel: string, vaultRoot: string) =>
    invoke<RecordingSession>('start_recording', { recording_type: recordingType, privacy_level: privacyLevel, vault_root: vaultRoot }),
  pauseRecording: () => invoke<RecordingSession>('pause_recording'),
  resumeRecording: () => invoke<RecordingSession>('resume_recording'),
  stopRecording: () => invoke<RecordingSession>('stop_recording'),
  addBookmark: (label: string | null) => invoke<RecordingSession>('add_bookmark', { label }),

  // Documents
  listDocuments: (vaultRoot: string) => invoke<DocumentMetadata[]>('list_documents', { vault_root: vaultRoot }),
  searchMemories: (query: { query: string; memory_types: string[]; limit: number; min_relevance: number }, vaultRoot: string) =>
    invoke<Array<{ id: string; memory_type: string; title: string; preview: string; relevance: number; created_at: string }>>('search_memories', { query, vault_root: vaultRoot }),

  // Accessibility (vision/OCR via local model)
  getAccessibilityStatus: () => invoke<AccessibilityStatus>('get_accessibility_status'),
  performOcr: (imagePath: string) => invoke<OcrResult>('perform_ocr', { image_path: imagePath }),
  describeImage: (imagePath: string) => invoke<BlindViewResult>('describe_image', { image_path: imagePath }),
  getCameraInfo: () => invoke<CameraInfo>('get_camera_info'),
  encodeImageForVision: (imagePath: string) => invoke<string>('encode_image_for_vision', { image_path: imagePath }),

  // Security
  emergencyLock: (vaultRoot: string) => invoke<{ success: boolean; keys_cleared: boolean; vault_locked: boolean; timestamp: string }>('emergency_lock', { vault_root: vaultRoot }),
  generateManifest: (vaultRoot: string) => invoke<VaultInfo & { entries: number; manifest_sha256: string }>('generate_manifest', { vault_root: vaultRoot }),
  verifyManifest: (vaultRoot: string) => invoke<SecurityVerificationResult>('verify_manifest', { vault_root: vaultRoot }),
  recoverFromCrash: (vaultRoot: string) => invoke<{ state: string; recovered_files: number; rolled_back_files: number; errors: string[] }>('recover_from_crash', { vault_root: vaultRoot }),

  // Vault state (D7 additions)
  vaultIsUnlocked: () => invoke<boolean>('vault_is_unlocked'),
  vaultReadRecord: (recordId: string) => invoke<string>('vault_read_record', { record_id: recordId }),
  vaultWriteRecord: (params: {
    recordType: string;
    contentBase64: string;
    privacyLevel?: string;
    parentRecordId?: string;
  }) => invoke<string>('vault_write_record', {
    record_type: params.recordType,
    content_base64: params.contentBase64,
    privacy_level: params.privacyLevel,
    parent_record_id: params.parentRecordId,
  }),

  // Agent loop (D2) — retained as an explicit rollback path. The production
  // text plane is harnessChat below; agentChat is only used if the Harness
  // bridge is unavailable on this build.
  agentChat: (message: string, conversationHistory: ConversationTurn[]) =>
    invoke<AgentResult>('agent_chat', { message, conversation_history: conversationHistory }),
  // Unified Harness text plane (production). Routes L0/L1/L2/L3 deterministically
  // and runs the single agent loop against the verified 127.0.0.1 llama-server.
  harnessChat: (
    message: string,
    conversationHistory: ConversationTurn[],
    conversationId: string | null,
    allowWorkspaceGoal: boolean,
  ) =>
    invoke<HarnessChatResult>('harness_chat', {
      message,
      conversation_history: conversationHistory,
      conversation_id: conversationId,
      allow_workspace_goal: allowWorkspaceGoal,
    }),
  // InBharat Audio adapter — production_ready is true only when the real
  // audio.cpp CLI passes its hash-bound readiness + acceptance gate.
  getBharatAudioStatus: (vaultRoot: string) =>
    invoke<BharatAudioStatus>('get_bharat_audio_status', { vault_root: vaultRoot }),
  checkModelHealth: () => invoke<Record<string, unknown>>('check_model_health'),
  sendChatCompletion: (request: InferenceRequest) => invoke<InferenceResponse>('send_chat_completion', { request }),

  // Voice module (D4)
  getVoiceStatus: (vaultRoot: string, language: string) => invoke<VoiceStatus>('get_voice_status', { vault_root: vaultRoot, language }),
  transcribeAudio: (audioPath: string, vaultRoot: string, language: string) =>
    invoke<{ text: string; language: string; confidence: number | null; status: string }>('transcribe_audio', { audio_path: audioPath, vault_root: vaultRoot, language }),
  synthesizeSpeech: (text: string, vaultRoot: string, language: string) =>
    invoke<{ audio_path: string | null; duration_seconds: number | null; sample_rate: number; status: string; error: string | null }>('synthesize_speech', { text, vault_root: vaultRoot, language }),

  // Browser workspace — the backend executes actions against the real webview
  // window and reports what actually happened (verified only when the page
  // returned a real success payload).
  startBrowserSession: (config: BrowserConfig | undefined, windowLabel: string) =>
    invoke<BrowserActionResult>('browser_start_session', { config, window_label: windowLabel }),
  stopBrowserSession: () => invoke<BrowserActionResult>('browser_stop_session'),
  executeBrowserAction: (action: BrowserAction, confirmed = false) =>
    invoke<BrowserActionResult>('browser_execute', { action, confirmed }),
  getBrowserBridgeScript: () => invoke<string>('get_browser_bridge_script'),
  browserEval: (windowLabel: string, script: string) =>
    invoke<string>('browser_eval', { window_label: windowLabel, script }),

  // Settings and configuration
  getVersion: () => invoke<string>('get_version'),
  setSettings: (settings: AppSettings, vaultRoot: string) => invoke<string>('set_settings', { settings, vault_root: vaultRoot }),
  getSettings: (vaultRoot: string) => invoke<AppSettings>('get_settings', { vault_root: vaultRoot }),
  setAccessibilityStatus: (settings: AccessibilitySettingsInput, vaultRoot: string) =>
    invoke<string>('set_accessibility_status', { settings, vault_root: vaultRoot }),
  getAccessibilitySettings: (vaultRoot: string) => invoke<AccessibilitySettingsInput>('get_accessibility_settings', { vault_root: vaultRoot }),
  getVaultDomainCounts: (vaultRoot: string) => invoke<VaultDomainCounts>('get_vault_domain_counts', { vault_root: vaultRoot }),

  // Unified capability profile
  getDesktopCapabilityProfile: () => invoke<DesktopCapabilityProfile>('get_desktop_capability_profile'),

  // Utility: convert a filesystem path to a webview-loadable URL
  convertFileSrc,
};
