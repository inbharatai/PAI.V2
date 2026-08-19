#include "../crates/ffi/include/inbharat_harness.h"
#include <stdio.h>
#include <string.h>

static IbByteSpanV1 span(const char *value) {
    IbByteSpanV1 result = {
        .struct_size = (uint32_t)sizeof(IbByteSpanV1),
        .data = (const uint8_t *)value,
        .len = strlen(value),
    };
    return result;
}

int main(void) {
    IbHarnessConfigV1 config = {
        .struct_size = (uint32_t)sizeof(IbHarnessConfigV1),
        .abi_version = IB_HARNESS_ABI_VERSION,
        .root = span("."),
        .maximum_level = 3,
        .reserved = {0},
    };
    IbHarnessHandle *harness = NULL;
    int32_t status = ib_harness_create_v1(&config, &harness);
    if (status != IB_STATUS_OK) return status;

    uint8_t level = 255;
    status = ib_harness_route_v1(harness, span("hello"), -1, &level);
    if (status == IB_STATUS_OK) printf("level=L%u\n", (unsigned)level);

    IbCancellationHandle *cancel = NULL;
    if (status == IB_STATUS_OK)
        status = ib_harness_cancel_create_v1(&cancel);

    IbOwnedBytesV1 output = {
        .struct_size = (uint32_t)sizeof(IbOwnedBytesV1),
        .data = NULL,
        .len = 0,
    };
    if (status == IB_STATUS_OK)
        status = ib_harness_run_with_cancel_v1(
            harness, span("hello"), 0, cancel, &output);
    if (status == IB_STATUS_OK)
        printf("%.*s\n", (int)output.len, (const char *)output.data);

    ib_harness_bytes_free_v1(&output);
    ib_harness_cancel_destroy_v1(cancel);
    ib_harness_destroy_v1(harness);
    return status;
}
