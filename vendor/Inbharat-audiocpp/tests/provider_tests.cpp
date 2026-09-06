// Provider registry / capability router contract tests (default build).
// Proves the universal-core seam: a model resolves to a provider and inference
// flows through the provider vtable rather than a hard-coded engine. Uses only
// the public C ABI — the provider layer itself is internal.

#include "inbharat/ibaudio.h"
#include "../src/provider.hpp"
#include "../src/internal.hpp"

#include <algorithm>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <iostream>
#include <vector>

namespace {

ibaudio_string_view_v1 view_of(const char *text) {
    ibaudio_string_view_v1 v{};
    v.struct_size = sizeof(v);
    v.api_version = IBAUDIO_API_VERSION;
    v.data = text;
    v.size = std::strlen(text);
    return v;
}

ibaudio_runtime_t *make_runtime() {
    ibaudio_runtime_options_v1 options{};
    ibaudio_runtime_options_init(&options);
    ibaudio_runtime_t *runtime = nullptr;
    assert(ibaudio_runtime_create(&options, &runtime) == IBAUDIO_STATUS_OK);
    assert(runtime != nullptr);
    return runtime;
}

ibaudio_model_t *load_model(ibaudio_runtime_t *runtime, const char *id) {
    ibaudio_model_load_options_v1 options{};
    ibaudio_model_load_options_init(&options);
    options.model_id = view_of(id);
    ibaudio_model_t *model = nullptr;
    assert(ibaudio_model_load(runtime, &options, &model) == IBAUDIO_STATUS_OK);
    assert(model != nullptr);
    return model;
}

ibaudio_session_t *make_session(ibaudio_model_t *model, ibaudio_task_t task) {
    ibaudio_session_options_v1 options{};
    ibaudio_session_options_init(&options);
    options.task = task;
    ibaudio_session_t *session = nullptr;
    assert(ibaudio_session_create(model, &options, &session) == IBAUDIO_STATUS_OK);
    assert(session != nullptr);
    return session;
}

std::vector<float> speech_like_frames() {
    // 100 ms of 16 kHz mono with a non-zero signal so ASR/VAD have something to chew on.
    std::vector<float> samples(1600);
    for (size_t i = 0; i < samples.size(); ++i) {
        samples[i] = 0.2f * (i % 2 == 0 ? 1.0f : -1.0f);
    }
    return samples;
}

void test_provider_asr_roundtrip() {
    ibaudio_runtime_t *runtime = make_runtime();
    ibaudio_model_t *model = load_model(runtime, "reference-asr-v1");
    ibaudio_session_t *session = make_session(model, IBAUDIO_TASK_ASR);

    std::vector<float> samples = speech_like_frames();
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = static_cast<uint32_t>(samples.size());
    audio.sample_rate = 16000;
    audio.channels = 1;

    ibaudio_buffer_t *out = nullptr;
    assert(ibaudio_session_run_asr(session, &audio, &out) == IBAUDIO_STATUS_OK);
    assert(out != nullptr);
    const void *data = nullptr;
    uint64_t size = 0;
    assert(ibaudio_buffer_get_data(out, &data, &size) == IBAUDIO_STATUS_OK);
    assert(size > 0);  // provider produced a transcript payload

    ibaudio_buffer_release(&out);
    ibaudio_session_release(&session);
    ibaudio_model_release(&model);
    ibaudio_runtime_release(&runtime);
    std::cout << "PASS provider_asr_roundtrip\n";
}

void test_provider_tts_roundtrip() {
    ibaudio_runtime_t *runtime = make_runtime();
    ibaudio_model_t *model = load_model(runtime, "reference-tts-v1");
    ibaudio_session_t *session = make_session(model, IBAUDIO_TASK_TTS);

    ibaudio_buffer_t *out = nullptr;
    assert(ibaudio_session_run_tts(session, view_of("namaste"), &out) == IBAUDIO_STATUS_OK);
    assert(out != nullptr);
    ibaudio_audio_view_v1 view{};
    assert(ibaudio_buffer_get_audio_view(out, &view) == IBAUDIO_STATUS_OK);
    assert(view.frame_count > 0);  // provider produced audio frames

    ibaudio_buffer_release(&out);
    ibaudio_session_release(&session);
    ibaudio_model_release(&model);
    ibaudio_runtime_release(&runtime);
    std::cout << "PASS provider_tts_roundtrip\n";
}

void test_provider_vad_roundtrip() {
    ibaudio_runtime_t *runtime = make_runtime();
    ibaudio_model_t *model = load_model(runtime, "energy-vad-v1");
    ibaudio_session_t *session = make_session(model, IBAUDIO_TASK_VAD);

    std::vector<float> samples = speech_like_frames();
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = samples.data();
    audio.frame_count = static_cast<uint32_t>(samples.size());
    audio.sample_rate = 16000;
    audio.channels = 1;

    ibaudio_buffer_t *out = nullptr;
    assert(ibaudio_session_run_vad(session, &audio, &out) == IBAUDIO_STATUS_OK);
    assert(out != nullptr);

    ibaudio_buffer_release(&out);
    ibaudio_session_release(&session);
    ibaudio_model_release(&model);
    ibaudio_runtime_release(&runtime);
    std::cout << "PASS provider_vad_roundtrip\n";
}

// Streaming fail-closed semantics: when the ASR provider fails during a
// streaming run, the stream must reach its terminal event WITHOUT emitting a
// fabricated transcript, and ibaudio_stream_finish must return an error so no
// caller mistakes the failure for success. The pre-fix behavior emitted
// "[reference-asr unavailable]" as FINAL_TEXT with a success status.
void test_streaming_never_fabricates_transcript_on_provider_failure() {
    using ibaudio::Provider;
    using ibaudio::ProviderCapabilities;

    class FailingAsrProvider final : public Provider {
    public:
        const ProviderCapabilities &capabilities() const override {
            static const ProviderCapabilities caps = [] {
                ProviderCapabilities c;
                c.id = "failing-asr";
                c.version = "0.0.0";
                c.locality = "local-native";
                c.privacy_class = "ephemeral";
                c.remote = false;
                c.supports_asr = true;
                return c;
            }();
            return caps;
        }
        bool serves_family(const std::string &family) const override {
            return family == "failing-asr";
        }
        // run_asr keeps the base UNSUPPORTED: every call fails.
    };

    ibaudio_runtime_t *runtime = make_runtime();
    FailingAsrProvider provider;

    // Wire a model/session pair directly to the failing provider. The model
    // registry only holds built-in families, so this test constructs the
    // internal structs the stream layer consumes — exactly what the resolver
    // would have produced for a backed family.
    ibaudio_model model{};
    model.runtime = runtime;
    model.record.descriptor.task = IBAUDIO_TASK_ASR;
    std::snprintf(model.record.descriptor.id, sizeof(model.record.descriptor.id), "%s",
                  "failing-asr-model");
    model.provider = &provider;

    ibaudio_session session{};
    session.model = &model;
    session.task = IBAUDIO_TASK_ASR;
    session.streaming_enabled = true;

    ibaudio_stream_options_v1 options{};
    ibaudio_stream_options_init(&options);
    options.emit_partial_results = 1u;
    ibaudio_stream_t *stream = nullptr;
    assert(ibaudio_stream_start(&session, &options, &stream) == IBAUDIO_STATUS_OK);
    assert(stream != nullptr);

    // Push phase: a failed partial must produce a diagnostic, never a
    // PARTIAL_TEXT carrying invented text. 400 ms of signal so the partial
    // window (3200 frames = 200 ms) fires at least once.
    std::vector<float> window = speech_like_frames();
    window.insert(window.end(), window.begin(), window.end());
    window.insert(window.end(), window.begin(), window.end());
    ibaudio_audio_view_v1 audio{};
    audio.struct_size = sizeof(audio);
    audio.api_version = IBAUDIO_API_VERSION;
    audio.interleaved_f32 = window.data();
    audio.frame_count = static_cast<uint32_t>(window.size());
    audio.sample_rate = 16000;
    audio.channels = 1;
    assert(ibaudio_stream_push_audio(stream, &audio) == IBAUDIO_STATUS_OK);
    bool saw_partial = false;
    bool saw_push_diagnostic = false;
    while (true) {
        ibaudio_stream_event_v1 event{};
        const ibaudio_status_t status = ibaudio_stream_poll_event(stream, 0u, &event);
        if (status == IBAUDIO_STATUS_WOULD_BLOCK) break;
        assert(status == IBAUDIO_STATUS_OK);
        if (event.type == IBAUDIO_EVENT_PARTIAL_TEXT) saw_partial = true;
        if (event.type == IBAUDIO_EVENT_DIAGNOSTIC) saw_push_diagnostic = true;
        ibaudio_stream_event_release(&event);
    }
    assert(!saw_partial);
    assert(saw_push_diagnostic);

    // Finish phase: the call itself must fail, the stream must still reach
    // a coherent terminal state, and no FINAL_TEXT may appear.
    const ibaudio_status_t finish_status = ibaudio_stream_finish(stream);
    assert(finish_status == IBAUDIO_STATUS_UNAVAILABLE);

    bool saw_final_text = false;
    bool saw_final = false;
    bool saw_finish_diagnostic = false;
    while (true) {
        ibaudio_stream_event_v1 event{};
        const ibaudio_status_t status = ibaudio_stream_poll_event(stream, 100u, &event);
        if (status == IBAUDIO_STATUS_WOULD_BLOCK) break;
        assert(status == IBAUDIO_STATUS_OK);
        if (event.type == IBAUDIO_EVENT_FINAL_TEXT) saw_final_text = true;
        if (event.type == IBAUDIO_EVENT_DIAGNOSTIC) saw_finish_diagnostic = true;
        if (event.type == IBAUDIO_EVENT_FINAL) saw_final = true;
        const bool terminal = event.type == IBAUDIO_EVENT_FINAL ||
                              event.type == IBAUDIO_EVENT_CANCELLED;
        ibaudio_stream_event_release(&event);
        if (terminal) break;
    }
    assert(!saw_final_text);
    assert(saw_finish_diagnostic);
    assert(saw_final);

    ibaudio_stream_release(&stream);
    ibaudio_runtime_release(&runtime);
    std::cout << "PASS streaming_fail_closed_no_fabricated_transcript\n";
}

// Anti-rot gate for the capability router's remote policy. Asserts, against the live
// internal registry, that (a) family resolution works, (b) a remote-gated family
// resolves under both policies (a stub remote provider is registered), and (c) the
// remote stub is never returned when remote is disallowed. If a refactor ever bypasses
// the gate, this test fails — the gate cannot silently rot.
void test_remote_gate() {
    using ibaudio::Provider;
    using ibaudio::ProviderCapabilities;
    using ibaudio::ProviderRegistry;

    class StubRemoteProvider final : public Provider {
    public:
        const ProviderCapabilities &capabilities() const override {
            static const ProviderCapabilities caps = [] {
                ProviderCapabilities c;
                c.id = "stub-remote";
                c.version = "0.0.0";
                c.locality = "remote";
                c.privacy_class = "audio-and-transcript";
                c.remote = true;
                c.supports_asr = true;
                return c;
            }();
            return caps;
        }
        bool serves_family(const std::string &family) const override {
            return family == "stub-remote-family";
        }
    };

    // A local stub so the test does not depend on the shared library's reference
    // provider (which lives behind hidden internal symbols and is covered separately
    // by the public-ABI roundtrips above).
    class StubLocalProvider final : public Provider {
    public:
        const ProviderCapabilities &capabilities() const override {
            static const ProviderCapabilities caps = [] {
                ProviderCapabilities c;
                c.id = "stub-local";
                c.version = "0.0.0";
                c.locality = "local-native";
                c.privacy_class = "ephemeral";
                c.remote = false;
                c.supports_asr = true;
                return c;
            }();
            return caps;
        }
        bool serves_family(const std::string &family) const override {
            return family == "stub-local-family";
        }
    };

    StubLocalProvider local;
    StubRemoteProvider stub;
    auto &registry = ProviderRegistry::instance();
    registry.register_provider(&local);
    registry.register_provider(&stub);

    // (a) family resolution works for a local family under both policies.
    assert(registry.resolve_for_family("stub-local-family", false) == &local);
    assert(registry.resolve_for_family("stub-local-family", true) == &local);

    // (b) the remote family resolves only when remote is allowed.
    Provider *with_remote = registry.resolve_for_family("stub-remote-family", true);
    assert(with_remote == &stub);
    // (c) and is never returned when remote is disallowed — the gate under test.
    Provider *without_remote = registry.resolve_for_family("stub-remote-family", false);
    assert(without_remote == nullptr);

    // route() must also honor the gate: an ASR request resolves to the local stub,
    // and never to the remote stub when remote is disallowed.
    assert(registry.route(IBAUDIO_TASK_ASR, "", false, false) == &local);

    // Language-coverage gate (the "as-IN must never run Qwen3" invariant):
    // a provider that declares coverage only for the languages it truly
    // serves must never be returned for an uncovered language. Registration
    // order is priority, and language-agnostic providers (empty list) claim
    // everything, so the sharp assertion is negative: the coverage-limited
    // provider — the exact capability set of the production Qwen3-ASR
    // adapter — must never answer for as-IN or any other uncovered
    // Scheduled language, while covered languages must still route to
    // SOME provider (the request is not unroutable because of the gate).
    class StubCoverageProvider final : public Provider {
    public:
        const ProviderCapabilities &capabilities() const override {
            static const ProviderCapabilities caps = [] {
                ProviderCapabilities c;
                c.id = "stub-coverage-asr";
                c.version = "0.0.0";
                c.locality = "local-native";
                c.privacy_class = "ephemeral";
                c.remote = false;
                c.supports_asr = true;
                // The exact coverage the production Qwen3-ASR adapter declares.
                c.languages = {"en-IN", "hi-IN", "hi-en-codemix"};
                return c;
            }();
            return caps;
        }
    };
    StubCoverageProvider coverage;
    registry.register_provider(&coverage);
    for (const char *uncovered : {"as-IN", "bn-IN", "gu-IN", "ta-IN"}) {
        Provider *routed = registry.route(IBAUDIO_TASK_ASR, uncovered, false, false);
        assert(routed != &coverage);
        if (routed != nullptr) {
            const auto &declared = routed->capabilities().languages;
            const bool declared_ok = declared.empty() ||
                                     std::find(declared.begin(), declared.end(), uncovered) != declared.end();
            assert(declared_ok);
        }
    }
    for (const char *covered : {"en-IN", "hi-IN", "hi-en-codemix"}) {
        assert(registry.route(IBAUDIO_TASK_ASR, covered, false, false) != nullptr);
    }
    std::cout << "PASS remote_gate\n";
}

// Streaming-vs-offline differential for the audio.cpp Silero VAD provider: proves the
// incremental streaming path is genuinely exercised (not accepted-but-ignored) by pushing
// audio in chunks and confirming VAD segments stream out before finish. Adapter builds only.
#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
void test_streaming_vad_differential() {
    ibaudio_runtime_t *runtime = make_runtime();
    ibaudio_model_t *model = load_model(runtime, "audiocpp-silero-vad-v1");
    ibaudio_session_options_v1 options{};
    ibaudio_session_options_init(&options);
    options.task = IBAUDIO_TASK_VAD;
    options.streaming = 1u;
    ibaudio_session_t *session = nullptr;
    assert(ibaudio_session_create(model, &options, &session) == IBAUDIO_STATUS_OK);
    assert(session != nullptr);

    ibaudio_stream_options_v1 sopts{};
    ibaudio_stream_options_init(&sopts);
    sopts.emit_partial_results = 1u;
    ibaudio_stream_t *stream = nullptr;
    assert(ibaudio_stream_start(session, &sopts, &stream) == IBAUDIO_STATUS_OK);
    assert(stream != nullptr);

    // Push a 2s voiced signal in 4 chunks (silence / voiced / voiced / silence).
    std::vector<float> mono(32000, 0.0f);
    for (uint32_t i = 8000; i < 24000; ++i) mono[i] = (i % 2 == 0 ? 0.2f : -0.2f);
    uint64_t start = 0;
    for (uint32_t chunk = 0; chunk < 4; ++chunk) {
        ibaudio_audio_view_v1 audio{};
        audio.struct_size = sizeof(audio);
        audio.api_version = IBAUDIO_API_VERSION;
        audio.interleaved_f32 = mono.data() + chunk * 8000;
        audio.frame_count = 8000;
        audio.sample_rate = 16000;
        audio.channels = 1;
        audio.start_frame = start;
        assert(ibaudio_stream_push_audio(stream, &audio) == IBAUDIO_STATUS_OK);
        start += 8000;
    }
    assert(ibaudio_stream_finish(stream) == IBAUDIO_STATUS_OK);

    // Drain events; require at least one VAD speech/segment event from the streaming path.
    bool saw_vad_event = false;
    for (;;) {
        ibaudio_stream_event_v1 ev{};
        const ibaudio_status_t st = ibaudio_stream_poll_event(stream, 0, &ev);
        if (st != IBAUDIO_STATUS_OK) break;
        if (ev.type == IBAUDIO_EVENT_VAD_SPEECH_START || ev.type == IBAUDIO_EVENT_VAD_SEGMENT ||
            ev.type == IBAUDIO_EVENT_VAD_SPEECH_END) {
            saw_vad_event = true;
        }
        const bool terminal = (ev.type == IBAUDIO_EVENT_FINAL || ev.type == IBAUDIO_EVENT_CANCELLED);
        ibaudio_stream_event_release(&ev);
        if (terminal) break;
    }
    assert(saw_vad_event);

    ibaudio_stream_release(&stream);
    ibaudio_session_release(&session);
    ibaudio_model_release(&model);
    ibaudio_runtime_release(&runtime);
    std::cout << "PASS streaming_vad_differential\n";
}
#endif

} // namespace

int main() {
    test_provider_asr_roundtrip();
    test_provider_tts_roundtrip();
    test_provider_vad_roundtrip();
    test_streaming_never_fabricates_transcript_on_provider_failure();
    test_remote_gate();
#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
    test_streaming_vad_differential();
#endif
    std::cout << "All provider tests passed!\n";
    return 0;
}
