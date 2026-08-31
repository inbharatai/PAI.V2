#ifndef INBHARAT_IBAUDIO_SHERPA_ONNX_INTERNAL_HPP
#define INBHARAT_IBAUDIO_SHERPA_ONNX_INTERNAL_HPP

// Internal seam surface for the sherpa-onnx IndicConformer provider.
// Nothing here crosses the C ABI; the provider is compiled only when
// IBAUDIO_ENABLE_SHERPA_ONNX=ON.

#include <cstddef>

namespace ibaudio::sherpa_onnx {

// Probe one local IndicConformer ONNX pack root for usable model assets.
// Same fail-closed semantics as the audio.cpp probe: an unconfigured root,
// a missing directory, or a directory without .onnx weights reports false
// with the exact reason. Success means "assets are present and readable",
// NEVER "inference is wired" — the native sherpa-onnx runtime is not linked
// in this seam.
bool probe_indicconformer_root(const char *root,
                               char *reason,
                               std::size_t reason_size);

// Probe the configured pack root (compile-time macro in the library build).
bool indicconformer_assets_usable(char *reason, std::size_t reason_size);

} // namespace ibaudio::sherpa_onnx

#endif // INBHARAT_IBAUDIO_SHERPA_ONNX_INTERNAL_HPP