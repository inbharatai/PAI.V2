//! ABI v2 — the registration surface ABI v1 never had.
//!
//! ABI v1 can create, route, run and cancel a harness, but it can REGISTER
//! NOTHING: the embedder gets a standalone harness with no model provider
//! (every run fails closed) and no host adapters. ABI v2 adds the missing
//! half over the same core traits the desktop embeds natively:
//!
//! - `ib_harness_builder_create_v2` — the embedding-safe constructor
//!   (`HarnessBuilder::local_embedded`, NO built-in tools) where v1 used the
//!   standalone `local` constructor. Registration is builder-stage, exactly
//!   like the Rust API: the core captures every replaceable implementation at
//!   build time, so the ABI mirrors `create → register… → build`.
//! - `ib_harness_builder_register_model_v2` / `…_tool_v2` / `…_memory_v2` /
//!   `…_permission_v2` — C function-pointer vtables with caller-owned
//!   `user_data`, wrapped in adapter objects. The optional `destroy` callback
//!   fires EXACTLY ONCE per registration: when the registration is rejected,
//!   when the builder is abandoned, or when the last harness referencing the
//!   adapter is destroyed.
//! - `ib_harness_run_v2` — like the v1 run, but the caller selects the
//!   registered provider/model and capability grants, and may request the
//!   tamper-evident audit ledger of the run (the session's hash-chained
//!   events as JSONL, one event per line, chain included).
//!
//! Honest scope of this cycle: the ABI surface + registration tests. The
//! on-device Kotlin/JNI consumer of this surface is a device gate; nothing
//! here flips a production capability flag.
//!
//! Two contract rules worth stating explicitly:
//!
//! - A REJECTED registration (duplicate id, invalid manifest, bad size tag)
//!   consumes the builder: the core's `register_*` methods take `self` by
//!   value and drop it on error, so the builder handle stays alive but every
//!   later call on it returns `IB_STATUS_INVALID_ARGUMENT`. The rejected
//!   adapter's destroy callback fires immediately. The handle must still be
//!   destroyed exactly once.
//! - Data marshalling uses the core's own bounded JSON `Value` (no serde on
//!   the vendored core, no new dependencies). Spans a callback FILLS point to
//!   registration-owned memory kept valid until that callback's next
//!   invocation or the registration's destroy; the library copies them
//!   immediately after each call. Owned outputs of exported functions are
//!   library memory released with `ib_harness_bytes_free_v1`.

use crate::{
    CANCEL_HANDLE_MAGIC, HARNESS_HANDLE_MAGIC, IB_STATUS_CANCELLED, IB_STATUS_DENIED,
    IB_STATUS_INVALID_ARGUMENT, IB_STATUS_OK, IB_STATUS_OPERATION_FAILED, IB_STATUS_PANIC,
    IB_STATUS_RESOURCE_EXHAUSTED, IB_STATUS_UNAVAILABLE, IbByteSpanV1, IbCancellationHandle,
    IbHarnessHandle, IbOwnedBytesV1,
};
use inbharat_harness_core::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use inbharat_harness_core::providers::{
    Capability, FinishReason, MemoryCapabilities, MemoryProvider, MemoryQuery, MemoryRecord,
    MemoryScope, ModelChunk, ModelProvider, ModelRequest, ModelResponse, ModelRole,
    PermissionDecision, PermissionProvider,
};
use inbharat_harness_core::routing::ExecutionLevel;
use inbharat_harness_core::runtime::{HarnessBuilder, RunOptions};
use inbharat_harness_core::session::Session;
use inbharat_harness_core::tools::{
    ConfirmationMode, Determinism, SideEffect, Tool, ToolArguments, ToolContext, ToolManifest,
    ToolOutput,
};
use inbharat_harness_core::value::Value;
use inbharat_harness_core::{CancellationToken, RoutePolicy};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

/// ABI major version of the registration surface.
pub const IB_HARNESS_ABI_VERSION_V2: u32 = 2;

/// Permission decision codes, as filled by the C authorize callback.
pub const IB_DECISION_ALLOW: u8 = 0;
pub const IB_DECISION_ASK: u8 = 1;
pub const IB_DECISION_DENY: u8 = 2;

/// Magic tag for builder handles (distinct from the harness and cancellation
/// tags so a type-confused pointer fails the tag check).
const BUILDER_HANDLE_MAGIC: u64 = 0x4942_4255_494C_0032; // "IBBUILD\0\x32"

// ---------------------------------------------------------------------------
// Function-pointer types (the C vtables)
// ---------------------------------------------------------------------------

/// Optional final release of one registration's `user_data`. Called exactly once.
pub type IbDestroyFn = Option<extern "C" fn(user_data: *mut c_void)>;

/// Emits one streamed chunk from the C provider back into the harness.
/// Returns `IB_STATUS_OK` to continue; nonzero aborts the stream with that status.
pub type IbChunkEmitFn = extern "C" fn(emit_data: *mut c_void, chunk: *const IbModelChunkV2) -> i32;

/// One model dispatch: C receives the request, streams chunks through `emit`,
/// and fills `out_response`. `cancel` (non-null) is polled with
/// `ib_harness_cancel_requested_v2`. All spans in the request and response
/// remain valid for the duration of the call.
pub type IbModelStreamFn = extern "C" fn(
    user_data: *mut c_void,
    request: *const IbModelRequestV2,
    cancel: *mut IbCancellationHandle,
    emit: IbChunkEmitFn,
    emit_data: *mut c_void,
    out_response: *mut IbModelResponseV2,
) -> i32;

/// Validates one tool invocation's arguments (a JSON object) before execution.
pub type IbToolValidateFn =
    extern "C" fn(user_data: *mut c_void, arguments_json: IbByteSpanV1) -> i32;

/// Executes one tool call. `out_output` spans point to memory the registration
/// owns and keeps valid until the same callback's next invocation or the
/// registration's destroy; the harness copies them immediately after the call.
pub type IbToolExecuteFn = extern "C" fn(
    user_data: *mut c_void,
    arguments_json: IbByteSpanV1,
    context: *const IbToolContextV2,
    out_output: *mut IbToolOutputV2,
) -> i32;

/// Retrieves one record; C writes a JSON record span, or an empty span when
/// the record does not exist. Out spans must point to registration-owned
/// memory kept valid until the same callback's next invocation or destroy.
pub type IbMemoryRetrieveFn = extern "C" fn(
    user_data: *mut c_void,
    scope: u8,
    namespace: IbByteSpanV1,
    id: IbByteSpanV1,
    out_record_json: *mut IbByteSpanV1,
) -> i32;

/// Searches records; C writes a JSON array of record objects. The out span
/// must point to registration-owned memory kept valid until the same
/// callback's next invocation or destroy.
pub type IbMemorySearchFn = extern "C" fn(
    user_data: *mut c_void,
    scope: u8,
    namespace: IbByteSpanV1,
    text: IbByteSpanV1,
    limit: u32,
    out_records_json: *mut IbByteSpanV1,
) -> i32;

/// Stores (or updates) one record given as JSON.
pub type IbMemoryWriteFn = extern "C" fn(user_data: *mut c_void, record_json: IbByteSpanV1) -> i32;

/// Deletes one record; C reports whether it existed through `out_found`.
pub type IbMemoryDeleteFn = extern "C" fn(
    user_data: *mut c_void,
    scope: u8,
    namespace: IbByteSpanV1,
    id: IbByteSpanV1,
    out_found: *mut u8,
) -> i32;

/// One authority check; C fills the decision (with fixed-size rule/reason
/// buffers when denying).
pub type IbPermissionAuthorizeFn = extern "C" fn(
    user_data: *mut c_void,
    actor: IbByteSpanV1,
    capability: u8,
    resource: IbByteSpanV1,
    out_decision: *mut IbPermissionDecisionV2,
) -> i32;

// ---------------------------------------------------------------------------
// Size-tagged DTOs
// ---------------------------------------------------------------------------

/// Builder configuration. `system_prefix` may be empty (none). Uses the
/// embedding-safe constructor: NO built-in tools are registered.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbHarnessConfigV2 {
    pub struct_size: u32,
    pub abi_version: u32,
    pub root: IbByteSpanV1,
    pub maximum_level: u8,
    /// 0 = escalation denied, 1 = allowed (v1 hardcoded 1).
    pub allow_explicit_escalation: u8,
    pub system_prefix: IbByteSpanV1,
    pub reserved: [u8; 6],
}

/// One model request, handed to the C stream callback. Message and tool
/// catalogues are marshalled as bounded JSON (the core's own `Value`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbModelRequestV2 {
    pub struct_size: u32,
    pub request_id: IbByteSpanV1,
    pub provider: IbByteSpanV1,
    pub model: IbByteSpanV1,
    pub system: IbByteSpanV1,
    /// JSON array of `{"role":"user","content":"..."}` objects.
    pub messages_json: IbByteSpanV1,
    /// JSON array of `{"id","description","input_schema"}` objects.
    pub tools_json: IbByteSpanV1,
    pub max_output_bytes: usize,
}

/// One streamed chunk. `kind` selects the variant; other fields are read per kind.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbModelChunkV2 {
    pub struct_size: u32,
    /// 0 Start, 1 TextDelta, 2 ReasoningDelta, 3 ToolCall, 4 End, 5 Usage, 6 Finish.
    pub kind: u8,
    pub block: u32,
    pub text: IbByteSpanV1,
    pub call_id: IbByteSpanV1,
    pub tool_id: IbByteSpanV1,
    pub arguments: IbByteSpanV1,
    pub input_units: u64,
    pub output_units: u64,
    /// Only read for `kind == 6` (Finish).
    pub finish_reason: u8,
}

/// The terminal response C fills for one dispatch. The harness assembles
/// `text` from the emitted TextDelta chunks itself; C owns finish + usage.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbModelResponseV2 {
    pub struct_size: u32,
    /// 0 Stop, 1 ToolCalls, 2 Length, 3 Cancelled, 4 Error.
    pub finish_reason: u8,
    pub input_units: u64,
    pub output_units: u64,
}

/// A model provider registration. `models_json` is a JSON array of model ids.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IbModelProviderV2 {
    pub struct_size: u32,
    pub id: IbByteSpanV1,
    pub models_json: IbByteSpanV1,
    pub user_data: *mut c_void,
    pub stream: IbModelStreamFn,
    pub destroy: IbDestroyFn,
}

/// Tool execution context. The Rust `ExecutionBroker` is deliberately NOT
/// exposed in ABI v2: brokered host work stays on the Rust side; C tools do
/// their own work inside the non-bypassable confirmation/audit/budget wrap.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbToolContextV2 {
    pub struct_size: u32,
    pub actor: IbByteSpanV1,
    pub level: u8,
    pub cancel: *mut IbCancellationHandle,
}

/// The tool result handed back by the C execute callback (spans borrowed for
/// the duration of the callback). `value_json` is parsed into the structured
/// output; `model_content` is the model-visible text.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbToolOutputV2 {
    pub struct_size: u32,
    pub value_json: IbByteSpanV1,
    pub model_content: IbByteSpanV1,
}

/// One tool registration. Enum fields are the C encodings of the manifest
/// enums; `required_capabilities` bit `i` maps to `Capability` discriminant
/// `i` (0 model, 1 file.read, 2 file.write, 3 process.spawn, 4 network,
/// 5 credential, 6 workspace, 7 job, 8 subagent).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IbToolV2 {
    pub struct_size: u32,
    pub id: IbByteSpanV1,
    pub version: IbByteSpanV1,
    pub description: IbByteSpanV1,
    pub input_schema: IbByteSpanV1,
    pub output_schema: IbByteSpanV1,
    pub required_capabilities: u32,
    /// Bit 0..3 = L0..L3.
    pub supported_levels: u8,
    /// 0 Deterministic, 1 Idempotent, 2 NonIdempotent.
    pub determinism: u8,
    /// 0 None, 1 Read, 2 Write, 3 Process, 4 Network.
    pub side_effect: u8,
    /// 0 Never, 1 OnSideEffect, 2 Always.
    pub confirmation: u8,
    /// 0 or 1.
    pub concurrency_safe: u8,
    pub default_timeout_ms: u32,
    pub max_output_bytes: u32,
    pub verification: IbByteSpanV1,
    pub compensation: IbByteSpanV1,
    pub user_data: *mut c_void,
    pub validate: IbToolValidateFn,
    pub execute: IbToolExecuteFn,
    pub destroy: IbDestroyFn,
}

/// One memory provider registration. `scopes_bitmask` bit `i` maps to
/// `MemoryScope` discriminant `i+1` (bit 0 conversation, 1 preferences,
/// 2 relevant, 3 project, 4 document, 5 extended).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IbMemoryProviderV2 {
    pub struct_size: u32,
    pub scopes_bitmask: u32,
    pub can_retrieve: u8,
    pub can_search: u8,
    pub can_store: u8,
    pub can_update: u8,
    pub can_delete: u8,
    pub max_results: u32,
    pub user_data: *mut c_void,
    pub retrieve: IbMemoryRetrieveFn,
    pub search: IbMemorySearchFn,
    pub store: IbMemoryWriteFn,
    pub update: IbMemoryWriteFn,
    pub delete: IbMemoryDeleteFn,
    pub destroy: IbDestroyFn,
}

/// One permission decision, filled by the C authorize callback.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbPermissionDecisionV2 {
    pub struct_size: u32,
    /// 0 Allow, 1 Ask, 2 Deny.
    pub decision: u8,
    pub rule_id: [u8; 64],
    pub rule_id_len: u32,
    pub reason: [u8; 256],
    pub reason_len: u32,
}

/// One permission provider registration.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IbPermissionProviderV2 {
    pub struct_size: u32,
    pub user_data: *mut c_void,
    pub authorize: IbPermissionAuthorizeFn,
    pub destroy: IbDestroyFn,
}

// ---------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------

/// Opaque registration-stage builder. Consumed by `ib_harness_builder_build_v2`.
#[repr(C)]
pub struct IbBuilderHandleV2 {
    magic: u64,
    builder: Option<HarnessBuilder>,
}

/// Returns ABI major version 2.
#[unsafe(no_mangle)]
pub extern "C" fn ib_harness_api_version_v2() -> u32 {
    IB_HARNESS_ABI_VERSION_V2
}

// ---------------------------------------------------------------------------
// Small helpers shared by the surface
// ---------------------------------------------------------------------------

fn read_span(span: IbByteSpanV1) -> Result<String, Failure> {
    if span.len > 16 * 1024 * 1024 || (span.data.is_null() && span.len != 0) {
        return Err(Failure::invalid(
            "ffi.v2.span",
            "span exceeds the ABI byte limit or has a null/non-empty mismatch",
        ));
    }
    let bytes = if span.len == 0 {
        &[][..]
    } else {
        // SAFETY: pointer/length validity is part of the C caller contract.
        unsafe { std::slice::from_raw_parts(span.data, span.len) }
    };
    String::from_utf8(bytes.to_vec())
        .map_err(|_| Failure::invalid("ffi.v2.span", "span is not valid UTF-8"))
}

fn span_of(value: &str) -> IbByteSpanV1 {
    IbByteSpanV1 {
        struct_size: u32::try_from(std::mem::size_of::<IbByteSpanV1>()).unwrap_or(u32::MAX),
        data: value.as_ptr(),
        len: value.len(),
    }
}

fn empty_span() -> IbByteSpanV1 {
    IbByteSpanV1 {
        struct_size: u32::try_from(std::mem::size_of::<IbByteSpanV1>()).unwrap_or(u32::MAX),
        data: ptr::null(),
        len: 0,
    }
}

fn size_of<T>() -> u32 {
    u32::try_from(std::mem::size_of::<T>()).unwrap_or(u32::MAX)
}

fn validate_dto_size(actual: u32, required: u32) -> Result<(), Failure> {
    if actual < required {
        return Err(Failure::invalid(
            "ffi.v2.size_tag",
            "caller struct is smaller than the ABI requires",
        ));
    }
    Ok(())
}

fn capability_from_discriminant(discriminant: u8) -> Option<Capability> {
    Some(match discriminant {
        0 => Capability::Model,
        1 => Capability::FileRead,
        2 => Capability::FileWrite,
        3 => Capability::ProcessSpawn,
        4 => Capability::Network,
        5 => Capability::Credential,
        6 => Capability::Workspace,
        7 => Capability::Job,
        8 => Capability::Subagent,
        _ => return None,
    })
}

fn scope_from_u8(code: u8) -> HarnessResult<MemoryScope> {
    match code {
        0 => Ok(MemoryScope::None),
        1 => Ok(MemoryScope::Conversation),
        2 => Ok(MemoryScope::Preferences),
        3 => Ok(MemoryScope::Relevant),
        4 => Ok(MemoryScope::Project),
        5 => Ok(MemoryScope::Document),
        6 => Ok(MemoryScope::Extended),
        _ => Err(Failure::invalid(
            "ffi.v2.memory_scope",
            "unknown scope code",
        )),
    }
}

fn scope_to_u8(scope: MemoryScope) -> u8 {
    match scope {
        MemoryScope::None => 0,
        MemoryScope::Conversation => 1,
        MemoryScope::Preferences => 2,
        MemoryScope::Relevant => 3,
        MemoryScope::Project => 4,
        MemoryScope::Document => 5,
        MemoryScope::Extended => 6,
    }
}

/// Status codes (the v1 values) ↔ core failures, for callback results.
fn failure_from_status(operation: &str, status: i32) -> Failure {
    match status {
        IB_STATUS_INVALID_ARGUMENT => Failure::invalid(operation, "callback reported invalid data"),
        IB_STATUS_DENIED => Failure::new(
            ErrorCode::PermissionDenied,
            FailureClass::Policy,
            operation,
            "callback denied the operation",
        ),
        IB_STATUS_CANCELLED => Failure::cancelled(operation, "callback reported cancellation"),
        IB_STATUS_RESOURCE_EXHAUSTED => Failure::new(
            ErrorCode::BudgetExceeded,
            FailureClass::Resource,
            operation,
            "callback exhausted a budget",
        ),
        IB_STATUS_PANIC => Failure::new(
            ErrorCode::Internal,
            FailureClass::Internal,
            operation,
            "callback reported a contained panic",
        ),
        _ => Failure::new(
            ErrorCode::ProviderFailed,
            FailureClass::Provider,
            operation,
            "callback failed",
        ),
    }
}

fn status_from_failure(failure: &Failure) -> i32 {
    match failure.code {
        ErrorCode::InvalidInput => IB_STATUS_INVALID_ARGUMENT,
        ErrorCode::RouteDenied
        | ErrorCode::PermissionDenied
        | ErrorCode::ConfirmationRequired
        | ErrorCode::FilesystemDenied
        | ErrorCode::SubprocessDenied => IB_STATUS_DENIED,
        ErrorCode::CapabilityUnavailable | ErrorCode::SandboxUnavailable | ErrorCode::NotFound => {
            IB_STATUS_UNAVAILABLE
        }
        ErrorCode::Cancelled => IB_STATUS_CANCELLED,
        ErrorCode::BudgetExceeded | ErrorCode::Timeout | ErrorCode::RecoveryExhausted => {
            IB_STATUS_RESOURCE_EXHAUSTED
        }
        _ => IB_STATUS_OPERATION_FAILED,
    }
}

// ---------------------------------------------------------------------------
// C adapters (Rust objects wrapping the C vtables)
// ---------------------------------------------------------------------------

/// Shared release semantics: one `destroy(user_data)` per registration, fired
/// by Drop — on rejection, on an abandoned builder, or when the last harness
/// referencing the adapter is destroyed.
///
/// The user-data pointer is Send+Sync by the C caller contract: the host
/// guarantees its callbacks are callable (and destroy is callable exactly
/// once) from any thread the harness uses, exactly like every other
/// `extern "C"` callback the ABI hands out.
#[repr(transparent)]
struct CUserData(*mut c_void);
// SAFETY: see the struct doc — thread-safety is a caller guarantee of the ABI.
unsafe impl Send for CUserData {}
// SAFETY: as above.
unsafe impl Sync for CUserData {}

struct CRegistration {
    user_data: CUserData,
    destroy: IbDestroyFn,
}

impl Drop for CRegistration {
    fn drop(&mut self) {
        if let Some(destroy) = self.destroy {
            // The C host guarantees the callback stays callable for the
            // registration's whole lifetime and fires exactly once (here).
            destroy(self.user_data.0);
        }
    }
}

/// Model adapter over one C vtable.
struct CModelProvider {
    registration: CRegistration,
    id: String,
    models: Vec<String>,
    stream: IbModelStreamFn,
}

/// Per-dispatch emit state shared with the emit trampoline.
struct EmitState<'a> {
    sink: &'a mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    text: String,
    failure: Option<Failure>,
}

/// The trampoline C calls for each chunk. Contained: a panic inside the sink
/// becomes a nonzero status and a stored failure instead of an unwind across C.
extern "C" fn chunk_emit_trampoline(emit_data: *mut c_void, chunk: *const IbModelChunkV2) -> i32 {
    // SAFETY: emit_data is the &mut EmitState this stream call created; it
    // outlives every emission of this dispatch. chunk is checked non-null
    // and stays valid for the duration of the call.
    let state = unsafe { &mut *(emit_data.cast::<EmitState<'_>>()) };
    let result = catch_unwind(AssertUnwindSafe(|| -> HarnessResult<()> {
        let chunk = if chunk.is_null() {
            return Err(Failure::invalid("ffi.v2.model_chunk", "null chunk"));
        } else {
            // SAFETY: non-null checked above.
            unsafe { &*chunk }
        };
        validate_dto_size(chunk.struct_size, size_of::<IbModelChunkV2>())?;
        let converted = convert_chunk(chunk)?;
        if let ModelChunk::TextDelta { text, .. } = &converted {
            state.text.push_str(text);
        }
        (state.sink)(converted)
    }));
    match result {
        Ok(Ok(())) => IB_STATUS_OK,
        Ok(Err(failure)) => {
            state.failure = Some(failure);
            IB_STATUS_OPERATION_FAILED
        }
        Err(_panic) => {
            state.failure = Some(Failure::new(
                ErrorCode::Internal,
                FailureClass::Internal,
                "ffi.v2.model_emit",
                "emit trampoline contained a panic",
            ));
            IB_STATUS_PANIC
        }
    }
}

fn convert_chunk(chunk: &IbModelChunkV2) -> HarnessResult<ModelChunk> {
    Ok(match chunk.kind {
        0 => ModelChunk::Start { block: chunk.block },
        1 => ModelChunk::TextDelta {
            block: chunk.block,
            text: read_span(chunk.text)?,
        },
        2 => ModelChunk::ReasoningDelta {
            block: chunk.block,
            text: read_span(chunk.text)?,
        },
        3 => ModelChunk::ToolCall {
            block: chunk.block,
            call_id: read_span(chunk.call_id)?,
            tool_id: read_span(chunk.tool_id)?,
            arguments: read_span(chunk.arguments)?,
        },
        4 => ModelChunk::End { block: chunk.block },
        5 => ModelChunk::Usage {
            input_units: chunk.input_units,
            output_units: chunk.output_units,
        },
        6 => ModelChunk::Finish {
            reason: finish_reason_from_u8(chunk.finish_reason)?,
        },
        _ => return Err(Failure::invalid("ffi.v2.model_chunk", "unknown chunk kind")),
    })
}

fn finish_reason_from_u8(code: u8) -> HarnessResult<FinishReason> {
    match code {
        0 => Ok(FinishReason::Stop),
        1 => Ok(FinishReason::ToolCalls),
        2 => Ok(FinishReason::Length),
        3 => Ok(FinishReason::Cancelled),
        4 => Ok(FinishReason::Error),
        _ => Err(Failure::invalid(
            "ffi.v2.model_response",
            "unknown finish reason",
        )),
    }
}

fn role_to_str(role: ModelRole) -> &'static str {
    match role {
        ModelRole::System => "system",
        ModelRole::User => "user",
        ModelRole::Assistant => "assistant",
        ModelRole::Tool => "tool",
    }
}

impl ModelProvider for CModelProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn models(&self) -> Vec<String> {
        self.models.clone()
    }

    fn stream(
        &self,
        request: &ModelRequest,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        // Bind the marshalled JSON before taking spans of it: spans borrow.
        let messages_json = Value::Array(
            request
                .messages
                .iter()
                .map(|message| {
                    Value::Object(BTreeMap::from([
                        (
                            "role".to_owned(),
                            Value::String(role_to_str(message.role).to_owned()),
                        ),
                        ("content".to_owned(), Value::String(message.content.clone())),
                    ]))
                })
                .collect(),
        )
        .to_canonical_json();
        let tools_json = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    Value::Object(BTreeMap::from([
                        ("id".to_owned(), Value::String(tool.id.clone())),
                        (
                            "description".to_owned(),
                            Value::String(tool.description.clone()),
                        ),
                        (
                            "input_schema".to_owned(),
                            Value::String(tool.input_schema.clone()),
                        ),
                    ]))
                })
                .collect(),
        )
        .to_canonical_json();
        let request_dto = IbModelRequestV2 {
            struct_size: size_of::<IbModelRequestV2>(),
            request_id: span_of(&request.request_id),
            provider: span_of(&request.provider),
            model: span_of(&request.model),
            system: span_of(&request.system),
            messages_json: span_of(&messages_json),
            tools_json: span_of(&tools_json),
            max_output_bytes: request.max_output_bytes,
        };
        // The cancellation handle is stack-owned for this dispatch: the same
        // token the harness handed us (a cheap clone), so C polls the live token.
        let mut cancellation = IbCancellationHandle {
            magic: CANCEL_HANDLE_MAGIC,
            token: cancel.clone(),
        };
        let mut emit_state = EmitState {
            sink,
            text: String::new(),
            failure: None,
        };
        let mut out_response = IbModelResponseV2 {
            struct_size: size_of::<IbModelResponseV2>(),
            finish_reason: 0,
            input_units: 0,
            output_units: 0,
        };
        // The C host guarantees stream stays callable for the provider's
        // registered lifetime; every pointer above is valid for this call and
        // the emit data outlives all emissions of the call.
        let status = (self.stream)(
            self.registration.user_data.0,
            &request_dto,
            ptr::from_mut(&mut cancellation),
            chunk_emit_trampoline,
            (&mut emit_state as *mut EmitState<'_>).cast(),
            &mut out_response,
        );
        if let Some(failure) = emit_state.failure.take() {
            return Err(failure);
        }
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.model_stream", status));
        }
        validate_dto_size(out_response.struct_size, size_of::<IbModelResponseV2>())?;
        // The harness consumes response.text as the final output (chunks are
        // observability), so the assembled text is authoritative here.
        Ok(ModelResponse {
            text: emit_state.text,
            finish: finish_reason_from_u8(out_response.finish_reason)?,
            input_units: out_response.input_units,
            output_units: out_response.output_units,
            provider_request_id: None,
        })
    }
}

/// Tool adapter over one C vtable.
struct CTool {
    registration: CRegistration,
    manifest: ToolManifest,
    validate: IbToolValidateFn,
    execute: IbToolExecuteFn,
}

fn arguments_to_json(arguments: &ToolArguments) -> String {
    Value::Object(arguments.clone().into_iter().collect()).to_canonical_json()
}

impl Tool for CTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        let arguments_json = arguments_to_json(arguments);
        // The C host guarantees the callbacks for the tool's registered
        // lifetime; the span is borrowed for this call only. (Calling an
        // `extern "C"` fn pointer is not itself unsafe.)
        let status = (self.validate)(self.registration.user_data.0, span_of(&arguments_json));
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.tool_validate", status));
        }
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        let arguments_json = arguments_to_json(arguments);
        let mut cancellation = IbCancellationHandle {
            magic: CANCEL_HANDLE_MAGIC,
            token: context.cancel.clone(),
        };
        let context_dto = IbToolContextV2 {
            struct_size: size_of::<IbToolContextV2>(),
            actor: span_of(context.actor),
            level: context.level as u8,
            cancel: ptr::from_mut(&mut cancellation),
        };
        let mut out = IbToolOutputV2 {
            struct_size: size_of::<IbToolOutputV2>(),
            value_json: empty_span(),
            model_content: empty_span(),
        };
        // Callbacks are guaranteed for the tool's registered lifetime; the out
        // spans are written by C and copied before this call returns.
        let status = (self.execute)(
            self.registration.user_data.0,
            span_of(&arguments_json),
            &context_dto,
            &mut out,
        );
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.tool_execute", status));
        }
        validate_dto_size(out.struct_size, size_of::<IbToolOutputV2>())?;
        let value = if out.value_json.len == 0 {
            Value::Null
        } else {
            let text = read_span(out.value_json)?;
            Value::parse_json(&text).map_err(|message| {
                Failure::new(
                    ErrorCode::ToolFailed,
                    FailureClass::Execution,
                    "ffi.v2.tool_output",
                    "tool output value is not valid JSON",
                )
                .with_detail("parse", &message)
            })?
        };
        let model_content = read_span(out.model_content)?;
        Ok(ToolOutput {
            value,
            model_content,
            presentation: BTreeMap::new(),
        })
    }
}

/// Memory adapter over one C vtable.
struct CMemoryProvider {
    registration: CRegistration,
    capabilities: MemoryCapabilities,
    retrieve: IbMemoryRetrieveFn,
    search: IbMemorySearchFn,
    store: IbMemoryWriteFn,
    update: IbMemoryWriteFn,
    delete: IbMemoryDeleteFn,
}

fn record_to_json(record: &MemoryRecord) -> Value {
    Value::Object(BTreeMap::from([
        ("id".to_owned(), Value::String(record.id.clone())),
        (
            "scope".to_owned(),
            Value::String(record.scope.as_str().to_owned()),
        ),
        (
            "namespace".to_owned(),
            Value::String(record.namespace.clone()),
        ),
        ("content".to_owned(), Value::String(record.content.clone())),
        (
            "attributes".to_owned(),
            Value::Object(
                record
                    .attributes
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ),
        ),
    ]))
}

fn record_from_json(value: &Value) -> HarnessResult<MemoryRecord> {
    let object = value
        .as_object()
        .ok_or_else(|| Failure::invalid("ffi.v2.memory_record", "record JSON is not an object"))?;
    let string = |key: &str| -> HarnessResult<String> {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                Failure::invalid(
                    "ffi.v2.memory_record",
                    "record field is missing or not a string",
                )
            })
    };
    let scope_name = string("scope")?;
    let scope = [
        MemoryScope::Conversation,
        MemoryScope::Preferences,
        MemoryScope::Relevant,
        MemoryScope::Project,
        MemoryScope::Document,
        MemoryScope::Extended,
    ]
    .into_iter()
    .find(|scope| scope.as_str() == scope_name)
    .ok_or_else(|| Failure::invalid("ffi.v2.memory_record", "record scope is unknown"))?;
    let mut attributes = BTreeMap::new();
    if let Some(entries) = object.get("attributes").and_then(Value::as_object) {
        for (key, entry) in entries {
            attributes.insert(
                key.clone(),
                entry
                    .as_str()
                    .ok_or_else(|| {
                        Failure::invalid("ffi.v2.memory_record", "attribute value is not a string")
                    })?
                    .to_owned(),
            );
        }
    }
    let record = MemoryRecord {
        id: string("id")?,
        scope,
        namespace: string("namespace")?,
        content: string("content")?,
        attributes,
    };
    record.validate()?;
    Ok(record)
}

impl MemoryProvider for CMemoryProvider {
    fn capabilities(&self) -> MemoryCapabilities {
        self.capabilities.clone()
    }

    fn retrieve(
        &self,
        scope: MemoryScope,
        namespace: &str,
        id: &str,
    ) -> HarnessResult<Option<MemoryRecord>> {
        let mut out = empty_span();
        // Callbacks are guaranteed for the provider's registered lifetime;
        // the out span is written by C and copied before returning.
        let status = (self.retrieve)(
            self.registration.user_data.0,
            scope_to_u8(scope),
            span_of(namespace),
            span_of(id),
            &mut out,
        );
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.memory_retrieve", status));
        }
        let text = read_span(out)?;
        if text.is_empty() {
            return Ok(None);
        }
        let value = Value::parse_json(&text).map_err(|message| {
            Failure::invalid("ffi.v2.memory_retrieve", "record JSON is invalid")
                .with_detail("parse", &message)
        })?;
        Ok(Some(record_from_json(&value)?))
    }

    fn search(&self, query: &MemoryQuery) -> HarnessResult<Vec<MemoryRecord>> {
        query.validate()?;
        let namespace_span = query
            .namespace
            .as_deref()
            .map(span_of)
            .unwrap_or_else(empty_span);
        let mut out = empty_span();
        // As retrieve; an absent namespace is a null span by contract.
        let status = (self.search)(
            self.registration.user_data.0,
            scope_to_u8(query.scope),
            namespace_span,
            span_of(&query.text),
            query.limit.min(u32::MAX as usize) as u32,
            &mut out,
        );
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.memory_search", status));
        }
        let text = read_span(out)?;
        let value = Value::parse_json(&text).map_err(|message| {
            Failure::invalid("ffi.v2.memory_search", "results JSON is invalid")
                .with_detail("parse", &message)
        })?;
        let array = match value {
            Value::Array(entries) => entries,
            _ => {
                return Err(Failure::invalid(
                    "ffi.v2.memory_search",
                    "results JSON is not an array",
                ));
            }
        };
        array.iter().map(record_from_json).collect()
    }

    fn store(&self, record: MemoryRecord) -> HarnessResult<()> {
        record.validate()?;
        let json = record_to_json(&record).to_canonical_json();
        // Callbacks are guaranteed for the provider's registered lifetime.
        let status = (self.store)(self.registration.user_data.0, span_of(&json));
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.memory_store", status));
        }
        Ok(())
    }

    fn update(&self, record: MemoryRecord) -> HarnessResult<()> {
        record.validate()?;
        let json = record_to_json(&record).to_canonical_json();
        // As store.
        let status = (self.update)(self.registration.user_data.0, span_of(&json));
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.memory_update", status));
        }
        Ok(())
    }

    fn delete(&self, scope: MemoryScope, namespace: &str, id: &str) -> HarnessResult<bool> {
        let mut found = 0_u8;
        // As retrieve; out_found is a plain byte written by C.
        let status = (self.delete)(
            self.registration.user_data.0,
            scope_to_u8(scope),
            span_of(namespace),
            span_of(id),
            &mut found,
        );
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.memory_delete", status));
        }
        Ok(found != 0)
    }
}

/// Permission adapter over one C vtable.
struct CPermissionProvider {
    registration: CRegistration,
    authorize: IbPermissionAuthorizeFn,
}

impl PermissionProvider for CPermissionProvider {
    fn authorize(
        &self,
        actor: &str,
        capability: Capability,
        resource: &str,
    ) -> HarnessResult<PermissionDecision> {
        let mut out = IbPermissionDecisionV2 {
            struct_size: size_of::<IbPermissionDecisionV2>(),
            decision: IB_DECISION_ASK,
            rule_id: [0; 64],
            rule_id_len: 0,
            reason: [0; 256],
            reason_len: 0,
        };
        // Callbacks are guaranteed for the provider's registered lifetime;
        // the decision struct is plain memory written by C.
        let status = (self.authorize)(
            self.registration.user_data.0,
            span_of(actor),
            capability as u8,
            span_of(resource),
            &mut out,
        );
        if status != IB_STATUS_OK {
            return Err(failure_from_status("ffi.v2.permission", status));
        }
        validate_dto_size(out.struct_size, size_of::<IbPermissionDecisionV2>())?;
        match out.decision {
            IB_DECISION_ALLOW => Ok(PermissionDecision::Allow),
            IB_DECISION_ASK => Ok(PermissionDecision::Ask),
            IB_DECISION_DENY => {
                let bounded = |buffer: &[u8], length: u32| -> HarnessResult<String> {
                    let length = usize::try_from(length).unwrap_or(usize::MAX);
                    if length > buffer.len() {
                        return Err(Failure::invalid(
                            "ffi.v2.permission",
                            "denial rule or reason length exceeds the fixed buffer",
                        ));
                    }
                    String::from_utf8(buffer[..length].to_vec()).map_err(|_| {
                        Failure::invalid("ffi.v2.permission", "denial text is not UTF-8")
                    })
                };
                let rule_id = bounded(&out.rule_id, out.rule_id_len)?;
                let reason = bounded(&out.reason, out.reason_len)?;
                Ok(PermissionDecision::Deny { rule_id, reason })
            }
            _ => Err(Failure::invalid(
                "ffi.v2.permission",
                "unknown permission decision",
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Builder lifecycle + registration exports
// ---------------------------------------------------------------------------

fn builder_ref<'a>(handle: *mut IbBuilderHandleV2) -> Result<&'a mut IbBuilderHandleV2, i32> {
    if handle.is_null() {
        return Err(IB_STATUS_INVALID_ARGUMENT);
    }
    // SAFETY: non-null was checked. The leading tag check rejects stale,
    // swapped, or type-confused pointers before the builder body is touched.
    let reference = unsafe { &mut *handle };
    if reference.magic != BUILDER_HANDLE_MAGIC {
        return Err(IB_STATUS_INVALID_ARGUMENT);
    }
    Ok(reference)
}

/// Creates one builder over the embedding-safe constructor (NO built-in
/// tools). The handle must be consumed by `ib_harness_builder_build_v2` or
/// destroyed with `ib_harness_builder_destroy_v2`.
///
/// # Safety
/// `config` and `out_builder` must be valid, aligned pointers for this call.
/// Spans in `config` must remain readable for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_builder_create_v2(
    config: *const IbHarnessConfigV2,
    out_builder: *mut *mut IbBuilderHandleV2,
) -> i32 {
    v2_boundary(|| {
        if config.is_null() || out_builder.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: non-null checked; the caller contract requires a valid aligned struct.
        let config = unsafe { &*config };
        validate_dto_size(config.struct_size, size_of::<IbHarnessConfigV2>())
            .map_err(|failure| status_from_failure(&failure))?;
        if config.abi_version != IB_HARNESS_ABI_VERSION_V2 {
            return Err(IB_STATUS_UNAVAILABLE);
        }
        let root = read_span(config.root).map_err(|failure| status_from_failure(&failure))?;
        let maximum_level = level_from_u8(config.maximum_level)?;
        let builder = HarnessBuilder::local_embedded(&root)
            .map_err(|failure| status_from_failure(&failure))?
            .route_policy(RoutePolicy {
                maximum_level,
                allow_explicit_escalation: config.allow_explicit_escalation != 0,
            });
        let builder = if config.system_prefix.len > 0 {
            let prefix =
                read_span(config.system_prefix).map_err(|failure| status_from_failure(&failure))?;
            builder.system_prefix(prefix)
        } else {
            builder
        };
        let handle = Box::new(IbBuilderHandleV2 {
            magic: BUILDER_HANDLE_MAGIC,
            builder: Some(builder),
        });
        // SAFETY: out_builder is non-null writable storage by the caller contract.
        unsafe { *out_builder = Box::into_raw(handle) };
        Ok(())
    })
}

/// Destroys one abandoned builder. Registrations are released (their destroy
/// callbacks fire). A null handle is a no-op.
///
/// # Safety
/// A non-null handle must be live and not concurrently used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_builder_destroy_v2(builder: *mut IbBuilderHandleV2) -> i32 {
    v2_boundary(|| {
        if builder.is_null() {
            return Ok(());
        }
        // SAFETY: the tag check runs BEFORE the free: a stale, already-destroyed
        // or type-confused pointer must never reach Box::from_raw (double free).
        let reference = unsafe { &mut *builder };
        if reference.magic != BUILDER_HANDLE_MAGIC {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: created by create_v2; freed exactly once. The tag is
        // zeroed so a stale or double-destroy fails the tag check later.
        unsafe {
            let mut boxed = Box::from_raw(builder);
            boxed.magic = 0;
            drop(boxed);
        }
        Ok(())
    })
}

/// Builds the harness and hands over a v1-compatible harness handle. The
/// builder handle is consumed (destroyed) on both success and failure.
///
/// # Safety
/// `builder` must be live; `out_handle` must be writable. The returned harness
/// handle must be destroyed with `ib_harness_destroy_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_builder_build_v2(
    builder: *mut IbBuilderHandleV2,
    out_handle: *mut *mut IbHarnessHandle,
) -> i32 {
    v2_boundary(|| {
        let reference = builder_ref(builder)?;
        if out_handle.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        let core_builder = reference.builder.take().ok_or(IB_STATUS_INVALID_ARGUMENT)?;
        // Consume the builder handle regardless of the build result so it can
        // never be used again.
        // SAFETY: created by create_v2; freed exactly once by this call.
        unsafe {
            let mut boxed = Box::from_raw(builder);
            boxed.magic = 0;
            drop(boxed);
        }
        let harness = core_builder.build();
        let handle = Box::new(IbHarnessHandle {
            magic: HARNESS_HANDLE_MAGIC,
            harness,
        });
        // SAFETY: out_handle is non-null writable storage by the caller contract.
        unsafe { *out_handle = Box::into_raw(handle) };
        Ok(())
    })
}

/// Registers one C model provider. The provider id and model catalogue are
/// copied; the vtable is held until the registration is released. A rejected
/// registration consumes the builder (see the module contract) and fires the
/// destroy callback immediately.
///
/// # Safety
/// `provider` must be a valid, size-tagged struct whose spans stay readable
/// for the call and whose callbacks stay callable until its destroy fires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_builder_register_model_v2(
    builder: *mut IbBuilderHandleV2,
    provider: *const IbModelProviderV2,
) -> i32 {
    v2_boundary(|| {
        let reference = builder_ref(builder)?;
        if provider.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: non-null checked; the caller contract requires a valid aligned struct.
        let provider = unsafe { &*provider };
        validate_dto_size(provider.struct_size, size_of::<IbModelProviderV2>())
            .map_err(|failure| status_from_failure(&failure))?;
        let id = read_span(provider.id).map_err(|failure| status_from_failure(&failure))?;
        let models_json =
            read_span(provider.models_json).map_err(|failure| status_from_failure(&failure))?;
        let catalogue = Value::parse_json(&models_json).map_err(|_| IB_STATUS_INVALID_ARGUMENT)?;
        let models = match catalogue {
            Value::Array(entries) => {
                let mut models = Vec::with_capacity(entries.len());
                for entry in entries {
                    models.push(entry.as_str().ok_or(IB_STATUS_INVALID_ARGUMENT)?.to_owned());
                }
                models
            }
            _ => return Err(IB_STATUS_INVALID_ARGUMENT),
        };
        let current = reference.builder.take().ok_or(IB_STATUS_INVALID_ARGUMENT)?;
        // The adapter is created AFTER the builder take: on the error path
        // below it drops here, firing the destroy callback for the rejection.
        let adapter = CModelProvider {
            registration: CRegistration {
                user_data: CUserData(provider.user_data),
                destroy: provider.destroy,
            },
            id,
            models,
            stream: provider.stream,
        };
        match current.register_model(Arc::new(adapter)) {
            Ok(updated) => {
                reference.builder = Some(updated);
                Ok(())
            }
            Err(failure) => Err(status_from_failure(&failure)),
        }
    })
}

/// Registers one C tool. A rejected registration consumes the builder (see
/// the module contract) and fires the destroy callback immediately.
///
/// # Safety
/// `tool` must be a valid, size-tagged struct whose spans stay readable for
/// the call and whose callbacks stay callable until its destroy fires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_builder_register_tool_v2(
    builder: *mut IbBuilderHandleV2,
    tool: *const IbToolV2,
) -> i32 {
    v2_boundary(|| {
        let reference = builder_ref(builder)?;
        if tool.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: non-null checked; the caller contract requires a valid aligned struct.
        let tool = unsafe { &*tool };
        validate_dto_size(tool.struct_size, size_of::<IbToolV2>())
            .map_err(|failure| status_from_failure(&failure))?;
        let mut capabilities = inbharat_harness_core::CapabilitySet::new();
        for discriminant in 0_u8..=8 {
            if tool.required_capabilities & 1u32 << discriminant != 0 {
                capabilities.insert(
                    capability_from_discriminant(discriminant).ok_or(IB_STATUS_INVALID_ARGUMENT)?,
                );
            }
        }
        let mut supported_levels = Vec::new();
        for (bit, level) in [
            (0_u8, ExecutionLevel::L0),
            (1, ExecutionLevel::L1),
            (2, ExecutionLevel::L2),
            (3, ExecutionLevel::L3),
        ] {
            if tool.supported_levels & (1 << bit) != 0 {
                supported_levels.push(level);
            }
        }
        let manifest = ToolManifest {
            id: read_span(tool.id).map_err(|failure| status_from_failure(&failure))?,
            version: read_span(tool.version).map_err(|failure| status_from_failure(&failure))?,
            description: read_span(tool.description)
                .map_err(|failure| status_from_failure(&failure))?,
            input_schema: read_span(tool.input_schema)
                .map_err(|failure| status_from_failure(&failure))?,
            output_schema: read_span(tool.output_schema)
                .map_err(|failure| status_from_failure(&failure))?,
            required_capabilities: capabilities,
            supported_levels,
            determinism: match tool.determinism {
                0 => Determinism::Deterministic,
                1 => Determinism::Idempotent,
                2 => Determinism::NonIdempotent,
                _ => return Err(IB_STATUS_INVALID_ARGUMENT),
            },
            side_effect: match tool.side_effect {
                0 => SideEffect::None,
                1 => SideEffect::Read,
                2 => SideEffect::Write,
                3 => SideEffect::Process,
                4 => SideEffect::Network,
                _ => return Err(IB_STATUS_INVALID_ARGUMENT),
            },
            confirmation: match tool.confirmation {
                0 => ConfirmationMode::Never,
                1 => ConfirmationMode::OnSideEffect,
                2 => ConfirmationMode::Always,
                _ => return Err(IB_STATUS_INVALID_ARGUMENT),
            },
            concurrency_safe: tool.concurrency_safe != 0,
            default_timeout: Duration::from_millis(u64::from(tool.default_timeout_ms)),
            max_output_bytes: tool.max_output_bytes as usize,
            verification: read_span(tool.verification)
                .map_err(|failure| status_from_failure(&failure))?,
            compensation: read_span(tool.compensation)
                .map_err(|failure| status_from_failure(&failure))?,
        };
        let current = reference.builder.take().ok_or(IB_STATUS_INVALID_ARGUMENT)?;
        let adapter = CTool {
            registration: CRegistration {
                user_data: CUserData(tool.user_data),
                destroy: tool.destroy,
            },
            manifest,
            validate: tool.validate,
            execute: tool.execute,
        };
        match current.register_tool(Arc::new(adapter)) {
            Ok(updated) => {
                reference.builder = Some(updated);
                Ok(())
            }
            Err(failure) => Err(status_from_failure(&failure)),
        }
    })
}

/// Registers one C memory provider (replaces any earlier one — the core keeps
/// at most one memory provider). This call cannot fail; the only error paths
/// are argument validation.
///
/// # Safety
/// `memory` must be a valid, size-tagged struct whose callbacks stay callable
/// until its destroy fires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_builder_register_memory_v2(
    builder: *mut IbBuilderHandleV2,
    memory: *const IbMemoryProviderV2,
) -> i32 {
    v2_boundary(|| {
        let reference = builder_ref(builder)?;
        if memory.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: non-null checked; the caller contract requires a valid aligned struct.
        let memory = unsafe { &*memory };
        validate_dto_size(memory.struct_size, size_of::<IbMemoryProviderV2>())
            .map_err(|failure| status_from_failure(&failure))?;
        let mut scopes = Vec::new();
        for bit in 0_u32..6 {
            if memory.scopes_bitmask & (1 << bit) != 0 {
                scopes.push(
                    scope_from_u8((bit + 1) as u8)
                        .map_err(|failure| status_from_failure(&failure))?,
                );
            }
        }
        let capabilities = MemoryCapabilities {
            scopes,
            can_retrieve: memory.can_retrieve != 0,
            can_search: memory.can_search != 0,
            can_store: memory.can_store != 0,
            can_update: memory.can_update != 0,
            can_delete: memory.can_delete != 0,
            max_results: memory.max_results as usize,
        };
        let current = reference.builder.take().ok_or(IB_STATUS_INVALID_ARGUMENT)?;
        // memory_provider never fails; a replacement drops the previous
        // registration (its destroy fires).
        let adapter = CMemoryProvider {
            registration: CRegistration {
                user_data: CUserData(memory.user_data),
                destroy: memory.destroy,
            },
            capabilities,
            retrieve: memory.retrieve,
            search: memory.search,
            store: memory.store,
            update: memory.update,
            delete: memory.delete,
        };
        reference.builder = Some(current.memory_provider(Arc::new(adapter)));
        Ok(())
    })
}

/// Registers one C permission provider. Without one, a v2 build denies every
/// tool call through the core's deny-by-default policy — the honest failure,
/// never a silent allow. This call cannot fail; the only error paths are
/// argument validation.
///
/// # Safety
/// `permission` must be a valid, size-tagged struct whose callbacks stay
/// callable until its destroy fires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_builder_register_permission_v2(
    builder: *mut IbBuilderHandleV2,
    permission: *const IbPermissionProviderV2,
) -> i32 {
    v2_boundary(|| {
        let reference = builder_ref(builder)?;
        if permission.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: non-null checked; the caller contract requires a valid aligned struct.
        let permission = unsafe { &*permission };
        validate_dto_size(permission.struct_size, size_of::<IbPermissionProviderV2>())
            .map_err(|failure| status_from_failure(&failure))?;
        let current = reference.builder.take().ok_or(IB_STATUS_INVALID_ARGUMENT)?;
        let adapter = CPermissionProvider {
            registration: CRegistration {
                user_data: CUserData(permission.user_data),
                destroy: permission.destroy,
            },
            authorize: permission.authorize,
        };
        reference.builder = Some(current.permission_provider(Arc::new(adapter)));
        Ok(())
    })
}

/// Polls one cancellation handle from inside a C callback: returns 1 when
/// cancellation was requested, 0 when not, `IB_STATUS_INVALID_ARGUMENT` for a
/// null, stale, or type-confused handle.
///
/// # Safety
/// `handle` must be null or a live cancellation handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_cancel_requested_v2(handle: *mut IbCancellationHandle) -> i32 {
    match v2_boundary_value(|| {
        if handle.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // SAFETY: non-null checked; the tag check rejects stale pointers.
        let reference = unsafe { &*handle };
        if reference.magic != CANCEL_HANDLE_MAGIC {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        Ok(if reference.token.is_cancelled() { 1 } else { 0 })
    }) {
        Ok(value) => value,
        Err(status) => status,
    }
}

/// Runs one bounded task with the caller-selected provider, model and
/// capability grants, optionally returning the run's tamper-evident audit
/// ledger (hash-chained session events, one canonical JSON line per event).
///
/// `cancellation` may be null (an internal never-cancelled token is used).
/// `out_bytes` receives the run output; `out_audit` may be null. Both outputs
/// (when non-null) are library-owned and released with
/// `ib_harness_bytes_free_v1`.
///
/// # Safety
/// Handles and spans must stay valid for the call; output structs must be
/// writable and released exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_run_v2(
    handle: *mut IbHarnessHandle,
    prompt: IbByteSpanV1,
    explicit_level: i8,
    provider: IbByteSpanV1,
    model: IbByteSpanV1,
    capabilities: u32,
    cancellation: *mut IbCancellationHandle,
    out_bytes: *mut IbOwnedBytesV1,
    out_audit: *mut IbOwnedBytesV1,
) -> i32 {
    v2_boundary(|| {
        let handle = v1_handle_ref(handle)?;
        if out_bytes.is_null() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        // Validate every argument BEFORE writing to caller memory (the v1
        // rule: never clobber a caller's live allocation pointer on a failing call).
        let prompt = read_utf8_v1(prompt)?;
        let provider = read_span(provider).map_err(|failure| status_from_failure(&failure))?;
        let model = read_span(model).map_err(|failure| status_from_failure(&failure))?;
        if provider.is_empty() || model.is_empty() {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        let explicit = explicit_level_option_v1(explicit_level)?;
        let mut granted = inbharat_harness_core::CapabilitySet::new();
        for discriminant in 0_u8..=8 {
            if capabilities & 1u32 << discriminant != 0 {
                granted.insert(
                    capability_from_discriminant(discriminant).ok_or(IB_STATUS_INVALID_ARGUMENT)?,
                );
            }
        }
        if !granted.contains(Capability::Model) {
            return Err(IB_STATUS_INVALID_ARGUMENT);
        }
        let options = RunOptions {
            explicit_level: explicit,
            provider,
            model,
            capabilities: granted,
            ..RunOptions::default()
        };
        let (outcome, session) = if cancellation.is_null() {
            handle
                .harness
                .run(&prompt, &options, &CancellationToken::new())
                .map_err(|failure| status_from_failure(&failure))?
        } else {
            // SAFETY: the caller contract supplies a live cancellation handle.
            let token = unsafe { &*cancellation };
            if token.magic != CANCEL_HANDLE_MAGIC {
                return Err(IB_STATUS_INVALID_ARGUMENT);
            }
            handle
                .harness
                .run(&prompt, &options, &token.token)
                .map_err(|failure| status_from_failure(&failure))?
        };
        // SAFETY: out_bytes is non-null writable storage by the caller contract.
        unsafe { *out_bytes = IbOwnedBytesV1::default() };
        write_owned_v1(out_bytes, outcome.output)?;
        if !out_audit.is_null() {
            // SAFETY: out_audit is non-null writable storage by the caller contract.
            unsafe { *out_audit = IbOwnedBytesV1::default() };
            let ledger = audit_ledger(&session);
            write_owned_v1(out_audit, ledger)?;
        }
        Ok(())
    })
}

/// The audit ledger of one run: every session event as a canonical JSON line
/// (format, session id, sequence number, hash chain, replay-required flag and
/// the event body), newline-separated. The caller can re-verify the chain.
fn audit_ledger(session: &Session) -> String {
    let mut ledger = String::new();
    for event in session.events() {
        ledger.push_str(&event.to_json_line());
        ledger.push('\n');
    }
    ledger
}

// ---------------------------------------------------------------------------
// Boundary plumbing
// ---------------------------------------------------------------------------

fn level_from_u8(level: u8) -> Result<ExecutionLevel, i32> {
    match level {
        0 => Ok(ExecutionLevel::L0),
        1 => Ok(ExecutionLevel::L1),
        2 => Ok(ExecutionLevel::L2),
        3 => Ok(ExecutionLevel::L3),
        _ => Err(IB_STATUS_INVALID_ARGUMENT),
    }
}

fn explicit_level_option_v1(level: i8) -> Result<Option<ExecutionLevel>, i32> {
    if level < 0 {
        Ok(None)
    } else {
        let level = u8::try_from(level).map_err(|_| IB_STATUS_INVALID_ARGUMENT)?;
        level_from_u8(level).map(Some)
    }
}

/// The v1 handle check, shared so v2 run functions accept harness handles
/// built by either ABI version.
fn v1_handle_ref<'a>(handle: *mut IbHarnessHandle) -> Result<&'a IbHarnessHandle, i32> {
    if handle.is_null() {
        return Err(IB_STATUS_INVALID_ARGUMENT);
    }
    // SAFETY: non-null was checked. The tag check rejects stale/swapped
    // pointers before the harness body is touched.
    let reference = unsafe { &*handle };
    if reference.magic != HARNESS_HANDLE_MAGIC {
        return Err(IB_STATUS_INVALID_ARGUMENT);
    }
    Ok(reference)
}

fn read_utf8_v1(span: IbByteSpanV1) -> Result<String, i32> {
    if span.len > 8 * 1024 * 1024 || (span.data.is_null() && span.len != 0) {
        return Err(IB_STATUS_INVALID_ARGUMENT);
    }
    let bytes = if span.len == 0 {
        &[][..]
    } else {
        // SAFETY: pointer/length validity is part of the C caller contract.
        unsafe { std::slice::from_raw_parts(span.data, span.len) }
    };
    String::from_utf8(bytes.to_vec()).map_err(|_| IB_STATUS_INVALID_ARGUMENT)
}

fn write_owned_v1(out: *mut IbOwnedBytesV1, value: String) -> Result<(), i32> {
    let mut boxed = value.into_bytes().into_boxed_slice();
    let len = boxed.len();
    let data = boxed.as_mut_ptr();
    std::mem::forget(boxed);
    // SAFETY: out is validated non-null by the exported caller.
    unsafe {
        (*out).data = data;
        (*out).len = len;
    }
    Ok(())
}

fn v2_boundary(operation: impl FnOnce() -> Result<(), i32>) -> i32 {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => IB_STATUS_OK,
        Ok(Err(status)) => status,
        Err(_panic) => IB_STATUS_PANIC,
    }
}

/// The value-returning variant of [`v2_boundary`]: panics inside the operation
/// become `IB_STATUS_PANIC` instead of an unwind across C.
fn v2_boundary_value<T>(operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or(Err(IB_STATUS_PANIC))
}

#[cfg(test)]
mod tests;
