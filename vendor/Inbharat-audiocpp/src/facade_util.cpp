#include "internal.hpp"

#include <algorithm>
#include <cctype>
#include <cstring>
#include <fstream>
#include <limits>

namespace {

ibaudio_buffer *new_audio_buffer(ibaudio_runtime *runtime, ibaudio::AudioData data) {
    auto buffer = std::make_unique<ibaudio_buffer>();
    buffer->kind = IBAUDIO_BUFFER_AUDIO_F32;
    buffer->metrics = runtime == nullptr ? nullptr : &runtime->metrics;
    buffer->audio = std::move(data);
    if (buffer->metrics != nullptr) {
        buffer->metrics->live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
    }
    return buffer.release();
}

bool is_descendant(const std::filesystem::path &path, const std::filesystem::path &root) {
    const auto canonical_path = std::filesystem::weakly_canonical(path);
    const auto canonical_root = std::filesystem::weakly_canonical(root);
    auto path_it = canonical_path.begin();
    auto root_it = canonical_root.begin();
    for (; root_it != canonical_root.end(); ++root_it, ++path_it) {
        if (path_it == canonical_path.end() || *path_it != *root_it) return false;
    }
    return true;
}

bool valid_sha256_hex(const std::string &value) {
    return value.size() == 64u && std::all_of(value.begin(), value.end(), [](unsigned char c) {
        return std::isxdigit(c) != 0;
    });
}

std::string lower_ascii(std::string value) {
    std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
        return static_cast<char>(std::tolower(c));
    });
    return value;
}

ibaudio_status_t authorize_external_path(
    const ibaudio_runtime *runtime,
    const std::filesystem::path &path,
    const char *function_name) {
    if (!runtime->strict_path_policy) return IBAUDIO_STATUS_OK;
    if (runtime->allowed_model_root.empty()) {
        return ibaudio::set_error(IBAUDIO_STATUS_SECURITY_ERROR, IBAUDIO_ERROR_DOMAIN_SECURITY,
                                  function_name, "strict path policy requires an explicit allowed_model_root");
    }
    if (!is_descendant(path, runtime->allowed_model_root)) {
        return ibaudio::set_error(IBAUDIO_STATUS_SECURITY_ERROR, IBAUDIO_ERROR_DOMAIN_SECURITY,
                                  function_name, "path escapes allowed_model_root");
    }
    return IBAUDIO_STATUS_OK;
}

} // namespace

extern "C" {

ibaudio_status_t ibaudio_model_load(
    ibaudio_runtime_t *runtime,
    const ibaudio_model_load_options_v1 *options,
    ibaudio_model_t **out_model) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || options == nullptr || out_model == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime, options, and output model are required");
        }
        *out_model = nullptr;
        if (!ibaudio::valid_header(options->struct_size, sizeof(*options), options->api_version)) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid model-load options header");
        }
        const std::string model_id = ibaudio::from_view(options->model_id);
        if (model_id.empty()) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "model_id must not be empty");
        }
        const auto record_it = std::find_if(runtime->models.begin(), runtime->models.end(),
            [&](const ibaudio::ModelRecord &record) { return model_id == record.descriptor.id; });
        if (record_it == runtime->models.end()) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "model id is not registered");
        }
        if (record_it->descriptor.available == 0u) {
            return ibaudio::set_error(IBAUDIO_STATUS_DEFERRED, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, record_it->descriptor.availability_reason, true);
        }
        if (options->backend < IBAUDIO_BACKEND_AUTO || options->backend > IBAUDIO_BACKEND_DIRECTML) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid backend enum");
        }
        const ibaudio_backend_t requested_backend = options->backend == IBAUDIO_BACKEND_AUTO
            ? runtime->selected_backend : options->backend;
        if (requested_backend != IBAUDIO_BACKEND_CPU) {
            return ibaudio::set_error(IBAUDIO_STATUS_UNAVAILABLE, IBAUDIO_ERROR_DOMAIN_BACKEND,
                                      __func__, "explicit accelerator model load is unavailable; create/load a CPU session instead", true);
        }

        std::string artifact_path = ibaudio::from_view(options->artifact_path);
        std::string expected_hash = lower_ascii(ibaudio::from_view(options->expected_sha256));
        std::string verified_hash = record_it->descriptor.artifact_sha256;
        uint64_t artifact_size = 0u;
        if (!artifact_path.empty()) {
            const std::filesystem::path path(artifact_path);
            const ibaudio_status_t path_status = authorize_external_path(runtime, path, __func__);
            if (path_status != IBAUDIO_STATUS_OK) return path_status;
            if (!std::filesystem::is_regular_file(path)) {
                return ibaudio::set_error(IBAUDIO_STATUS_IO_ERROR, IBAUDIO_ERROR_DOMAIN_IO,
                                          __func__, "artifact path is not a regular file", true);
            }
            artifact_size = std::filesystem::file_size(path);
            if (artifact_size > ibaudio::kMaxOwnedBytes * 8u) {
                return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_MODEL,
                                          __func__, "artifact exceeds 4 GiB local RC policy limit");
            }
            if (options->verify_hash != 0u && !valid_sha256_hex(expected_hash)) {
                return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_MODEL,
                                          __func__, "a 64-character expected SHA-256 is required for external artifacts");
            }
            verified_hash = ibaudio::sha256_file_path(path);
            if (options->verify_hash != 0u && verified_hash != expected_hash) {
                return ibaudio::set_error(IBAUDIO_STATUS_INTEGRITY_ERROR, IBAUDIO_ERROR_DOMAIN_SECURITY,
                                          __func__, "artifact SHA-256 mismatch");
            }
            artifact_path = std::filesystem::weakly_canonical(path).string();
        } else if (!expected_hash.empty()) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_MODEL,
                                      __func__, "expected hash supplied without artifact path");
        }

        auto model = std::make_unique<ibaudio_model>();
        model->runtime = runtime;
        model->record = *record_it;
        model->artifact_path = artifact_path;
        model->verified_hash = verified_hash;
        model->backend = IBAUDIO_BACKEND_CPU;
        if (!artifact_path.empty()) {
            model->record.descriptor.artifact_size_bytes = artifact_size;
            ibaudio::copy_text(model->record.descriptor.artifact_sha256,
                sizeof(model->record.descriptor.artifact_sha256), verified_hash);
            ibaudio::copy_text(model->record.descriptor.hash_kind,
                sizeof(model->record.descriptor.hash_kind), "artifact-sha256");
            ibaudio::copy_text(model->record.descriptor.source_uri,
                sizeof(model->record.descriptor.source_uri), std::string("file://") + artifact_path);
        }
        const std::string cache_key = model_id + ":" + verified_hash;
        {
            std::lock_guard<std::mutex> lock(runtime->cache_mutex);
            if (runtime->cache_keys.count(cache_key) != 0u) {
                runtime->metrics.model_cache_hits.fetch_add(1u, std::memory_order_relaxed);
                runtime->cache_lru.erase(std::remove(runtime->cache_lru.begin(), runtime->cache_lru.end(), cache_key),
                                         runtime->cache_lru.end());
            } else {
                runtime->metrics.model_cache_misses.fetch_add(1u, std::memory_order_relaxed);
                if (runtime->max_cached_models > 0u) {
                    runtime->cache_keys.insert(cache_key);
                }
            }
            if (runtime->max_cached_models > 0u) {
                runtime->cache_lru.push_back(cache_key);
                while (runtime->cache_lru.size() > runtime->max_cached_models) {
                    runtime->cache_keys.erase(runtime->cache_lru.front());
                    runtime->cache_lru.pop_front();
                }
            }
        }
        runtime->live_models.fetch_add(1u, std::memory_order_release);
        runtime->metrics.models_loaded.fetch_add(1u, std::memory_order_relaxed);
        *out_model = model.release();
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_model_get_descriptor(
    const ibaudio_model_t *model,
    ibaudio_model_descriptor_v1 *out_descriptor) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (model == nullptr || out_descriptor == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "model or output descriptor is null");
        }
        *out_descriptor = model->record.descriptor;
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_model_release(ibaudio_model_t **model) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (model == nullptr || *model == nullptr) return IBAUDIO_STATUS_OK;
        if ((*model)->live_sessions.load(std::memory_order_acquire) != 0u) {
            return ibaudio::set_error(IBAUDIO_STATUS_BUSY, IBAUDIO_ERROR_DOMAIN_LIFECYCLE,
                                      __func__, "model still owns live sessions", true);
        }
        ibaudio_runtime *runtime = (*model)->runtime;
        delete *model;
        *model = nullptr;
        if (runtime != nullptr) runtime->live_models.fetch_sub(1u, std::memory_order_release);
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_audio_process(
    ibaudio_runtime_t *runtime,
    const ibaudio_audio_view_v1 *input,
    const ibaudio_audio_process_options_v1 *options,
    ibaudio_buffer_t **out_audio) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || input == nullptr || options == nullptr || out_audio == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime, input, options, and output are required");
        }
        *out_audio = nullptr;
        if (!ibaudio::valid_header(input->struct_size, sizeof(*input), input->api_version) ||
            !ibaudio::valid_header(options->struct_size, sizeof(*options), options->api_version)) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "invalid audio or processing-options header");
        }
        ibaudio::AudioData processed = ibaudio::process_audio(*input, *options, runtime->max_input_frames);
        runtime->metrics.audio_frames_in.fetch_add(input->frame_count, std::memory_order_relaxed);
        runtime->metrics.audio_frames_out.fetch_add(processed.info.frame_count, std::memory_order_relaxed);
        *out_audio = new_audio_buffer(runtime, std::move(processed));
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_wav_decode_memory(
    ibaudio_runtime_t *runtime,
    const void *bytes,
    uint64_t byte_count,
    ibaudio_buffer_t **out_audio) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_audio == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime and output pointer are required");
        }
        *out_audio = nullptr;
        ibaudio::AudioData decoded = ibaudio::wav_decode(bytes, byte_count, runtime->max_input_frames);
        runtime->metrics.audio_frames_in.fetch_add(decoded.info.frame_count, std::memory_order_relaxed);
        *out_audio = new_audio_buffer(runtime, std::move(decoded));
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_wav_encode_pcm16(
    ibaudio_runtime_t *runtime,
    const ibaudio_audio_view_v1 *audio,
    ibaudio_buffer_t **out_wav) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || audio == nullptr || out_wav == nullptr ||
            !ibaudio::valid_header(audio->struct_size, sizeof(*audio), audio->api_version)) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime, valid audio, and output pointer are required");
        }
        *out_wav = nullptr;
        std::vector<uint8_t> wav = ibaudio::wav_encode_pcm16(*audio, runtime->max_input_frames);
        auto buffer = std::make_unique<ibaudio_buffer>();
        buffer->kind = IBAUDIO_BUFFER_BYTES;
        buffer->metrics = &runtime->metrics;
        buffer->bytes = std::move(wav);
        runtime->metrics.live_owned_buffers.fetch_add(1u, std::memory_order_relaxed);
        *out_wav = buffer.release();
        return IBAUDIO_STATUS_OK;
    });
}

ibaudio_status_t ibaudio_buffer_get_kind(
    const ibaudio_buffer_t *buffer,
    ibaudio_buffer_kind_t *out_kind) {
    if (buffer == nullptr || out_kind == nullptr) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  __func__, "buffer or output kind is null");
    }
    *out_kind = buffer->kind;
    ibaudio::clear_error_internal();
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_buffer_get_data(
    const ibaudio_buffer_t *buffer,
    const void **out_data,
    uint64_t *out_size_bytes) {
    if (buffer == nullptr || out_data == nullptr || out_size_bytes == nullptr) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  __func__, "buffer and output pointers are required");
    }
    switch (buffer->kind) {
        case IBAUDIO_BUFFER_BYTES:
            *out_data = buffer->bytes.empty() ? nullptr : buffer->bytes.data();
            *out_size_bytes = buffer->bytes.size();
            break;
        case IBAUDIO_BUFFER_UTF8:
            *out_data = buffer->bytes.empty() ? nullptr : buffer->bytes.data();
            *out_size_bytes = buffer->bytes.empty() ? 0u : buffer->bytes.size() - 1u;
            break;
        case IBAUDIO_BUFFER_AUDIO_F32:
            *out_data = buffer->audio.samples.empty() ? nullptr : buffer->audio.samples.data();
            *out_size_bytes = static_cast<uint64_t>(buffer->audio.samples.size()) * sizeof(float);
            break;
        case IBAUDIO_BUFFER_VAD_SEGMENTS:
            *out_data = buffer->segments.empty() ? nullptr : buffer->segments.data();
            *out_size_bytes = static_cast<uint64_t>(buffer->segments.size()) * sizeof(ibaudio_vad_segment_v1);
            break;
        default:
            return ibaudio::set_error(IBAUDIO_STATUS_INTERNAL_ERROR, IBAUDIO_ERROR_DOMAIN_INTERNAL,
                                      __func__, "unknown buffer kind");
    }
    ibaudio::clear_error_internal();
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_buffer_get_audio_view(
    const ibaudio_buffer_t *buffer,
    ibaudio_audio_view_v1 *out_audio) {
    if (buffer == nullptr || out_audio == nullptr || buffer->kind != IBAUDIO_BUFFER_AUDIO_F32) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  __func__, "buffer is null, not audio, or output is null");
    }
    *out_audio = {};
    out_audio->struct_size = sizeof(*out_audio);
    out_audio->api_version = IBAUDIO_API_VERSION;
    out_audio->interleaved_f32 = buffer->audio.samples.empty() ? nullptr : buffer->audio.samples.data();
    out_audio->channels = buffer->audio.channels;
    out_audio->sample_rate = buffer->audio.sample_rate;
    out_audio->frame_count = buffer->audio.channels == 0u ? 0u : buffer->audio.samples.size() / buffer->audio.channels;
    ibaudio::clear_error_internal();
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_buffer_get_audio_info(
    const ibaudio_buffer_t *buffer,
    ibaudio_audio_info_v1 *out_info) {
    if (buffer == nullptr || out_info == nullptr || buffer->kind != IBAUDIO_BUFFER_AUDIO_F32) {
        return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                  __func__, "buffer is null, not audio, or output is null");
    }
    *out_info = buffer->audio.info;
    out_info->struct_size = sizeof(*out_info);
    out_info->api_version = IBAUDIO_API_VERSION;
    ibaudio::clear_error_internal();
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_buffer_release(ibaudio_buffer_t **buffer) {
    if (buffer == nullptr || *buffer == nullptr) return IBAUDIO_STATUS_OK;
    ibaudio::MetricsData *metrics = (*buffer)->metrics;
    delete *buffer;
    *buffer = nullptr;
    if (metrics != nullptr) metrics->live_owned_buffers.fetch_sub(1u, std::memory_order_relaxed);
    ibaudio::clear_error_internal();
    return IBAUDIO_STATUS_OK;
}

ibaudio_status_t ibaudio_sha256_file(
    ibaudio_runtime_t *runtime,
    ibaudio_string_view_v1 path,
    char out_hex[65]) {
    return ibaudio::guarded(__func__, [&]() -> ibaudio_status_t {
        if (runtime == nullptr || out_hex == nullptr) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "runtime and hash output are required");
        }
        const std::string path_text = ibaudio::from_view(path);
        if (path_text.empty()) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_ARGUMENT,
                                      __func__, "path must not be empty");
        }
        const std::filesystem::path file_path(path_text);
        const ibaudio_status_t path_status = authorize_external_path(runtime, file_path, __func__);
        if (path_status != IBAUDIO_STATUS_OK) return path_status;
        if (!std::filesystem::is_regular_file(file_path)) {
            return ibaudio::set_error(IBAUDIO_STATUS_IO_ERROR, IBAUDIO_ERROR_DOMAIN_IO,
                                      __func__, "path is not a regular file", true);
        }
        if (std::filesystem::file_size(file_path) > ibaudio::kMaxOwnedBytes * 8u) {
            return ibaudio::set_error(IBAUDIO_STATUS_INVALID_ARGUMENT, IBAUDIO_ERROR_DOMAIN_IO,
                                      __func__, "file exceeds the 4 GiB hashing policy limit");
        }
        ibaudio::copy_text(out_hex, 65u, ibaudio::sha256_file_path(file_path));
        return IBAUDIO_STATUS_OK;
    });
}

} /* extern "C" */
