#include "inbharat/ibaudio.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

_Static_assert(IBAUDIO_API_VERSION == 0x00010000u, "ABI v1 encoding changed");
_Static_assert(sizeof(ibaudio_status_t) == 4u, "status ABI width changed");
_Static_assert(offsetof(ibaudio_runtime_options_v1, struct_size) == 0u, "struct_size must lead");
_Static_assert(offsetof(ibaudio_runtime_options_v1, api_version) == 4u, "api_version must be second");
_Static_assert(sizeof(ibaudio_vad_segment_v1) == 24u, "VAD segment ABI layout changed");

int main(void) {
    ibaudio_runtime_options_v1 options;
    ibaudio_runtime_t *runtime = NULL;
    ibaudio_capabilities_v1 capabilities;
    uint32_t model_count = 0u;
    ibaudio_model_descriptor_v1 descriptor;
    ibaudio_status_t status;

    memset(&options, 0, sizeof(options));
    ibaudio_runtime_options_init(&options);
    status = ibaudio_runtime_create(&options, &runtime);
    if (status != IBAUDIO_STATUS_OK || runtime == NULL) return 1;
    memset(&capabilities, 0, sizeof(capabilities));
    if (ibaudio_runtime_get_capabilities(runtime, &capabilities) != IBAUDIO_STATUS_OK) return 2;
    if (capabilities.abi_major != 1u || capabilities.abi_minor != 0u) return 3;
    if (ibaudio_runtime_get_model_count(runtime, &model_count) != IBAUDIO_STATUS_OK || model_count != 4u) return 4;
    memset(&descriptor, 0, sizeof(descriptor));
    if (ibaudio_runtime_get_model_descriptor(runtime, 0u, &descriptor) != IBAUDIO_STATUS_OK) return 5;
    if (strcmp(descriptor.id, "reference-asr-v1") != 0) return 6;
    if (ibaudio_runtime_release(&runtime) != IBAUDIO_STATUS_OK || runtime != NULL) return 7;
    if (ibaudio_runtime_release(&runtime) != IBAUDIO_STATUS_OK) return 8;
    return 0;
}
