# Audio processing and WAV policy

The public audio unit is a frame. Samples are interleaved float32. Processing order is: validate/bounds → sanitize non-finite values → channel conversion → deterministic linear resampling → gain → optional peak normalization → clipping. `ibaudio_audio_info_v1` reports peaks, applied gain, sanitized and clipped counts.

Channel conversion averages all channels for mono, duplicates mono for multi-channel, preserves existing channels when expanding, and fills new channels with the mean. Linear resampling uses an explicit source position and ceil output length; it is deterministic and suitable for a reference path, not a claim of production-bandlimited quality.

WAV decode is explicit little-endian and bounded. It accepts RIFF/WAVE PCM16, PCM24, and IEEE float32 (including extensible subtype 1/3), validates chunk bounds/block alignment, and rejects truncated/hostile sizes before allocation. WAV encode emits PCM16 RIFF and rejects >4 GiB/RF64 output. MP3/AAC/Opus/FLAC are intentionally outside the native core; decode those at the app/media layer.

Default per-runtime input limit is one hour at 16 kHz and has an absolute overflow guard. Synthetic fixtures cover silence, mono signal, stereo conversion, malformed RIFF/chunks, roundtrip, gain/normalization/clipping, and non-finite sanitization.
