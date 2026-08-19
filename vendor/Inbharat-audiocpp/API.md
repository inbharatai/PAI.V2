# C99 API guide

Include `inbharat/ibaudio.h`. Call every `*_options_init()` before setting fields. All extensible input structs start with `struct_size` and `api_version`; v1 accepts larger structs with the same ABI major and ignores unknown trailing fields.

## Ownership

- Runtime → models → sessions → jobs/streams is the parent/child order.
- Parent release returns `BUSY` while children (or runtime-owned output buffers) remain.
- Synchronous inputs are borrowed for the call only.
- Job start copies audio/text before returning.
- Every output `ibaudio_buffer_t*` is immutable and caller-owned until `ibaudio_buffer_release(&buffer)`.
- `stream_poll_event` transfers its event payload to the caller; call `ibaudio_stream_event_release`.
- Release functions accept null and clear the caller's pointer after success.

## Error handling

Every fallible function returns `ibaudio_status_t`. `ibaudio_error_get_last` copies thread-local detail: status, domain, native code, recoverability, function, and message. Do not retain a pointer to internal error text; the struct owns fixed arrays. No C++ exception crosses the ABI.

## Minimal flow

```c
ibaudio_runtime_options_v1 ro;
ibaudio_runtime_options_init(&ro);
ibaudio_runtime_t *runtime = NULL;
ibaudio_runtime_create(&ro, &runtime);

ibaudio_model_load_options_v1 mo;
ibaudio_model_load_options_init(&mo);
mo.model_id = (ibaudio_string_view_v1){sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION,
                                       "reference-asr-v1", 16};
ibaudio_model_t *model = NULL;
ibaudio_model_load(runtime, &mo, &model);
/* create session, run, release output */
ibaudio_model_release(&model);
ibaudio_runtime_release(&runtime);
```

## Audio contract

`frame_count` counts frames, never scalar samples. `interleaved_f32` contains `frame_count * channels` values. `start_frame` is in the source stream's frame domain. Supported rates are 1,000–384,000 Hz and channels 1–32; model adapters normalize to their descriptor requirements.

## ABI status

Major 1 exports are pinned in `abi/ibaudio_symbols_v1.txt`. `scripts/check_abi.py` compares ELF dynamic exports. The library SONAME is `libibaudio.so.1`; the semantic runtime string is separate. See `ABI_COMPATIBILITY.md` for evolution rules.
