#include "inbharat/ibaudio.h"

#include <jni.h>

#include <cstdint>
#include <limits>
#include <new>
#include <string>
#include <vector>

namespace {

template <typename T>
T *handle(jlong value) {
    return reinterpret_cast<T *>(static_cast<uintptr_t>(value));
}

template <typename T>
jlong to_handle(T *value) {
    return static_cast<jlong>(reinterpret_cast<uintptr_t>(value));
}

void throw_java(JNIEnv *env, const char *class_name, const char *message) noexcept {
    if (env->ExceptionCheck()) return;
    jclass exception_class = env->FindClass(class_name);
    if (exception_class != nullptr) env->ThrowNew(exception_class, message);
}

void append_utf8(std::string &output, uint32_t scalar) {
    if (scalar <= 0x7fu) {
        output.push_back(static_cast<char>(scalar));
    } else if (scalar <= 0x7ffu) {
        output.push_back(static_cast<char>(0xc0u | (scalar >> 6u)));
        output.push_back(static_cast<char>(0x80u | (scalar & 0x3fu)));
    } else if (scalar <= 0xffffu) {
        output.push_back(static_cast<char>(0xe0u | (scalar >> 12u)));
        output.push_back(static_cast<char>(0x80u | ((scalar >> 6u) & 0x3fu)));
        output.push_back(static_cast<char>(0x80u | (scalar & 0x3fu)));
    } else {
        output.push_back(static_cast<char>(0xf0u | (scalar >> 18u)));
        output.push_back(static_cast<char>(0x80u | ((scalar >> 12u) & 0x3fu)));
        output.push_back(static_cast<char>(0x80u | ((scalar >> 6u) & 0x3fu)));
        output.push_back(static_cast<char>(0x80u | (scalar & 0x3fu)));
    }
}

std::string utf8(JNIEnv *env, jstring value) noexcept {
    if (value == nullptr) return {};
    const jsize length = env->GetStringLength(value);
    const jchar *characters = env->GetStringChars(value, nullptr);
    if (characters == nullptr) return {};
    try {
        std::string result;
        result.reserve(static_cast<size_t>(length) * 3u);
        for (jsize index = 0; index < length; ++index) {
            uint32_t scalar = characters[index];
            if (scalar >= 0xd800u && scalar <= 0xdbffu) {
                if (index + 1 < length) {
                    const uint32_t low = characters[index + 1];
                    if (low >= 0xdc00u && low <= 0xdfffu) {
                        scalar = 0x10000u + ((scalar - 0xd800u) << 10u) + (low - 0xdc00u);
                        ++index;
                    } else {
                        scalar = 0xfffdu;
                    }
                } else {
                    scalar = 0xfffdu;
                }
            } else if (scalar >= 0xdc00u && scalar <= 0xdfffu) {
                scalar = 0xfffdu;
            }
            append_utf8(result, scalar);
        }
        env->ReleaseStringChars(value, characters);
        return result;
    } catch (const std::bad_alloc &) {
        env->ReleaseStringChars(value, characters);
        throw_java(env, "java/lang/OutOfMemoryError", "unable to allocate UTF-8 conversion buffer");
        return {};
    } catch (...) {
        env->ReleaseStringChars(value, characters);
        throw_java(env, "java/lang/IllegalStateException", "native UTF-8 conversion failed");
        return {};
    }
}

jstring java_string_from_utf8(JNIEnv *env, const char *data, size_t size) noexcept {
    if (data == nullptr && size != 0u) {
        throw_java(env, "java/lang/IllegalArgumentException", "UTF-8 data pointer is null");
        return nullptr;
    }
    try {
        std::vector<jchar> utf16;
        utf16.reserve(size);
        size_t index = 0u;
        while (index < size) {
            const uint8_t first = static_cast<uint8_t>(data[index]);
            uint32_t scalar = 0xfffdu;
            size_t width = 1u;
            if (first < 0x80u) {
                scalar = first;
            } else if ((first & 0xe0u) == 0xc0u && index + 1u < size) {
                const uint8_t second = static_cast<uint8_t>(data[index + 1u]);
                if ((second & 0xc0u) == 0x80u) {
                    const uint32_t candidate = ((first & 0x1fu) << 6u) | (second & 0x3fu);
                    if (candidate >= 0x80u) {
                        scalar = candidate;
                        width = 2u;
                    }
                }
            } else if ((first & 0xf0u) == 0xe0u && index + 2u < size) {
                const uint8_t second = static_cast<uint8_t>(data[index + 1u]);
                const uint8_t third = static_cast<uint8_t>(data[index + 2u]);
                if ((second & 0xc0u) == 0x80u && (third & 0xc0u) == 0x80u) {
                    const uint32_t candidate = ((first & 0x0fu) << 12u) |
                        ((second & 0x3fu) << 6u) | (third & 0x3fu);
                    if (candidate >= 0x800u && !(candidate >= 0xd800u && candidate <= 0xdfffu)) {
                        scalar = candidate;
                        width = 3u;
                    }
                }
            } else if ((first & 0xf8u) == 0xf0u && index + 3u < size) {
                const uint8_t second = static_cast<uint8_t>(data[index + 1u]);
                const uint8_t third = static_cast<uint8_t>(data[index + 2u]);
                const uint8_t fourth = static_cast<uint8_t>(data[index + 3u]);
                if ((second & 0xc0u) == 0x80u && (third & 0xc0u) == 0x80u &&
                    (fourth & 0xc0u) == 0x80u) {
                    const uint32_t candidate = ((first & 0x07u) << 18u) |
                        ((second & 0x3fu) << 12u) | ((third & 0x3fu) << 6u) |
                        (fourth & 0x3fu);
                    if (candidate >= 0x10000u && candidate <= 0x10ffffu) {
                        scalar = candidate;
                        width = 4u;
                    }
                }
            }
            index += width;
            if (scalar <= 0xffffu) {
                utf16.push_back(static_cast<jchar>(scalar));
            } else {
                scalar -= 0x10000u;
                utf16.push_back(static_cast<jchar>(0xd800u + (scalar >> 10u)));
                utf16.push_back(static_cast<jchar>(0xdc00u + (scalar & 0x3ffu)));
            }
        }
        if (utf16.size() > static_cast<size_t>(std::numeric_limits<jsize>::max())) {
            throw_java(env, "java/lang/OutOfMemoryError", "UTF-16 output exceeds Java string limit");
            return nullptr;
        }
        return env->NewString(utf16.empty() ? nullptr : utf16.data(),
                              static_cast<jsize>(utf16.size()));
    } catch (const std::bad_alloc &) {
        throw_java(env, "java/lang/OutOfMemoryError", "unable to allocate UTF-16 conversion buffer");
        return nullptr;
    } catch (...) {
        throw_java(env, "java/lang/IllegalStateException", "native UTF-16 conversion failed");
        return nullptr;
    }
}

ibaudio_string_view_v1 string_view(const std::string &value) {
    return {sizeof(ibaudio_string_view_v1), IBAUDIO_API_VERSION, value.data(), value.size()};
}

void throw_status(JNIEnv *env, ibaudio_status_t status) noexcept {
    if (status == IBAUDIO_STATUS_OK || env->ExceptionCheck()) return;
    ibaudio_error_info_v1 error{};
    ibaudio_error_get_last(&error);
    const char *class_name = (status == IBAUDIO_STATUS_INVALID_ARGUMENT)
        ? "java/lang/IllegalArgumentException"
        : "java/lang/IllegalStateException";
    try {
        const std::string message = std::string(ibaudio_status_string(status)) + ": " + error.message;
        throw_java(env, class_name, message.c_str());
    } catch (...) {
        throw_java(env, class_name, ibaudio_status_string(status));
    }
}

std::vector<float> copy_pcm(JNIEnv *env, jfloatArray pcm, jint channels) noexcept {
    if (pcm == nullptr || channels <= 0 || channels > 32) {
        throw_java(env, "java/lang/IllegalArgumentException", "PCM and channels in [1, 32] are required");
        return {};
    }
    const jsize count = env->GetArrayLength(pcm);
    constexpr jsize max_samples = 64 * 1024 * 1024;
    if (count < 0 || count > max_samples || count % channels != 0) {
        throw_java(env, "java/lang/IllegalArgumentException",
                   "PCM length must be bounded and divisible by channels");
        return {};
    }
    try {
        std::vector<float> result(static_cast<size_t>(count));
        if (count > 0) env->GetFloatArrayRegion(pcm, 0, count, result.data());
        if (env->ExceptionCheck()) return {};
        return result;
    } catch (const std::bad_alloc &) {
        throw_java(env, "java/lang/OutOfMemoryError", "unable to allocate PCM copy buffer");
        return {};
    } catch (...) {
        throw_java(env, "java/lang/IllegalStateException", "native PCM copy failed");
        return {};
    }
}

ibaudio_audio_view_v1 audio_view(
    const std::vector<float> &pcm,
    jint sample_rate,
    jint channels) {
    ibaudio_audio_view_v1 view{};
    view.struct_size = sizeof(view);
    view.api_version = IBAUDIO_API_VERSION;
    view.interleaved_f32 = pcm.data();
    view.channels = static_cast<uint32_t>(channels);
    view.sample_rate = static_cast<uint32_t>(sample_rate);
    view.frame_count = channels > 0 ? pcm.size() / static_cast<uint32_t>(channels) : 0u;
    return view;
}

} // namespace

extern "C" JNIEXPORT jint JNICALL
Java_org_inbharat_audio_NativeBridge_apiVersion(JNIEnv *, jobject) {
    return static_cast<jint>(ibaudio_get_api_version());
}

extern "C" JNIEXPORT jlong JNICALL
Java_org_inbharat_audio_NativeBridge_createRuntime(
    JNIEnv *env,
    jobject,
    jstring cache_directory,
    jstring model_root,
    jint threads,
    jboolean vulkan_requested) {
    const std::string cache = utf8(env, cache_directory);
    const std::string root = utf8(env, model_root);
    if (env->ExceptionCheck()) return 0;
    ibaudio_runtime_options_v1 options{};
    ibaudio_runtime_options_init(&options);
    options.cache_directory = string_view(cache);
    options.allowed_model_root = string_view(root);
    options.cpu_threads = static_cast<uint32_t>(threads);
    options.requested_backend = vulkan_requested ? IBAUDIO_BACKEND_VULKAN : IBAUDIO_BACKEND_CPU;
    options.allow_auto_cpu_fallback = 1u;
    ibaudio_runtime_t *runtime = nullptr;
    const ibaudio_status_t status = ibaudio_runtime_create(&options, &runtime);
    throw_status(env, status);
    return status == IBAUDIO_STATUS_OK ? to_handle(runtime) : 0;
}

extern "C" JNIEXPORT void JNICALL
Java_org_inbharat_audio_NativeBridge_releaseRuntime(JNIEnv *env, jobject, jlong value) {
    ibaudio_runtime_t *runtime = handle<ibaudio_runtime_t>(value);
    throw_status(env, ibaudio_runtime_release(&runtime));
}

extern "C" JNIEXPORT jstring JNICALL
Java_org_inbharat_audio_NativeBridge_diagnostics(JNIEnv *env, jobject, jlong value) {
    ibaudio_buffer_t *buffer = nullptr;
    const ibaudio_status_t status = ibaudio_runtime_get_diagnostics_json(handle<ibaudio_runtime_t>(value), &buffer);
    if (status != IBAUDIO_STATUS_OK) {
        throw_status(env, status);
        return nullptr;
    }
    const void *data = nullptr;
    uint64_t size = 0u;
    ibaudio_buffer_get_data(buffer, &data, &size);
    jstring result = java_string_from_utf8(
        env, static_cast<const char *>(data), static_cast<size_t>(size));
    ibaudio_buffer_release(&buffer);
    return result;
}

extern "C" JNIEXPORT jlong JNICALL
Java_org_inbharat_audio_NativeBridge_loadModel(JNIEnv *env, jobject, jlong runtime_value, jstring model_id) {
    const std::string id = utf8(env, model_id);
    ibaudio_model_load_options_v1 options{};
    ibaudio_model_load_options_init(&options);
    options.model_id = string_view(id);
    ibaudio_model_t *model = nullptr;
    const ibaudio_status_t status = ibaudio_model_load(handle<ibaudio_runtime_t>(runtime_value), &options, &model);
    throw_status(env, status);
    return status == IBAUDIO_STATUS_OK ? to_handle(model) : 0;
}

extern "C" JNIEXPORT void JNICALL
Java_org_inbharat_audio_NativeBridge_releaseModel(JNIEnv *env, jobject, jlong value) {
    ibaudio_model_t *model = handle<ibaudio_model_t>(value);
    throw_status(env, ibaudio_model_release(&model));
}

extern "C" JNIEXPORT jlong JNICALL
Java_org_inbharat_audio_NativeBridge_createSession(
    JNIEnv *env,
    jobject,
    jlong model_value,
    jint task,
    jboolean streaming,
    jfloat threshold) {
    ibaudio_session_options_v1 options{};
    ibaudio_session_options_init(&options);
    options.task = static_cast<ibaudio_task_t>(task);
    options.streaming = streaming ? 1u : 0u;
    options.vad_threshold_dbfs = threshold;
    ibaudio_session_t *session = nullptr;
    const ibaudio_status_t status = ibaudio_session_create(handle<ibaudio_model_t>(model_value), &options, &session);
    throw_status(env, status);
    return status == IBAUDIO_STATUS_OK ? to_handle(session) : 0;
}

extern "C" JNIEXPORT void JNICALL
Java_org_inbharat_audio_NativeBridge_releaseSession(JNIEnv *env, jobject, jlong value) {
    ibaudio_session_t *session = handle<ibaudio_session_t>(value);
    throw_status(env, ibaudio_session_release(&session));
}

extern "C" JNIEXPORT jstring JNICALL
Java_org_inbharat_audio_NativeBridge_runAsr(
    JNIEnv *env,
    jobject,
    jlong session_value,
    jfloatArray pcm_array,
    jint sample_rate,
    jint channels) {
    std::vector<float> pcm = copy_pcm(env, pcm_array, channels);
    if (env->ExceptionCheck()) return nullptr;
    ibaudio_audio_view_v1 view = audio_view(pcm, sample_rate, channels);
    ibaudio_buffer_t *result = nullptr;
    const ibaudio_status_t status = ibaudio_session_run_asr(handle<ibaudio_session_t>(session_value), &view, &result);
    if (status != IBAUDIO_STATUS_OK) {
        throw_status(env, status);
        return nullptr;
    }
    const void *data = nullptr;
    uint64_t size = 0u;
    ibaudio_buffer_get_data(result, &data, &size);
    jstring text = java_string_from_utf8(
        env, static_cast<const char *>(data), static_cast<size_t>(size));
    ibaudio_buffer_release(&result);
    return text;
}

extern "C" JNIEXPORT jfloatArray JNICALL
Java_org_inbharat_audio_NativeBridge_runTts(
    JNIEnv *env,
    jobject,
    jlong session_value,
    jstring input_text) {
    const std::string text = utf8(env, input_text);
    ibaudio_buffer_t *result = nullptr;
    const ibaudio_status_t status = ibaudio_session_run_tts(
        handle<ibaudio_session_t>(session_value), string_view(text), &result);
    if (status != IBAUDIO_STATUS_OK) {
        throw_status(env, status);
        return nullptr;
    }
    ibaudio_audio_view_v1 audio{};
    ibaudio_buffer_get_audio_view(result, &audio);
    if (audio.frame_count > static_cast<uint64_t>(std::numeric_limits<jsize>::max())) {
        ibaudio_buffer_release(&result);
        jclass exception_class = env->FindClass("java/lang/OutOfMemoryError");
        if (exception_class != nullptr) env->ThrowNew(exception_class, "TTS output exceeds Java array limit");
        return nullptr;
    }
    jfloatArray output = env->NewFloatArray(static_cast<jsize>(audio.frame_count));
    if (output != nullptr && audio.frame_count > 0u) {
        env->SetFloatArrayRegion(output, 0, static_cast<jsize>(audio.frame_count), audio.interleaved_f32);
    }
    ibaudio_buffer_release(&result);
    return output;
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_org_inbharat_audio_NativeBridge_runVad(
    JNIEnv *env,
    jobject,
    jlong session_value,
    jfloatArray pcm_array,
    jint sample_rate,
    jint channels) {
    std::vector<float> pcm = copy_pcm(env, pcm_array, channels);
    if (env->ExceptionCheck()) return nullptr;
    ibaudio_audio_view_v1 view = audio_view(pcm, sample_rate, channels);
    ibaudio_buffer_t *result = nullptr;
    const ibaudio_status_t status = ibaudio_session_run_vad(handle<ibaudio_session_t>(session_value), &view, &result);
    if (status != IBAUDIO_STATUS_OK) {
        throw_status(env, status);
        return nullptr;
    }
    const void *data = nullptr;
    uint64_t size = 0u;
    ibaudio_buffer_get_data(result, &data, &size);
    const auto *segments = static_cast<const ibaudio_vad_segment_v1 *>(data);
    const size_t count = static_cast<size_t>(size / sizeof(ibaudio_vad_segment_v1));
    if (count > static_cast<size_t>(std::numeric_limits<jsize>::max() / 2)) {
        ibaudio_buffer_release(&result);
        jclass exception_class = env->FindClass("java/lang/OutOfMemoryError");
        if (exception_class != nullptr) env->ThrowNew(exception_class, "VAD result exceeds Java array limit");
        return nullptr;
    }
    try {
        std::vector<jlong> packed(count * 2u);
        for (size_t index = 0; index < count; ++index) {
            packed[index * 2u] = static_cast<jlong>(segments[index].start_frame);
            packed[index * 2u + 1u] = static_cast<jlong>(segments[index].end_frame);
        }
        jlongArray output = env->NewLongArray(static_cast<jsize>(packed.size()));
        if (output != nullptr && !packed.empty()) {
            env->SetLongArrayRegion(output, 0, static_cast<jsize>(packed.size()), packed.data());
        }
        ibaudio_buffer_release(&result);
        return output;
    } catch (const std::bad_alloc &) {
        ibaudio_buffer_release(&result);
        throw_java(env, "java/lang/OutOfMemoryError", "unable to allocate VAD result array");
        return nullptr;
    } catch (...) {
        ibaudio_buffer_release(&result);
        throw_java(env, "java/lang/IllegalStateException", "native VAD conversion failed");
        return nullptr;
    }
}

extern "C" JNIEXPORT void JNICALL
Java_org_inbharat_audio_NativeBridge_setPlaybackActive(
    JNIEnv *env,
    jobject,
    jlong session_value,
    jboolean active) {
    throw_status(env, ibaudio_session_set_playback_active(
        handle<ibaudio_session_t>(session_value), active ? 1u : 0u));
}

extern "C" JNIEXPORT jint JNICALL
Java_org_inbharat_audio_NativeBridge_reportInputLevel(
    JNIEnv *env,
    jobject,
    jlong session_value,
    jfloat rms_dbfs,
    jint duration_ms) {
    if (duration_ms < 0) {
        throw_java(env, "java/lang/IllegalArgumentException", "durationMs must be non-negative");
        return static_cast<jint>(IBAUDIO_BARGE_IN_IDLE);
    }
    ibaudio_barge_in_state_t state = IBAUDIO_BARGE_IN_IDLE;
    uint32_t interrupt = 0u;
    const ibaudio_status_t status = ibaudio_session_report_input_level(
        handle<ibaudio_session_t>(session_value), rms_dbfs, static_cast<uint32_t>(duration_ms), &state, &interrupt);
    throw_status(env, status);
    return static_cast<jint>(state);
}

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *, void *) {
    return JNI_VERSION_1_6;
}
