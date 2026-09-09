#include "internal.hpp"

/*
 * Optional audio.cpp integration boundary.
 *
 * This compilation unit deliberately contains no copied audio.cpp source and
 * includes no upstream header. CMake enables it only after verifying that the
 * separately supplied checkout is clean and exactly pinned. Model-family
 * adapters remain unavailable until their source closure, licenses, weights,
 * parity, cancellation points, and Android memory profile pass review.
 *
 * Readiness lives in audio_cpp_probe.cpp: the adapter reports the facts
 * (reviewed commit, usable local assets) and the runtime status API derives
 * inference_ready from them. The old availability() stub — which returned
 * DEFERRED unconditionally while the runtime simultaneously reported READY
 * from compile-time truth — is gone; two contradictory answers were worse
 * than one honest probe.
 */

namespace ibaudio::audio_cpp_adapter {

constexpr const char *kReviewedCommit = "26dcb5c4cf5aa016ae6285096a7b45f2671e5d17";

const char *reviewed_commit() noexcept {
    return kReviewedCommit;
}

} // namespace ibaudio::audio_cpp_adapter