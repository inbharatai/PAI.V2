// AudioCppProvider — wraps pinned audio.cpp (release-0.6) as an InBharat provider.
//
// Compiled ONLY when IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON and a pristine pinned
// checkout is supplied (the configure-time pin gate enforces commit + cleanliness).
// The default build excludes this translation unit entirely, so the dependency-free
// core never links ggml/sentencepiece/etc.
//
// What is REAL here vs gated, honestly:
//  - VAD is real neural inference via audio.cpp's bundled Silero VAD safetensors
//    (no download). run_vad loads the model once and runs SileroVADSession.
//  - ASR and TTS return UNAVAILABLE: audio.cpp has no bundled ASR/TTS weights, and no
//    licensed ASR/TTS model is vendored into this tree. They are gated behind a
//    caller-supplied model path (see the provider plan) — never faked.
//
// A1-blocker bridging: audio.cpp exposes STL/exceptions across its API. Every call
// here is wrapped and translated; nothing upstream crosses the InBharat C ABI.

#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER

#include "../../provider.hpp"
#include "../../internal.hpp"

#include "audio_cpp_probe.hpp"

#include "engine/models/silero_vad/session.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <memory>
#include <mutex>

namespace ibaudio {
namespace {

// Applies the caller's duration tuning to the neural VAD output (finding
// V12): the facade validates VadConfig and used to hand it to this provider,
// which dropped it on the floor. min_speech_ms / min_silence_ms have a
// faithful meaning here — drop sub-minimum blips, merge segments split by
// less than the caller's minimum silence. threshold_dbfs, frame_ms and
// hop_ms remain inapplicable by design: Silero is a probability model over
// fixed 512-sample windows, not an energy detector over caller-sized frames,
// so pretending to honor an energy threshold would fabricate behavior.
void apply_vad_config_post_processing(const VadConfig &config,
                                      std::vector<ibaudio_vad_segment_v1> &segments) {
    if (segments.empty()) return;
    const uint64_t min_speech_frames = static_cast<uint64_t>(config.min_speech_ms) * 16u;
    const uint64_t min_silence_frames = static_cast<uint64_t>(config.min_silence_ms) * 16u;
    // Segment spans are absolute 16 kHz sample indices; 1 ms = 16 samples.
    std::vector<ibaudio_vad_segment_v1> merged;
    for (const auto &segment : segments) {
        if (!merged.empty() && segment.start_frame >= merged.back().end_frame &&
            segment.start_frame - merged.back().end_frame < min_silence_frames) {
            merged.back().end_frame = segment.end_frame;
            merged.back().confidence = std::max(merged.back().confidence, segment.confidence);
        } else {
            merged.push_back(segment);
        }
    }
    segments.clear();
    for (const auto &segment : merged) {
        if (segment.end_frame - segment.start_frame >= min_speech_frames) {
            segments.push_back(segment);
        }
    }
}

// Lazily-loaded shared handle to the bundled Silero VAD model. Loaded once, reused
// across calls (audio.cpp sessions are heavy to spin up; the weights are read-only).
class SileroVadProvider final : public Provider {
public:
    const ProviderCapabilities &capabilities() const override {
        static const ProviderCapabilities caps = [] {
            ProviderCapabilities c;
            c.id = "audiocpp";
            c.version = "release-0.6";
            c.locality = "local-native";
            c.privacy_class = "ephemeral";
            c.remote = false;
            c.languages = {};  // Silero VAD is language-agnostic
            c.supports_asr = false;  // gated: needs a licensed ASR model path
            c.supports_tts = false;  // gated: needs a licensed TTS model path
            c.supports_vad = true;   // bundled Silero VAD — real neural inference
            c.supports_kws = false;
            c.streaming_vad = true;  // Silero VAD has a true streaming path
            c.streaming_asr = false;
            c.streaming_tts = false;
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        return family == "audiocpp-silero-vad";
    }

    ibaudio_status_t run_vad(const AudioData &mono_audio,
                             const VadConfig &config,
                             const CancellationToken *cancel,
                             uint64_t *processed_frames,
                             std::vector<ibaudio_vad_segment_v1> &out_segments) override {
        // Silero owns its detection model (fixed 512-sample windows,
        // probability output); the caller's VadConfig is applied where it
        // has faithful meaning — see apply_vad_config_post_processing.
        // threshold_dbfs/frame_ms/hop_ms are energy-VAD tunables with no
        // neural equivalent and are deliberately not honored.
        out_segments.clear();
        if (mono_audio.sample_rate != 16000 || mono_audio.channels != 1) {
            return IBAUDIO_STATUS_INVALID_ARGUMENT;  // Silero VAD 16k expects mono 16 kHz
        }
        if (cancel != nullptr && cancel->requested.load(std::memory_order_relaxed)) {
            return IBAUDIO_STATUS_CANCELLED;
        }
        try {
            engine::models::silero_vad::SileroVADLoadedModel *model = ensure_model_loaded();
            if (model == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;

            engine::runtime::TaskSpec task;
            task.task = engine::runtime::VoiceTaskKind::Vad;
            task.mode = engine::runtime::RunMode::Offline;
            engine::runtime::SessionOptions options{};
            auto base_session = model->create_task_session(task, options);
            if (base_session == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;
            // run() lives on the offline-session interface, not the base IVoiceTaskSession.
            auto *session = dynamic_cast<engine::runtime::IOfflineVoiceTaskSession *>(base_session.get());
            if (session == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;

            // audio.cpp requires prepare() before run(): declare the audio contract.
            engine::runtime::SessionPreparationRequest prep;
            engine::runtime::AudioPreparationContract contract;
            contract.sample_rate = 16000;
            contract.channels = 1;
            contract.max_input_samples = static_cast<int64_t>(mono_audio.samples.size());
            prep.audio = contract;
            base_session->prepare(prep);

            engine::runtime::TaskRequest request;
            engine::runtime::AudioBuffer audio;
            audio.sample_rate = 16000;
            audio.channels = 1;
            audio.samples = mono_audio.samples;
            request.audio_input = std::move(audio);

            // Cancellation is checked before the (bounded) inference call; audio.cpp's
            // offline VAD runs a fixed-size graph over the input and returns.
            if (cancel != nullptr && cancel->requested.load(std::memory_order_relaxed)) {
                return IBAUDIO_STATUS_CANCELLED;
            }
            engine::runtime::TaskResult result = session->run(request);
            if (processed_frames != nullptr) *processed_frames = mono_audio.samples.size();

            // Map audio.cpp speech segments (sample-domain spans) to InBharat VAD segments.
            for (const auto &seg : result.speech_segments) {
                ibaudio_vad_segment_v1 out{};
                out.start_frame = static_cast<uint64_t>(seg.span.start_sample < 0 ? 0 : seg.span.start_sample);
                out.end_frame = static_cast<uint64_t>(seg.span.end_sample < 0 ? 0 : seg.span.end_sample);
                out.confidence = seg.confidence;
                out.peak_dbfs = 0.0f;  // not provided by audio.cpp VAD; left neutral, not invented
                out_segments.push_back(out);
            }
            apply_vad_config_post_processing(config, out_segments);
            return IBAUDIO_STATUS_OK;
        } catch (const std::exception &) {
            return IBAUDIO_STATUS_INTERNAL_ERROR;  // upstream exception contained at the boundary
        } catch (...) {
            return IBAUDIO_STATUS_INTERNAL_ERROR;
        }
    }

    // --- Streaming VAD (Silero's true incremental path) -------------------------
    // Silero VAD at 16 kHz consumes exactly 512-sample windows. The stream
    // layer may push any chunk size, so push() buffers incoming samples and
    // feeds the model complete, contiguous windows only; a partial window at
    // the end of the audio is dropped (it cannot be classified, and padding
    // it with zeros would invent audio).
    static constexpr size_t kSileroWindowSamples = 512u;

    struct VadStreamState {
        std::unique_ptr<engine::runtime::IVoiceTaskSession> base;
        engine::runtime::IStreamingVoiceTaskSession *streaming = nullptr;  // non-owning
        uint64_t start_sample = 0;  // absolute sample index of the next sample to be processed
        std::vector<float> pending;  // sub-window leftovers held between pushes
    };

    ibaudio_status_t vad_stream_create(void **out_state) override {
        if (out_state == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
        *out_state = nullptr;
        try {
            engine::models::silero_vad::SileroVADLoadedModel *model = ensure_model_loaded();
            if (model == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;
            engine::runtime::TaskSpec task;
            task.task = engine::runtime::VoiceTaskKind::Vad;
            task.mode = engine::runtime::RunMode::Streaming;
            engine::runtime::SessionOptions options{};
            auto state = std::make_unique<VadStreamState>();
            state->base = model->create_task_session(task, options);
            if (state->base == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;
            state->streaming = dynamic_cast<engine::runtime::IStreamingVoiceTaskSession *>(state->base.get());
            if (state->streaming == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;
            // audio.cpp requires prepare() before use — the same contract the
            // offline path declares. reset() on an unprepared session was a
            // latent ordering bug: the session had no audio contract yet.
            engine::runtime::SessionPreparationRequest prep;
            engine::runtime::AudioPreparationContract contract;
            contract.sample_rate = 16000;
            contract.channels = 1;
            contract.max_input_samples = static_cast<int64_t>(kSileroWindowSamples);
            prep.audio = contract;
            state->base->prepare(prep);
            state->streaming->reset();
            *out_state = state.release();
            return IBAUDIO_STATUS_OK;
        } catch (...) {
            return IBAUDIO_STATUS_INTERNAL_ERROR;
        }
    }

    ibaudio_status_t vad_stream_push(void *state,
                                     const float *samples,
                                     uint64_t frame_count,
                                     uint32_t sample_rate,
                                     std::vector<ibaudio_vad_segment_v1> &out_segments) override {
        out_segments.clear();
        auto *s = static_cast<VadStreamState *>(state);
        if (s == nullptr || s->streaming == nullptr || samples == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
        if (sample_rate != 16000) return IBAUDIO_STATUS_INVALID_ARGUMENT;
        try {
            s->pending.insert(s->pending.end(), samples, samples + frame_count);
            while (s->pending.size() >= kSileroWindowSamples) {
                engine::runtime::AudioChunk chunk;
                chunk.sample_rate = 16000;
                chunk.channels = 1;
                chunk.start_sample = static_cast<int64_t>(s->start_sample);
                chunk.samples.assign(s->pending.begin(),
                                     s->pending.begin() + static_cast<std::ptrdiff_t>(kSileroWindowSamples));
                s->pending.erase(s->pending.begin(),
                                 s->pending.begin() + static_cast<std::ptrdiff_t>(kSileroWindowSamples));
                s->start_sample += kSileroWindowSamples;
                engine::runtime::StreamEvent event = s->streaming->process_audio_chunk(chunk);
                for (const auto &va : event.voice_activity) {
                    if (va.segment.has_value()) {
                        ibaudio_vad_segment_v1 out{};
                        out.start_frame = static_cast<uint64_t>(va.segment->span.start_sample < 0 ? 0 : va.segment->span.start_sample);
                        out.end_frame = static_cast<uint64_t>(va.segment->span.end_sample < 0 ? 0 : va.segment->span.end_sample);
                        out.confidence = va.segment->confidence;
                        out.peak_dbfs = 0.0f;
                        out_segments.push_back(out);
                    }
                }
            }
            return IBAUDIO_STATUS_OK;
        } catch (...) {
            return IBAUDIO_STATUS_INTERNAL_ERROR;
        }
    }

    ibaudio_status_t vad_stream_finish(void *state,
                                       std::vector<ibaudio_vad_segment_v1> &out_segments) override {
        out_segments.clear();
        auto *s = static_cast<VadStreamState *>(state);
        if (s == nullptr || s->streaming == nullptr) return IBAUDIO_STATUS_INVALID_ARGUMENT;
        try {
            engine::runtime::TaskResult result = s->streaming->finalize();
            for (const auto &seg : result.speech_segments) {
                ibaudio_vad_segment_v1 out{};
                out.start_frame = static_cast<uint64_t>(seg.span.start_sample < 0 ? 0 : seg.span.start_sample);
                out.end_frame = static_cast<uint64_t>(seg.span.end_sample < 0 ? 0 : seg.span.end_sample);
                out.confidence = seg.confidence;
                out.peak_dbfs = 0.0f;
                out_segments.push_back(out);
            }
            return IBAUDIO_STATUS_OK;
        } catch (...) {
            return IBAUDIO_STATUS_INTERNAL_ERROR;
        }
    }

    void vad_stream_destroy(void *state) override {
        delete static_cast<VadStreamState *>(state);
    }

private:
    // Lazily loads the bundled Silero model under the mutex, with negative
    // caching (finding V11): a failed load used to be retried in full on
    // EVERY inference call — a missing-weights deployment paid a heavy load
    // attempt per utterance. While the readiness probe still fails, calls
    // fail fast; the expensive load is retried only after the probe passes
    // again, i.e. when the weights were genuinely replaced. Returns the
    // loaded model or nullptr (UNAVAILABLE at the call sites).
    engine::models::silero_vad::SileroVADLoadedModel *ensure_model_loaded() {
        std::lock_guard<std::mutex> lock(model_mutex_);
        if (model_ == nullptr) {
            if (model_load_failed_) {
                char probe_reason[192];
                if (!ibaudio::audio_cpp_adapter::probe_model_root(
                        model_root_.c_str(), "audio.cpp VAD", nullptr, probe_reason,
                        sizeof(probe_reason), "silero_vad_16k.safetensors")) {
                    return nullptr;
                }
                model_load_failed_ = false;
            }
            engine::runtime::ModelLoadRequest request;
            request.model_path = model_root_;
            request.family_hint = std::string("silero_vad");
            model_ = engine::models::silero_vad::load_silero_vad_model(request);
            if (model_ == nullptr) {
                model_load_failed_ = true;
                fprintf(stderr, "[audiocpp-vad] model load failed from root: %s\n",
                        model_root_.c_str());
            }
        }
        return model_.get();
    }

    // The bundled Silero VAD model root inside the pristine pinned checkout. Resolved
    // relative to the adapter's source location at configure time (see CMake).
    std::string model_root_ = IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT;
    std::shared_ptr<engine::models::silero_vad::SileroVADLoadedModel> model_;
    bool model_load_failed_ = false;  // set after a failed load; see ensure_model_loaded
    std::mutex model_mutex_;
};

SileroVadProvider g_silero_vad_provider;

struct AudioCppProviderRegistration {
    AudioCppProviderRegistration() {
        ProviderRegistry::instance().register_provider(&g_silero_vad_provider);
    }
};
AudioCppProviderRegistration g_audiocpp_registration;

} // namespace
} // namespace ibaudio

#endif // IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
