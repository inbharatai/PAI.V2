# Streaming semantics

Streaming labels are classes, not a misleading boolean:

- `OFFLINE_ONLY`
- `BUFFERED_FINAL`
- `WINDOW_INCREMENTAL_REVISABLE`
- `STATEFUL_LOW_LATENCY`
- `SEGMENT_CHUNKED`
- `DEFERRED`

Reference ASR accepts contiguous source frames, incrementally resamples to mono 16 kHz, emits revisable analysis every 3,200 canonical frames, and emits an authoritative final transcript at finish. It is window-incremental, not token-recurrent linguistic ASR.

Energy VAD preserves speech/silence runs and emits start/end/segment events after configured hysteresis. TTS emits PCM segment chunks but generation currently occurs inside stream start; use an asynchronous job for cancellable generation. Its label is segment-chunked, not real-time synthesis.

`start_frame` continuity is checked in the source frame domain. A rate/channel change or gap requires `IBAUDIO_AUDIO_FLAG_DISCONTINUITY`, which resets analysis state and emits a diagnostic. `END_OF_INPUT` flushes available resampling but callers still call `stream_finish` for final events.

Polling is pull-based. `WOULD_BLOCK` means no immediate event; timed waits return `TIMEOUT`. After the terminal event is consumed, polling returns `INVALID_STATE`. Queue pressure drops stale provisional/diagnostic events first and coalesces contiguous TTS audio after the configured soft limit. Terminal and VAD boundary events are preserved up to an absolute 4,096-event safety ceiling; reaching that ceiling clears queued payloads, releases the session slot, and emits a terminal cancellation instead of allowing unbounded memory growth.
