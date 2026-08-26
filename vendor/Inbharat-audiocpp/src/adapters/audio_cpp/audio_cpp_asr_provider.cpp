// AudioCppAsrProvider — real ASR via pinned audio.cpp's Qwen3-ASR, behind the C ABI.
//
// Compiled ONLY when IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON. The model is supplied by the
// caller through a licensed, integrity-verified path (IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT) —
// nothing is downloaded at runtime, and no inference is faked. If the model root is
// absent or fails to load, run_asr returns UNAVAILABLE.
//
// Qwen3-ASR-0.6B is Apache-2.0 (Hugging Face Qwen/Qwen3-ASR-0.6B), SHA-256 verified
// against the official LFS object id before use. The A1 blocker (no stable upstream C
// ABI, STL/exceptions across the API) is bridged: every upstream call is wrapped and
// translated; nothing upstream crosses the InBharat C ABI.

#ifdef IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER

#include "../../provider.hpp"
#include "../../internal.hpp"

#include "engine/models/qwen3_asr/loader.h"

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
            // India-pack languages it serves. Language list is the model's coverage.
            c.languages = {"en-IN", "hi-IN"};
            c.supports_asr = true;
            c.supports_tts = false;
            c.supports_vad = false;
            c.supports_kws = false;
            c.streaming_asr = true;   // Qwen3-ASR has a true streaming session
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
                    model_ = engine::models::qwen3_asr::load_qwen3_asr_model(model_root_);
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
