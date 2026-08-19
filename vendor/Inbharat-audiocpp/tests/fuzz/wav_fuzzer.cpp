#include "inbharat/ibaudio.h"

#include <cstddef>
#include <cstdint>

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
    static ibaudio_runtime_t *runtime = [] {
        ibaudio_runtime_options_v1 options{};
        ibaudio_runtime_options_init(&options);
        options.max_input_frames = 16000u * 60u;
        ibaudio_runtime_t *value = nullptr;
        if (ibaudio_runtime_create(&options, &value) != IBAUDIO_STATUS_OK) return static_cast<ibaudio_runtime_t *>(nullptr);
        return value;
    }();
    if (runtime == nullptr || size > 16u * 1024u * 1024u) return 0;
    ibaudio_buffer_t *audio = nullptr;
    (void)ibaudio_wav_decode_memory(runtime, data, size, &audio);
    (void)ibaudio_buffer_release(&audio);
    return 0;
}
