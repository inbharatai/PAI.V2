#include "internal.hpp"

/*
 * Optional audio.cpp integration boundary.
 *
 * This compilation unit deliberately contains no copied audio.cpp source and
 * includes no upstream header. CMake enables it only after verifying that the
 * separately supplied checkout is clean and exactly pinned. Model-family
 * adapters remain unavailable until their source closure, licenses, weights,
 * parity, cancellation points, and Android memory profile pass review.
 */

namespace ibaudio::audio_cpp_adapter {

constexpr const char *kReviewedCommit = "bb15edd78b56e035967e0eb999a6b28a62337db4";

const char *reviewed_commit() noexcept {
    return kReviewedCommit;
}

ibaudio_status_t availability() noexcept {
    return IBAUDIO_STATUS_DEFERRED;
}

} // namespace ibaudio::audio_cpp_adapter
