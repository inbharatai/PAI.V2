//! ABI v2 registration tests. The "C side" here is Rust `extern "C"` statics
//! with `user_data` — the same calling pattern a JNI/Kotlin or C embedder
//! would compile against the `include/inbharat_harness.h` surface.
//!
//! Test-only relaxation: a panicking assertion IS this module's failure
//! signal (mutex poisoning and trait results included). The workspace-wide
//! `expect_used`/`unwrap_used` denies target production runtime code; the
//! v1 module below this crate already applies the same reasoning to
//! `clippy::panic` for its containment test.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Per-test state handed to every callback as `user_data`.
struct LoopState {
    model_calls: AtomicUsize,
    tool_calls: AtomicUsize,
    permission_asks: AtomicUsize,
    destroy_model: AtomicUsize,
    destroy_tool: AtomicUsize,
    destroy_permission: AtomicUsize,
    destroy_memory: AtomicUsize,
    /// Registration-owned output buffers: memory the callbacks own and keep
    /// valid until their next invocation (the out-span contract).
    tool_payload: std::sync::Mutex<String>,
    tool_model_content: std::sync::Mutex<String>,
    memory_records: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    memory_retrieve_payload: std::sync::Mutex<String>,
    last_actor: std::sync::Mutex<String>,
    last_capability: AtomicUsize,
    last_resource: std::sync::Mutex<String>,
}

impl LoopState {
    fn new() -> Self {
        Self {
            model_calls: AtomicUsize::new(0),
            tool_calls: AtomicUsize::new(0),
            permission_asks: AtomicUsize::new(0),
            destroy_model: AtomicUsize::new(0),
            destroy_tool: AtomicUsize::new(0),
            destroy_permission: AtomicUsize::new(0),
            destroy_memory: AtomicUsize::new(0),
            tool_payload: std::sync::Mutex::new(String::new()),
            tool_model_content: std::sync::Mutex::new(String::new()),
            memory_records: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            memory_retrieve_payload: std::sync::Mutex::new(String::new()),
            last_actor: std::sync::Mutex::new(String::new()),
            last_capability: AtomicUsize::new(0),
            last_resource: std::sync::Mutex::new(String::new()),
        }
    }
}

fn chunk(kind: u8) -> IbModelChunkV2 {
    IbModelChunkV2 {
        struct_size: size_of::<IbModelChunkV2>(),
        kind,
        block: 0,
        text: empty_span(),
        call_id: empty_span(),
        tool_id: empty_span(),
        arguments: empty_span(),
        input_units: 0,
        output_units: 0,
        finish_reason: 0,
    }
}

/// The scripted model: one ToolCall chunk ("c.echo" with `{"text":"from the
/// phone"}`) then Finish ToolCalls — the L1 single-action shape.
extern "C" fn loop_model_stream(
    user_data: *mut c_void,
    _request: *const IbModelRequestV2,
    _cancel: *mut IbCancellationHandle,
    emit: IbChunkEmitFn,
    emit_data: *mut c_void,
    out_response: *mut IbModelResponseV2,
) -> i32 {
    // SAFETY: user_data is the live LoopState for this test.
    let state = unsafe { &*(user_data.cast::<LoopState>()) };
    state.model_calls.fetch_add(1, Ordering::SeqCst);
    let mut tool_call = chunk(3);
    tool_call.call_id = span_of("call-1");
    tool_call.tool_id = span_of("c.echo");
    tool_call.arguments = span_of(r#"{"text":"from the phone"}"#);
    // SAFETY: emit/emit_data form the valid trampoline pair the library passes.
    if emit(emit_data, &tool_call) != IB_STATUS_OK {
        return IB_STATUS_OPERATION_FAILED;
    }
    let mut finish = chunk(6);
    finish.finish_reason = 1; // ToolCalls
    // SAFETY: as above.
    if emit(emit_data, &finish) != IB_STATUS_OK {
        return IB_STATUS_OPERATION_FAILED;
    }
    // SAFETY: out_response is writable storage supplied by the library.
    unsafe {
        *out_response = IbModelResponseV2 {
            struct_size: size_of::<IbModelResponseV2>(),
            finish_reason: 1,
            input_units: 7,
            output_units: 3,
        }
    };
    IB_STATUS_OK
}

extern "C" fn loop_tool_validate(_user_data: *mut c_void, _arguments_json: IbByteSpanV1) -> i32 {
    IB_STATUS_OK
}

/// The scripted tool: echoes the `text` argument into both the structured
/// value and the model-visible content. Out spans point into registration-
/// owned buffers kept valid until the next invocation (the out-span contract).
extern "C" fn loop_tool_execute(
    user_data: *mut c_void,
    arguments_json: IbByteSpanV1,
    context: *const IbToolContextV2,
    out_output: *mut IbToolOutputV2,
) -> i32 {
    // SAFETY: user_data is the live LoopState; context/out_output are the
    // library-supplied DTOs for this call.
    let state = unsafe { &*(user_data.cast::<LoopState>()) };
    let context = unsafe { &*context };
    state.tool_calls.fetch_add(1, Ordering::SeqCst);
    let arguments = match read_span(arguments_json) {
        Ok(value) => value,
        Err(_) => return IB_STATUS_INVALID_ARGUMENT,
    };
    let text = Value::parse_json(&arguments)
        .ok()
        .and_then(|value| value.as_object()?.get("text")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    *state.tool_payload.lock().unwrap() = format!("{{\"heard\":\"{text}\"}}");
    *state.tool_model_content.lock().unwrap() = format!("echo: {text}");
    *state.last_actor.lock().unwrap() = read_span(context.actor).unwrap_or_default();
    // The out spans must point into the registration-owned buffers (kept
    // valid until the next invocation), never into locals dropped at return.
    {
        let mut payload = state.tool_payload.lock().unwrap();
        let mut content = state.tool_model_content.lock().unwrap();
        *payload = format!("{{\"heard\":\"{text}\"}}");
        *content = format!("echo: {text}");
        // SAFETY: out_output is writable storage supplied by the library.
        unsafe {
            *out_output = IbToolOutputV2 {
                struct_size: size_of::<IbToolOutputV2>(),
                value_json: span_of(&payload),
                model_content: span_of(&content),
            }
        }
    }
    IB_STATUS_OK
}

extern "C" fn loop_permission_authorize(
    user_data: *mut c_void,
    actor: IbByteSpanV1,
    capability: u8,
    resource: IbByteSpanV1,
    out_decision: *mut IbPermissionDecisionV2,
) -> i32 {
    // SAFETY: user_data is the live LoopState; out_decision is writable
    // storage supplied by the library.
    let state = unsafe { &*(user_data.cast::<LoopState>()) };
    state.permission_asks.fetch_add(1, Ordering::SeqCst);
    *state.last_actor.lock().unwrap() = read_span(actor).unwrap_or_default();
    state
        .last_capability
        .store(usize::from(capability), Ordering::SeqCst);
    *state.last_resource.lock().unwrap() = read_span(resource).unwrap_or_default();
    // SAFETY: out_decision is writable storage supplied by the library.
    unsafe {
        *out_decision = IbPermissionDecisionV2 {
            struct_size: size_of::<IbPermissionDecisionV2>(),
            decision: IB_DECISION_ALLOW,
            rule_id: [0; 64],
            rule_id_len: 0,
            reason: [0; 256],
            reason_len: 0,
        }
    };
    IB_STATUS_OK
}

extern "C" fn destroy_counter_model(user_data: *mut c_void) {
    // SAFETY: user_data is the live LoopState for this test.
    unsafe {
        (*(user_data.cast::<LoopState>()))
            .destroy_model
            .fetch_add(1, Ordering::SeqCst)
    };
}

extern "C" fn destroy_counter_tool(user_data: *mut c_void) {
    // SAFETY: as above.
    unsafe {
        (*(user_data.cast::<LoopState>()))
            .destroy_tool
            .fetch_add(1, Ordering::SeqCst)
    };
}

extern "C" fn destroy_counter_permission(user_data: *mut c_void) {
    // SAFETY: as above.
    unsafe {
        (*(user_data.cast::<LoopState>()))
            .destroy_permission
            .fetch_add(1, Ordering::SeqCst)
    };
}

extern "C" fn destroy_counter_memory(user_data: *mut c_void) {
    // SAFETY: as above.
    unsafe {
        (*(user_data.cast::<LoopState>()))
            .destroy_memory
            .fetch_add(1, Ordering::SeqCst)
    };
}

// ---------------------------------------------------------------------------
// C-side memory vtable (registration-owned JSON records)
// ---------------------------------------------------------------------------

extern "C" fn loop_memory_retrieve(
    user_data: *mut c_void,
    _scope: u8,
    _namespace: IbByteSpanV1,
    id: IbByteSpanV1,
    out_record_json: *mut IbByteSpanV1,
) -> i32 {
    // SAFETY: user_data is the live LoopState; the out span is writable.
    let state = unsafe { &*(user_data.cast::<LoopState>()) };
    let id = match read_span(id) {
        Ok(value) => value,
        Err(_) => return IB_STATUS_INVALID_ARGUMENT,
    };
    let records = state.memory_records.lock().unwrap();
    match records.get(&id) {
        Some(record) => {
            // The span must point into the registration-owned buffer, never
            // into a local clone that dies at return.
            let mut payload = state.memory_retrieve_payload.lock().unwrap();
            *payload = record.clone();
            drop(records);
            // SAFETY: the span points into the registration-owned buffer.
            unsafe { *out_record_json = span_of(&payload) };
        }
        None => {
            drop(records);
            // SAFETY: an empty span is the documented "not found" answer.
            unsafe { *out_record_json = empty_span() };
        }
    }
    IB_STATUS_OK
}

extern "C" fn loop_memory_search(
    _user_data: *mut c_void,
    _scope: u8,
    _namespace: IbByteSpanV1,
    _text: IbByteSpanV1,
    _limit: u32,
    out_records_json: *mut IbByteSpanV1,
) -> i32 {
    // SAFETY: an empty array is valid output (no records match in this stub).
    unsafe { *out_records_json = span_of("[]") };
    IB_STATUS_OK
}

extern "C" fn loop_memory_store(user_data: *mut c_void, record_json: IbByteSpanV1) -> i32 {
    // SAFETY: user_data is the live LoopState.
    let state = unsafe { &*(user_data.cast::<LoopState>()) };
    let record = match read_span(record_json) {
        Ok(value) => value,
        Err(_) => return IB_STATUS_INVALID_ARGUMENT,
    };
    let parsed = match Value::parse_json(&record) {
        Ok(value) => value,
        Err(_) => return IB_STATUS_INVALID_ARGUMENT,
    };
    let id = parsed
        .as_object()
        .and_then(|object| object.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    match id {
        Some(id) => {
            state.memory_records.lock().unwrap().insert(id, record);
            IB_STATUS_OK
        }
        None => IB_STATUS_INVALID_ARGUMENT,
    }
}

extern "C" fn loop_memory_update(user_data: *mut c_void, record_json: IbByteSpanV1) -> i32 {
    loop_memory_store(user_data, record_json)
}

extern "C" fn loop_memory_delete(
    user_data: *mut c_void,
    _scope: u8,
    _namespace: IbByteSpanV1,
    id: IbByteSpanV1,
    out_found: *mut u8,
) -> i32 {
    // SAFETY: user_data is the live LoopState; out_found is a writable byte.
    let state = unsafe { &*(user_data.cast::<LoopState>()) };
    let id = match read_span(id) {
        Ok(value) => value,
        Err(_) => return IB_STATUS_INVALID_ARGUMENT,
    };
    let removed = state.memory_records.lock().unwrap().remove(&id).is_some();
    // SAFETY: out_found is writable storage supplied by the library.
    unsafe { *out_found = u8::from(removed) };
    IB_STATUS_OK
}

// ---------------------------------------------------------------------------
// DTO builders
// ---------------------------------------------------------------------------

fn config_v2(root: &str) -> IbHarnessConfigV2 {
    IbHarnessConfigV2 {
        struct_size: size_of::<IbHarnessConfigV2>(),
        abi_version: IB_HARNESS_ABI_VERSION_V2,
        root: span_of(root),
        maximum_level: 2,
        allow_explicit_escalation: 1,
        system_prefix: empty_span(),
        reserved: [0; 6],
    }
}

fn model_provider_dto(state: &LoopState) -> IbModelProviderV2 {
    IbModelProviderV2 {
        struct_size: size_of::<IbModelProviderV2>(),
        id: span_of("c-provider"),
        models_json: span_of(r#"["c-model-1"]"#),
        user_data: state as *const LoopState as *mut c_void,
        stream: loop_model_stream,
        destroy: Some(destroy_counter_model),
    }
}

fn tool_dto(state: &LoopState) -> IbToolV2 {
    IbToolV2 {
        struct_size: size_of::<IbToolV2>(),
        id: span_of("c.echo"),
        version: span_of("1.0.0"),
        description: span_of("Echoes the text argument (C-registered test tool)"),
        input_schema: span_of(
            r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}"#,
        ),
        output_schema: span_of(
            r#"{"type":"object","properties":{"heard":{"type":"string"}},"additionalProperties":false}"#,
        ),
        // Model (bit 0) + FileRead (bit 1).
        required_capabilities: 0b11,
        supported_levels: 0b1111,
        determinism: 0,
        side_effect: 0,
        confirmation: 0,
        concurrency_safe: 1,
        default_timeout_ms: 1_000,
        max_output_bytes: 8_192,
        verification: span_of("echo deterministically maps the text argument"),
        compensation: span_of("no state, nothing to compensate"),
        user_data: state as *const LoopState as *mut c_void,
        validate: loop_tool_validate,
        execute: loop_tool_execute,
        destroy: Some(destroy_counter_tool),
    }
}

fn permission_dto(state: &LoopState) -> IbPermissionProviderV2 {
    IbPermissionProviderV2 {
        struct_size: size_of::<IbPermissionProviderV2>(),
        user_data: state as *const LoopState as *mut c_void,
        authorize: loop_permission_authorize,
        destroy: Some(destroy_counter_permission),
    }
}

fn memory_dto(state: &LoopState) -> IbMemoryProviderV2 {
    IbMemoryProviderV2 {
        struct_size: size_of::<IbMemoryProviderV2>(),
        // bit 0 conversation + bit 1 preferences.
        scopes_bitmask: 0b11,
        can_retrieve: 1,
        can_search: 1,
        can_store: 1,
        can_update: 1,
        can_delete: 1,
        max_results: 16,
        user_data: state as *const LoopState as *mut c_void,
        retrieve: loop_memory_retrieve,
        search: loop_memory_search,
        store: loop_memory_store,
        update: loop_memory_update,
        delete: loop_memory_delete,
        destroy: Some(destroy_counter_memory),
    }
}

fn read_owned(bytes: &mut IbOwnedBytesV1) -> String {
    let text = String::from_utf8(
        // SAFETY: the data/length pair is library-owned memory returned by run_v2.
        unsafe { std::slice::from_raw_parts(bytes.data, bytes.len) }.to_vec(),
    )
    .unwrap_or_default();
    // SAFETY: releasing library-owned bytes exactly once.
    unsafe { crate::ib_harness_bytes_free_v1(bytes) };
    text
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The headline ABI-v2 path: build a harness whose model, tool and permission
/// implementations are all C vtables, run one L1 single-action turn, and get
/// the run output PLUS a hash-chained audit ledger.
#[test]
fn registered_c_vtables_drive_one_l1_tool_loop_and_return_a_verifiable_ledger() {
    let state = LoopState::new();
    let config = config_v2(".");
    let mut builder = ptr::null_mut();
    // SAFETY: valid size-tagged pointers for the duration of each call.
    assert_eq!(
        unsafe { ib_harness_builder_create_v2(&config, &mut builder) },
        IB_STATUS_OK
    );
    let provider = model_provider_dto(&state);
    let tool = tool_dto(&state);
    let permission = permission_dto(&state);
    let memory = memory_dto(&state);
    // SAFETY: the builder handle and DTOs are valid for each call.
    assert_eq!(
        unsafe { ib_harness_builder_register_model_v2(builder, &provider) },
        IB_STATUS_OK
    );
    assert_eq!(
        unsafe { ib_harness_builder_register_tool_v2(builder, &tool) },
        IB_STATUS_OK
    );
    assert_eq!(
        unsafe { ib_harness_builder_register_permission_v2(builder, &permission) },
        IB_STATUS_OK
    );
    assert_eq!(
        unsafe { ib_harness_builder_register_memory_v2(builder, &memory) },
        IB_STATUS_OK
    );
    let mut harness = ptr::null_mut();
    // SAFETY: the builder is consumed by build.
    assert_eq!(
        unsafe { ib_harness_builder_build_v2(builder, &mut harness) },
        IB_STATUS_OK
    );
    assert!(!harness.is_null());

    let mut output = IbOwnedBytesV1::default();
    let mut audit = IbOwnedBytesV1::default();
    let mut cancellation = crate::IbCancellationHandle {
        magic: CANCEL_HANDLE_MAGIC,
        token: CancellationToken::new(),
    };
    // SAFETY: harness handle, spans and outputs are valid for the call.
    let status = unsafe {
        ib_harness_run_v2(
            harness,
            span_of("echo the phrase back"),
            1, // explicit L1: one model-selected tool call
            span_of("c-provider"),
            span_of("c-model-1"),
            0b11, // Model + FileRead
            ptr::from_mut(&mut cancellation),
            &mut output,
            &mut audit,
        )
    };
    assert_eq!(status, IB_STATUS_OK, "the registered C loop must complete");
    let output_text = read_owned(&mut output);
    assert_eq!(output_text, "echo: from the phone");
    assert_eq!(state.model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.tool_calls.load(Ordering::SeqCst), 1);
    // The core authorizes EVERY required capability separately (one Ask per
    // capability): the tool requires Model + FileRead, so the permission
    // vtable sees two authorize calls, the last one for FileRead.
    assert_eq!(state.permission_asks.load(Ordering::SeqCst), 2);
    assert_eq!(*state.last_actor.lock().unwrap(), "local-user");
    assert_eq!(state.last_capability.load(Ordering::SeqCst), 1, "FileRead");
    assert_eq!(*state.last_resource.lock().unwrap(), "c.echo");

    // The audit ledger: canonical JSONL with the hash chain present.
    let ledger = read_owned(&mut audit);
    assert!(!ledger.is_empty(), "the audit ledger must be returned");
    let lines: Vec<&str> = ledger.lines().collect();
    assert!(lines.len() >= 4, "a tool-loop run must audit its events");
    for line in &lines {
        let event = Value::parse_json(line).expect("every ledger line is canonical JSON");
        let object = event.as_object().expect("ledger lines are objects");
        assert!(
            object.contains_key("chain"),
            "the hash chain travels with each event"
        );
        assert!(object.contains_key("seq"));
        assert!(object.contains_key("type"));
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"type\":\"tool.call\"")),
        "the dispatched C tool call is audited"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"type\":\"tool.result\"")),
        "the C tool result is audited"
    );

    // Destroying the harness releases the registrations exactly once each.
    // SAFETY: the harness handle was built by build_v2 and is destroyed once.
    assert_eq!(
        unsafe { crate::ib_harness_destroy_v1(harness) },
        IB_STATUS_OK
    );
    assert_eq!(state.destroy_model.load(Ordering::SeqCst), 1);
    assert_eq!(state.destroy_tool.load(Ordering::SeqCst), 1);
    assert_eq!(state.destroy_permission.load(Ordering::SeqCst), 1);
    assert_eq!(state.destroy_memory.load(Ordering::SeqCst), 1);
}

/// A v2 build is embedding-safe: no built-in tools, and without a registered
/// model every run fails CLOSED — the ABI never fabricates a default model.
#[test]
fn v2_build_fails_closed_without_a_registered_model_but_routes() {
    let config = config_v2(".");
    let mut builder = ptr::null_mut();
    // SAFETY: valid size-tagged pointers for the call.
    assert_eq!(
        unsafe { ib_harness_builder_create_v2(&config, &mut builder) },
        IB_STATUS_OK
    );
    let mut harness = ptr::null_mut();
    // SAFETY: the builder is consumed by build.
    assert_eq!(
        unsafe { ib_harness_builder_build_v2(builder, &mut harness) },
        IB_STATUS_OK
    );
    // The handle is v1-compatible: pure routing works on it.
    let mut level = 255_u8;
    // SAFETY: handle and output pointer are live.
    assert_eq!(
        unsafe { crate::ib_harness_route_v1(harness, span_of("hello"), -1, &mut level) },
        IB_STATUS_OK
    );
    assert!(level <= 3);
    // But a run with an unregistered provider/model fails closed.
    let mut output = IbOwnedBytesV1::default();
    // SAFETY: handle, spans and outputs are valid for the call.
    let status = unsafe {
        ib_harness_run_v2(
            harness,
            span_of("hello"),
            1,
            span_of("no-such-provider"),
            span_of("no-such-model"),
            0b11,
            ptr::null_mut(),
            &mut output,
            ptr::null_mut(),
        )
    };
    assert_eq!(status, IB_STATUS_UNAVAILABLE);
    assert!(output.data.is_null());
    // SAFETY: the harness handle is destroyed exactly once.
    assert_eq!(
        unsafe { crate::ib_harness_destroy_v1(harness) },
        IB_STATUS_OK
    );
}

/// A duplicate registration is rejected, consumes the builder (core contract),
/// fires the rejected adapter's destroy immediately, and leaves the handle
/// safe to destroy exactly once.
#[test]
fn duplicate_model_registration_is_rejected_and_consumes_the_builder() {
    let state = LoopState::new();
    let config = config_v2(".");
    let mut builder = ptr::null_mut();
    // SAFETY: valid size-tagged pointers for the call.
    assert_eq!(
        unsafe { ib_harness_builder_create_v2(&config, &mut builder) },
        IB_STATUS_OK
    );
    let provider = model_provider_dto(&state);
    // SAFETY: the builder handle and DTO are valid for each call.
    assert_eq!(
        unsafe { ib_harness_builder_register_model_v2(builder, &provider) },
        IB_STATUS_OK
    );
    let duplicate = model_provider_dto(&state);
    let rejected = unsafe { ib_harness_builder_register_model_v2(builder, &duplicate) };
    assert_eq!(
        rejected, IB_STATUS_OPERATION_FAILED,
        "a duplicate id is a conflict"
    );
    // The rejected adapter is released, AND the consumed builder releases its
    // earlier successful registration (core contract: register_model consumes
    // the builder on error) — two destroys, both through the same counter.
    assert_eq!(
        state.destroy_model.load(Ordering::SeqCst),
        2,
        "the rejected adapter and the consumed builder's earlier registration are both released"
    );
    // The builder is consumed: any later registration is invalid-argument.
    let tool = tool_dto(&state);
    // SAFETY: the (dead) builder handle and DTO are valid pointers.
    assert_eq!(
        unsafe { ib_harness_builder_register_tool_v2(builder, &tool) },
        IB_STATUS_INVALID_ARGUMENT
    );
    // SAFETY: the handle is still destroyed exactly once.
    assert_eq!(
        unsafe { ib_harness_builder_destroy_v2(builder) },
        IB_STATUS_OK
    );
}

/// Argument validation happens BEFORE the builder is taken: a too-small size
/// tag is rejected without consuming the builder, and destroy has not fired.
#[test]
fn bad_size_tag_is_rejected_without_consuming_the_builder() {
    let state = LoopState::new();
    let config = config_v2(".");
    let mut builder = ptr::null_mut();
    // SAFETY: valid size-tagged pointers for the call.
    assert_eq!(
        unsafe { ib_harness_builder_create_v2(&config, &mut builder) },
        IB_STATUS_OK
    );
    let mut provider = model_provider_dto(&state);
    provider.struct_size = 4; // far too small
    // SAFETY: the DTO pointer is valid; its size tag is the value under test.
    assert_eq!(
        unsafe { ib_harness_builder_register_model_v2(builder, &provider) },
        IB_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        state.destroy_model.load(Ordering::SeqCst),
        0,
        "nothing was registered"
    );
    // The builder still works: a valid registration succeeds afterwards.
    let good = model_provider_dto(&state);
    // SAFETY: the builder handle and DTO are valid for the call.
    assert_eq!(
        unsafe { ib_harness_builder_register_model_v2(builder, &good) },
        IB_STATUS_OK
    );
    // SAFETY: the handle is destroyed exactly once.
    assert_eq!(
        unsafe { ib_harness_builder_destroy_v2(builder) },
        IB_STATUS_OK
    );
    // Abandoning without build releases the registration.
    assert_eq!(state.destroy_model.load(Ordering::SeqCst), 1);
}

/// A pre-cancelled run reports CANCELLED without ever calling the model.
#[test]
fn pre_cancelled_run_v2_reports_cancelled() {
    let state = LoopState::new();
    let config = config_v2(".");
    let mut builder = ptr::null_mut();
    // SAFETY: valid size-tagged pointers for the call.
    assert_eq!(
        unsafe { ib_harness_builder_create_v2(&config, &mut builder) },
        IB_STATUS_OK
    );
    let provider = model_provider_dto(&state);
    // SAFETY: the builder handle and DTO are valid for the call.
    assert_eq!(
        unsafe { ib_harness_builder_register_model_v2(builder, &provider) },
        IB_STATUS_OK
    );
    let mut harness = ptr::null_mut();
    // SAFETY: the builder is consumed by build.
    assert_eq!(
        unsafe { ib_harness_builder_build_v2(builder, &mut harness) },
        IB_STATUS_OK
    );
    let mut cancellation = ptr::null_mut();
    // SAFETY: the output pointer receives one owned cancellation handle.
    assert_eq!(
        unsafe { crate::ib_harness_cancel_create_v1(&mut cancellation) },
        IB_STATUS_OK
    );
    // SAFETY: the cancellation handle is live.
    assert_eq!(
        unsafe { crate::ib_harness_cancel_request_v1(cancellation, 0) },
        IB_STATUS_OK
    );
    // The C side can poll the same token through the v2 helper.
    // SAFETY: the cancellation handle is live.
    assert_eq!(unsafe { ib_harness_cancel_requested_v2(cancellation) }, 1);
    let mut output = IbOwnedBytesV1::default();
    // SAFETY: handle, spans, cancellation and outputs are valid for the call.
    let status = unsafe {
        ib_harness_run_v2(
            harness,
            span_of("never runs"),
            1,
            span_of("c-provider"),
            span_of("c-model-1"),
            0b11,
            cancellation,
            &mut output,
            ptr::null_mut(),
        )
    };
    assert_eq!(status, IB_STATUS_CANCELLED);
    assert!(output.data.is_null());
    assert_eq!(
        state.model_calls.load(Ordering::SeqCst),
        0,
        "the model was never called"
    );
    // SAFETY: both handles are destroyed exactly once.
    assert_eq!(
        unsafe { crate::ib_harness_cancel_destroy_v1(cancellation) },
        IB_STATUS_OK
    );
    assert_eq!(
        unsafe { crate::ib_harness_destroy_v1(harness) },
        IB_STATUS_OK
    );
}

/// A stale or type-confused builder pointer is rejected by the magic tag
/// before the builder body is touched.
#[test]
fn stale_builder_handle_is_rejected_by_magic_tag() {
    let config = config_v2(".");
    let mut builder = ptr::null_mut();
    // SAFETY: valid size-tagged pointers for the call.
    assert_eq!(
        unsafe { ib_harness_builder_create_v2(&config, &mut builder) },
        IB_STATUS_OK
    );
    // Corrupt the tag, as a stale/foreign pointer would have.
    // SAFETY: builder is a live owned handle; only the tag is rewritten.
    unsafe { (*builder).magic = 0 };
    // SAFETY: the pointer is valid; the tag check is the value under test.
    assert_eq!(
        unsafe { ib_harness_builder_destroy_v2(builder) },
        IB_STATUS_INVALID_ARGUMENT
    );
    // Restore and destroy exactly once.
    // SAFETY: builder is still the live owned handle.
    unsafe { (*builder).magic = BUILDER_HANDLE_MAGIC };
    // SAFETY: the handle is destroyed exactly once.
    assert_eq!(
        unsafe { ib_harness_builder_destroy_v2(builder) },
        IB_STATUS_OK
    );
}

/// The C memory adapter round-trips records through the vtable with the
/// core's own validation active (bounded JSON records, not found = empty).
#[test]
fn c_memory_vtable_round_trips_through_the_core_trait() {
    let state = LoopState::new();
    let adapter = CMemoryProvider {
        registration: CRegistration {
            user_data: CUserData(&state as *const LoopState as *mut c_void),
            destroy: Some(destroy_counter_memory),
        },
        capabilities: MemoryCapabilities {
            scopes: vec![MemoryScope::Conversation, MemoryScope::Preferences],
            can_retrieve: true,
            can_search: true,
            can_store: true,
            can_update: true,
            can_delete: true,
            max_results: 16,
        },
        retrieve: loop_memory_retrieve,
        search: loop_memory_search,
        store: loop_memory_store,
        update: loop_memory_update,
        delete: loop_memory_delete,
    };
    // Store through the trait, then retrieve.
    let record = MemoryRecord {
        id: "pref-1".to_owned(),
        scope: MemoryScope::Preferences,
        namespace: "vault".to_owned(),
        content: "the founder uses a Windows host".to_owned(),
        attributes: BTreeMap::new(),
    };
    adapter.store(record).expect("store must succeed");
    let found = adapter
        .retrieve(MemoryScope::Preferences, "vault", "pref-1")
        .expect("retrieve must succeed")
        .expect("the stored record must be found");
    assert_eq!(found.content, "the founder uses a Windows host");
    assert_eq!(found.scope, MemoryScope::Preferences);
    let missing = adapter
        .retrieve(MemoryScope::Preferences, "vault", "missing")
        .expect("retrieve of a missing id is not an error");
    assert!(
        missing.is_none(),
        "not found is an empty span, not an error"
    );
    // Search returns the empty array through the JSON boundary.
    let query = MemoryQuery {
        scope: MemoryScope::Preferences,
        namespace: Some("vault".to_owned()),
        text: "founder".to_owned(),
        limit: 8,
    };
    let results = adapter.search(&query).expect("search must succeed");
    assert!(results.is_empty(), "the stub returns no matches");
    // Delete reports what existed.
    assert!(
        adapter
            .delete(MemoryScope::Preferences, "vault", "pref-1")
            .expect("delete must succeed")
    );
    assert!(
        !adapter
            .delete(MemoryScope::Preferences, "vault", "pref-1")
            .expect("second delete must succeed")
    );
    drop(adapter);
    assert_eq!(
        state.destroy_memory.load(Ordering::SeqCst),
        1,
        "destroy fired once on drop"
    );
}
