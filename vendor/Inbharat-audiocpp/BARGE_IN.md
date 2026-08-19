# Cancellation, interruption, and barge-in

Asynchronous jobs copy their request, transition queued → running → succeeded/cancelled/failed, and expose monotonic timestamps and processed units. `cancel` is idempotent. Poll points exist in audio sanitization/resampling, VAD windows, ASR analysis, and TTS character/frame loops; wait settles only after worker quiescence. Release always joins.

Cancellation is cooperative: the reference engines have short bounded spans, but a future backend call may be non-preemptible. Such an adapter must document its maximum span or use an isolated process/service for hard deadlines. Setting an atomic flag around unmodified blocking inference is not sufficient.

Barge-in is a small state machine: `IDLE` → `OUTPUT_ACTIVE` → `SPEECH_CANDIDATE` → `INTERRUPTED`. While playback is active, sustained input at/above `barge_in_threshold_dbfs` for `barge_in_hold_ms` returns `should_interrupt=1` and cancels the active job. Falling below threshold resets the candidate timer. The caller remains responsible for stopping the audio sink and draining/releasing handles.
