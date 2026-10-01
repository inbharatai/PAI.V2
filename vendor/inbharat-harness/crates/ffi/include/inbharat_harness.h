#ifndef INBHARAT_HARNESS_H
#define INBHARAT_HARNESS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define IB_HARNESS_ABI_VERSION 1u
#define IB_STATUS_OK 0
#define IB_STATUS_INVALID_ARGUMENT 1
#define IB_STATUS_DENIED 2
#define IB_STATUS_UNAVAILABLE 3
#define IB_STATUS_CANCELLED 4
#define IB_STATUS_RESOURCE_EXHAUSTED 5
#define IB_STATUS_OPERATION_FAILED 6
#define IB_STATUS_PANIC 255

#define IB_CANCEL_USER 0
#define IB_CANCEL_PARENT 1
#define IB_CANCEL_DEADLINE 2
#define IB_CANCEL_POLICY 3
#define IB_CANCEL_SHUTDOWN 4
#define IB_CANCEL_DISPOSED 5

typedef struct IbHarnessHandle IbHarnessHandle;
typedef struct IbCancellationHandle IbCancellationHandle;

typedef struct IbByteSpanV1 {
    uint32_t struct_size;
    const uint8_t *data;
    size_t len;
} IbByteSpanV1;

typedef struct IbOwnedBytesV1 {
    uint32_t struct_size;
    uint8_t *data;
    size_t len;
} IbOwnedBytesV1;

typedef struct IbHarnessConfigV1 {
    uint32_t struct_size;
    uint32_t abi_version;
    IbByteSpanV1 root;
    uint8_t maximum_level;
    uint8_t reserved[7];
} IbHarnessConfigV1;

uint32_t ib_harness_api_version_v1(void);
int32_t ib_harness_create_v1(const IbHarnessConfigV1 *config, IbHarnessHandle **out_handle);
int32_t ib_harness_destroy_v1(IbHarnessHandle *handle);
int32_t ib_harness_cancel_create_v1(IbCancellationHandle **out_handle);
int32_t ib_harness_cancel_request_v1(IbCancellationHandle *handle, uint8_t cause);
int32_t ib_harness_cancel_destroy_v1(IbCancellationHandle *handle);
int32_t ib_harness_route_v1(IbHarnessHandle *handle, IbByteSpanV1 prompt,
                            int8_t explicit_level, uint8_t *out_level);
int32_t ib_harness_run_v1(IbHarnessHandle *handle, IbByteSpanV1 prompt,
                          int8_t explicit_level, IbOwnedBytesV1 *out_bytes);
int32_t ib_harness_run_with_cancel_v1(IbHarnessHandle *handle, IbByteSpanV1 prompt,
                                      int8_t explicit_level,
                                      IbCancellationHandle *cancellation,
                                      IbOwnedBytesV1 *out_bytes);
int32_t ib_harness_bytes_free_v1(IbOwnedBytesV1 *bytes);
const char *ib_harness_status_message_v1(int32_t status);

/* ---------------------------------------------------------------------------
 * ABI v2 — builder-stage registration surface (embedding-safe: NO built-in
 * tools). C/Kotlin embedders register their own model provider, tools, memory
 * and permission authority before build. The lifecycle is:
 *   create -> register* (any order, each may fail) -> build -> run* -> destroy
 * Contract (two rules, enforced and tested in the Rust source):
 *   1. A rejected registration CONSUMES the builder handle: its destroy
 *      callbacks fire immediately (for the rejected adapter and for every
 *      earlier registration), later registrations return
 *      IB_STATUS_INVALID_ARGUMENT, and destroy still runs exactly once.
 *      Argument validation happens BEFORE the builder is taken, so a bad
 *      size tag or null span leaves the builder usable.
 *   2. All JSON crosses the boundary as the core's bounded UTF-8 JSON; all
 *      out-spans a callback fills must point to memory the registration owns
 *      and keeps valid until the same callback's next invocation or destroy
 *      (the library copies immediately after each call).
 * ------------------------------------------------------------------------ */

#define IB_HARNESS_ABI_VERSION_V2 2u
#define IB_DECISION_ALLOW 0u
#define IB_DECISION_ASK 1u
#define IB_DECISION_DENY 2u

/* IbModelChunkV2.kind */
#define IB_CHUNK_START 0u
#define IB_CHUNK_TEXT_DELTA 1u
#define IB_CHUNK_REASONING_DELTA 2u
#define IB_CHUNK_TOOL_CALL 3u
#define IB_CHUNK_END 4u
#define IB_CHUNK_USAGE 5u
#define IB_CHUNK_FINISH 6u

/* IbModelResponseV2.finish_reason */
#define IB_FINISH_STOP 0u
#define IB_FINISH_TOOL_CALLS 1u
#define IB_FINISH_LENGTH 2u
#define IB_FINISH_CANCELLED 3u
#define IB_FINISH_ERROR 4u

typedef struct IbBuilderHandleV2 IbBuilderHandleV2; /* opaque; library-owned */

typedef void (*IbDestroyFn)(void *user_data); /* exactly once per registration */
typedef int32_t (*IbChunkEmitFn)(void *emit_data, const struct IbModelChunkV2 *chunk);
typedef int32_t (*IbModelStreamFn)(void *user_data,
                                   const struct IbModelRequestV2 *request,
                                   IbCancellationHandle *cancel,
                                   IbChunkEmitFn emit,
                                   void *emit_data,
                                   struct IbModelResponseV2 *out_response);
typedef int32_t (*IbToolValidateFn)(void *user_data, IbByteSpanV1 arguments_json);
typedef int32_t (*IbToolExecuteFn)(void *user_data,
                                   IbByteSpanV1 arguments_json,
                                   const struct IbToolContextV2 *context,
                                   struct IbToolOutputV2 *out_output);
typedef int32_t (*IbMemoryRetrieveFn)(void *user_data,
                                      uint8_t scope,
                                      IbByteSpanV1 namespace,
                                      IbByteSpanV1 id,
                                      IbByteSpanV1 *out_record_json);
typedef int32_t (*IbMemorySearchFn)(void *user_data,
                                    uint8_t scope,
                                    IbByteSpanV1 namespace,
                                    IbByteSpanV1 text,
                                    uint32_t limit,
                                    IbByteSpanV1 *out_records_json);
typedef int32_t (*IbMemoryWriteFn)(void *user_data, IbByteSpanV1 record_json);
typedef int32_t (*IbMemoryDeleteFn)(void *user_data,
                                    uint8_t scope,
                                    IbByteSpanV1 namespace,
                                    IbByteSpanV1 id,
                                    uint8_t *out_found);
typedef int32_t (*IbPermissionAuthorizeFn)(void *user_data,
                                           IbByteSpanV1 actor,
                                           uint8_t capability,
                                           IbByteSpanV1 resource,
                                           struct IbPermissionDecisionV2 *out_decision);

typedef struct IbHarnessConfigV2 {
    uint32_t struct_size;
    uint32_t abi_version;
    IbByteSpanV1 root;
    uint8_t maximum_level;
    uint8_t allow_explicit_escalation; /* 0 denied, 1 allowed */
    IbByteSpanV1 system_prefix;        /* may be empty */
    uint8_t reserved[6];
} IbHarnessConfigV2;

typedef struct IbModelRequestV2 {
    uint32_t struct_size;
    IbByteSpanV1 request_id;
    IbByteSpanV1 provider;
    IbByteSpanV1 model;
    IbByteSpanV1 system;
    IbByteSpanV1 messages_json; /* JSON array of {"role","content"} objects */
    IbByteSpanV1 tools_json;     /* JSON array of {"id","description","input_schema"} */
    size_t max_output_bytes;
} IbModelRequestV2;

/* kind: IB_CHUNK_* above; text/call_id/tool_id/arguments read per kind;
 * finish_reason is only read for kind == IB_CHUNK_FINISH. */
typedef struct IbModelChunkV2 {
    uint32_t struct_size;
    uint8_t kind;
    uint32_t block;
    IbByteSpanV1 text;
    IbByteSpanV1 call_id;
    IbByteSpanV1 tool_id;
    IbByteSpanV1 arguments;
    uint64_t input_units;
    uint64_t output_units;
    uint8_t finish_reason;
} IbModelChunkV2;

typedef struct IbModelResponseV2 {
    uint32_t struct_size;
    uint8_t finish_reason; /* IB_FINISH_* */
    uint64_t input_units;
    uint64_t output_units;
} IbModelResponseV2;

typedef struct IbModelProviderV2 {
    uint32_t struct_size;
    IbByteSpanV1 id;
    IbByteSpanV1 models_json; /* JSON array of model ids */
    void *user_data;
    IbModelStreamFn stream;
    IbDestroyFn destroy; /* optional */
} IbModelProviderV2;

/* The Rust ExecutionBroker is deliberately NOT exposed: brokered host work
 * stays on the Rust side; C tools do their own work inside the non-bypassable
 * confirmation/audit/budget wrap. */
typedef struct IbToolContextV2 {
    uint32_t struct_size;
    IbByteSpanV1 actor;
    uint8_t level; /* L0..L3 */
    IbCancellationHandle *cancel;
} IbToolContextV2;

typedef struct IbToolOutputV2 {
    uint32_t struct_size;
    IbByteSpanV1 value_json;     /* parsed into the structured output; empty = null */
    IbByteSpanV1 model_content;  /* model-visible text */
} IbToolOutputV2;

/* required_capabilities bit i == Capability discriminant i
 * (0 model, 1 file.read, 2 file.write, 3 process.spawn, 4 network,
 *  5 credential, 6 workspace, 7 job, 8 subagent).
 * supported_levels bits 0..3 == L0..L3.
 * determinism 0 Deterministic / 1 Idempotent / 2 NonIdempotent.
 * side_effect 0 None / 1 Read / 2 Write / 3 Process / 4 Network.
 * confirmation 0 Never / 1 OnSideEffect / 2 Always. */
typedef struct IbToolV2 {
    uint32_t struct_size;
    IbByteSpanV1 id;
    IbByteSpanV1 version;
    IbByteSpanV1 description;
    IbByteSpanV1 input_schema;
    IbByteSpanV1 output_schema;
    uint32_t required_capabilities;
    uint8_t supported_levels;
    uint8_t determinism;
    uint8_t side_effect;
    uint8_t confirmation;
    uint8_t concurrency_safe;
    uint32_t default_timeout_ms;
    uint32_t max_output_bytes;
    IbByteSpanV1 verification;
    IbByteSpanV1 compensation;
    void *user_data;
    IbToolValidateFn validate;
    IbToolExecuteFn execute;
    IbDestroyFn destroy; /* optional */
} IbToolV2;

/* scopes_bitmask bit i == MemoryScope discriminant i+1
 * (0 conversation, 1 preferences, 2 relevant, 3 project, 4 document,
 *  5 extended); the core always validates scope, so bit 6+ is rejected. */
typedef struct IbMemoryProviderV2 {
    uint32_t struct_size;
    uint32_t scopes_bitmask;
    uint8_t can_retrieve;
    uint8_t can_search;
    uint8_t can_store;
    uint8_t can_update;
    uint8_t can_delete;
    uint32_t max_results;
    void *user_data;
    IbMemoryRetrieveFn retrieve;
    IbMemorySearchFn search;
    IbMemoryWriteFn store;
    IbMemoryWriteFn update;
    IbMemoryDeleteFn delete;
    IbDestroyFn destroy; /* optional */
} IbMemoryProviderV2;

typedef struct IbPermissionDecisionV2 {
    uint32_t struct_size;
    uint8_t decision; /* IB_DECISION_* */
    uint8_t rule_id[64];
    uint32_t rule_id_len;
    uint8_t reason[256];
    uint32_t reason_len;
} IbPermissionDecisionV2;

typedef struct IbPermissionProviderV2 {
    uint32_t struct_size;
    void *user_data;
    IbPermissionAuthorizeFn authorize;
    IbDestroyFn destroy; /* optional */
} IbPermissionProviderV2;

uint32_t ib_harness_api_version_v2(void);
int32_t ib_harness_builder_create_v2(const IbHarnessConfigV2 *config,
                                     IbBuilderHandleV2 **out_builder);
int32_t ib_harness_builder_register_model_v2(IbBuilderHandleV2 *builder,
                                             const IbModelProviderV2 *provider);
int32_t ib_harness_builder_register_tool_v2(IbBuilderHandleV2 *builder,
                                            const IbToolV2 *tool);
int32_t ib_harness_builder_register_memory_v2(IbBuilderHandleV2 *builder,
                                              const IbMemoryProviderV2 *memory);
int32_t ib_harness_builder_register_permission_v2(IbBuilderHandleV2 *builder,
                                                  const IbPermissionProviderV2 *permission);
/* Consumes the builder on BOTH success and failure; the returned harness is
 * released with ib_harness_destroy_v1. */
int32_t ib_harness_builder_build_v2(IbBuilderHandleV2 *builder,
                                     IbHarnessHandle **out_handle);
int32_t ib_harness_builder_destroy_v2(IbBuilderHandleV2 *builder);

/* Polls one cancellation handle from inside a C callback: 1 when cancellation
 * was requested, 0 when not; rejects null/stale/type-confused handles. */
int32_t ib_harness_cancel_requested_v2(IbCancellationHandle *handle);

/* One bounded run with caller-selected provider, model and capability grants.
 * capabilities bit i == Capability discriminant i; the Model bit (0) is
 * required. `cancellation` may be NULL. `out_audit` (optional) receives the
 * run's tamper-evident ledger: hash-chained session events, one canonical
 * JSON line per event. Both outputs are library-owned; release with
 * ib_harness_bytes_free_v1. */
int32_t ib_harness_run_v2(IbHarnessHandle *handle,
                          IbByteSpanV1 prompt,
                          int8_t explicit_level,
                          IbByteSpanV1 provider,
                          IbByteSpanV1 model,
                          uint32_t capabilities,
                          IbCancellationHandle *cancellation,
                          IbOwnedBytesV1 *out_bytes,
                          IbOwnedBytesV1 *out_audit);

#ifdef __cplusplus
}
#endif

#endif
