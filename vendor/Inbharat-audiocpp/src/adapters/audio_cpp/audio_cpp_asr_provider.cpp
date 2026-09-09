// AudioCppAsrProvider — real ASR via pinned audio.cpp's Qwen3-ASR, behind the C ABI.
//
// Compiled ONLY when IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON. The model is supplied by the
// caller through a licensed path (IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT) — nothing is
// downloaded at runtime, and no inference is faked. If the model root is absent or
// fails to load, run_asr returns UNAVAILABLE.
//
// Integrity, honestly scoped (finding V6): the readiness probe verifies structure
// (single .gguf, readable, non-empty, GGUF magic). No digest is pinned next to the
// shipped weights, so this adapter cannot pin one itself; the shipped digest is
// enforced by the caller-side chain (package-manifest background validation + the
// acceptance attestation). When a deployment DOES pin a digest, setting
// IBAUDIO_AUDIO_CPP_QWEN3_ASR_SHA256 enables a load-time content-hash gate here:
// a mismatch fails closed with INTEGRITY_ERROR and the weights are never loaded.
// The bundled Silero VAD weights are covered by the upstream pin + pristine check.
//
// Qwen3-ASR-0.6B is Apache-2.0 (Hugging Face Qwen/Qwen3-ASR-0.6B). The A1 blocker
// (no stable upstream C ABI, STL/exceptions across the API) is bridged: every upstream
// call is wrapped and translated; nothing upstream crosses the InBharat C ABI.

#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER

#include "../../provider.hpp"
#include "../../internal.hpp"

#include "audio_cpp_probe.hpp"

#include "engine/models/qwen3_asr/loader.h"

#include <cstdio>
#include <cstdlib>
#include <memory>
#include <mutex>

namespace ibaudio {
namespace {

class AudioCppAsrProvider final : public Provider {
public:
    const ProviderCapabilities &capabilities() const override {
        static const ProviderCapabilities caps = [] {
            ProviderCapabilities c;
            c.id = "audiocpp-asr";
            c.version = "release-0.6.1";
            c.locality = "local-native";
            c.privacy_class = "ephemeral";
            c.remote = false;
            // Qwen3-ASR-0.6B covers 30+ languages incl. hi; en-IN/hi-IN are the InBharat
            // India-pack languages it serves, plus the hi-en codemix alias the
            // product routes to it. Assamese and the other 19 Scheduled
            // languages are NOT here on purpose: route() must never return this
            // provider for them (the IndicConformer seam owns those).
            c.languages = {"en-IN", "hi-IN", "hi-en-codemix"};
            c.supports_asr = true;
            c.supports_tts = false;
            c.supports_vad = false;
            c.supports_kws = false;
            // Honest capability (finding V8): Qwen3-ASR has an upstream
            // streaming session, but this adapter implements only the
            // offline run_asr path — the core stream layer feeds it windowed
            // offline partials. Claiming streaming_asr would promise a
            // provider streaming path that does not exist behind this
            // vtable; the router's require_streaming filter must not see
            // one either.
            c.streaming_asr = false;
            c.streaming_tts = false;
            c.streaming_vad = false;
            return c;
        }();
        return caps;
    }

    bool serves_family(const std::string &family) const override {
        return family == "audiocpp-qwen3-asr";
    }

    ibaudio_status_t run_asr(const AudioData &mono_audio,
                             const CancellationToken *cancel,
                             uint64_t *processed_frames,
                             std::string &out_text) override {
        out_text.clear();
        if (mono_audio.channels != 1) return IBAUDIO_STATUS_INVALID_ARGUMENT;
        if (cancel != nullptr && cancel->requested.load(std::memory_order_relaxed)) {
            return IBAUDIO_STATUS_CANCELLED;
        }
        try {
            engine::models::qwen3_asr::Qwen3ASRLoadedModel *model = nullptr;
            {
                std::lock_guard<std::mutex> lock(model_mutex_);
                if (model_ == nullptr) {
                    if (model_root_.empty()) return IBAUDIO_STATUS_UNAVAILABLE;
                    // Negative caching (finding V11): a failed load used to be
                    // retried in full on EVERY inference call under this
                    // mutex — a missing-weights deployment paid a heavy
                    // load attempt per utterance. Fail fast while the
                    // readiness probe still fails; the expensive load is
                    // retried only after the probe passes again, i.e. when
                    // the weights were genuinely replaced or mounted.
                    if (model_load_failed_) {
                        char probe_reason[192];
                        if (!ibaudio::audio_cpp_adapter::probe_model_root(
                                model_root_.c_str(), "audio.cpp ASR", ".gguf", probe_reason,
                                sizeof(probe_reason))) {
                            return IBAUDIO_STATUS_UNAVAILABLE;
                        }
                        model_load_failed_ = false;
                    }
                    // Optional content-hash gate (finding V6): when the
                    // deployment pins a digest for the baked-root weights,
                    // verify it before the first load. Without a pin there is
                    // nothing to verify against — the shipped digest is
                    // enforced caller-side (package manifest + acceptance
                    // attestation), never claimed here.
                    const char *expected_sha256 =
                        std::getenv("IBAUDIO_AUDIO_CPP_QWEN3_ASR_SHA256");
                    if (expected_sha256 != nullptr && expected_sha256[0] != '\0') {
                        char hash_reason[192];
                        if (!ibaudio::audio_cpp_adapter::verify_model_root_sha256(
                                model_root_.c_str(), "audio.cpp ASR", ".gguf", nullptr,
                                expected_sha256, hash_reason, sizeof(hash_reason))) {
                            fprintf(stderr,
                                    "[audiocpp-asr] weights hash gate failed: %s\n",
                                    hash_reason);
                            return IBAUDIO_STATUS_INTEGRITY_ERROR;
                        }
                    }
                    model_ = engine::models::qwen3_asr::load_qwen3_asr_model(model_root_);
                    if (model_ == nullptr) {
                        model_load_failed_ = true;
                        fprintf(stderr, "[audiocpp-asr] model load failed from root: %s\n",
                                model_root_.c_str());
                    }
                }
                model = model_.get();
            }
            if (model == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;

            engine::runtime::TaskSpec task;
            task.task = engine::runtime::VoiceTaskKind::Asr;
            task.mode = engine::runtime::RunMode::Offline;
            engine::runtime::SessionOptions options{};
            auto base_session = model->create_task_session(task, options);
            if (base_session == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;
            auto *session = dynamic_cast<engine::runtime::IOfflineVoiceTaskSession *>(base_session.get());
            if (session == nullptr) return IBAUDIO_STATUS_UNAVAILABLE;

            // prepare() before run(), declaring the audio contract.
            engine::runtime::SessionPreparationRequest prep;
            engine::runtime::AudioPreparationContract contract;
            contract.sample_rate = static_cast<int>(mono_audio.sample_rate);
            contract.channels = 1;
            contract.max_input_samples = static_cast<int64_t>(mono_audio.samples.size());
            prep.audio = contract;
            base_session->prepare(prep);

            engine::runtime::TaskRequest request;
            engine::runtime::AudioBuffer audio;
            audio.sample_rate = static_cast<int>(mono_audio.sample_rate);
            audio.channels = 1;
            audio.samples = mono_audio.samples;
            request.audio_input = std::move(audio);

            if (cancel != nullptr && cancel->requested.load(std::memory_order_relaxed)) {
                return IBAUDIO_STATUS_CANCELLED;
            }
            engine::runtime::TaskResult result = session->run(request);
            if (processed_frames != nullptr) *processed_frames = mono_audio.samples.size();
            if (result.text_output.has_value()) out_text = result.text_output->text;
            return IBAUDIO_STATUS_OK;
        } catch (const std::exception &ex) {
            fprintf(stderr, "[audiocpp-asr] upstream exception: %s\n", ex.what());
            return IBAUDIO_STATUS_INTERNAL_ERROR;  // upstream exception contained at the boundary
        } catch (...) {
            fprintf(stderr, "[audiocpp-asr] unknown upstream exception\n");
            return IBAUDIO_STATUS_INTERNAL_ERROR;
        }
    }

private:
    std::string model_root_ = IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT;
    std::shared_ptr<engine::models::qwen3_asr::Qwen3ASRLoadedModel> model_;
    bool model_load_failed_ = false;  // set after a failed load; see run_asr
    std::mutex model_mutex_;
};

AudioCppAsrProvider g_audiocpp_asr_provider;

struct AudioCppAsrRegistration {
    AudioCppAsrRegistration() {
        ProviderRegistry::instance().register_provider(&g_audiocpp_asr_provider);
    }
};
AudioCppAsrRegistration g_audiocpp_asr_registration;

} // namespace
} // namespace ibaudio

#endif // IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER
