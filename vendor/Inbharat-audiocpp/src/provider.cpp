#include "provider.hpp"

#include <algorithm>

namespace ibaudio {

ProviderRegistry &ProviderRegistry::instance() {
    static ProviderRegistry registry;
    return registry;
}

void ProviderRegistry::register_provider(Provider *provider) {
    if (provider == nullptr) return;
    if (std::find(providers_.begin(), providers_.end(), provider) == providers_.end()) {
        providers_.push_back(provider);
    }
}

Provider *ProviderRegistry::find(const std::string &id) const {
    for (Provider *provider : providers_) {
        if (provider->capabilities().id == id) return provider;
    }
    return nullptr;
}

std::vector<Provider *> ProviderRegistry::all() const {
    return providers_;
}

Provider *ProviderRegistry::route(ibaudio_task_t task,
                                  const std::string &language,
                                  bool require_streaming,
                                  bool remote_allowed) const {
    for (Provider *provider : providers_) {
        const ProviderCapabilities &caps = provider->capabilities();

        // Hard policy gate: a remote provider is never a candidate when the
        // deployment disallows remote. No silent cloud fallback.
        if (caps.remote && !remote_allowed) continue;

        const bool task_ok =
            (task == IBAUDIO_TASK_ASR && caps.supports_asr) ||
            (task == IBAUDIO_TASK_TTS && caps.supports_tts) ||
            (task == IBAUDIO_TASK_VAD && caps.supports_vad) ||
            (task == IBAUDIO_TASK_KWS && caps.supports_kws);
        if (!task_ok) continue;

        if (require_streaming) {
            const bool stream_ok =
                (task == IBAUDIO_TASK_ASR && caps.streaming_asr) ||
                (task == IBAUDIO_TASK_TTS && caps.streaming_tts);
            if (!stream_ok) continue;
        }

        // Language constraint: an empty request means "provider's choice"; an empty
        // provider list means "language-agnostic". Otherwise require evidence of the
        // requested language.
        if (!language.empty() && !caps.languages.empty() &&
            std::find(caps.languages.begin(), caps.languages.end(), language) == caps.languages.end()) {
            continue;
        }

        return provider;  // registration order encodes priority
    }
    return nullptr;
}

Provider *ProviderRegistry::resolve_for_family(const std::string &family, bool remote_allowed) const {
    for (Provider *provider : providers_) {
        // Hard policy gate first: a remote provider is never a candidate when the
        // deployment disallows remote, even if it backs the requested family.
        if (provider->capabilities().remote && !remote_allowed) continue;
        if (provider->serves_family(family)) return provider;
    }
    return nullptr;
}

} // namespace ibaudio
