#ifndef INBHARAT_HARNESS_H
#define INBHARAT_HARNESS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define IB_HARNESS_ABI_VERSION 1u
#define IB_STATUS_OK 0
#define IB_STATUS_INVALID_ARGUMENT 1
#define IB_STATUS_DENIED 2
#define IB_STATUS_UNAVAILABLE 3
#define IB_STATUS_CANCELLED 4
#define IB_STATUS_RESOURCE_EXHAUSTED 5
#define IB_STATUS_OPERATION_FAILED 6
#define IB_STATUS_PANIC 255

#define IB_CANCEL_USER 0
#define IB_CANCEL_PARENT 1
#define IB_CANCEL_DEADLINE 2
#define IB_CANCEL_POLICY 3
#define IB_CANCEL_SHUTDOWN 4
#define IB_CANCEL_DISPOSED 5

typedef struct IbHarnessHandle IbHarnessHandle;
typedef struct IbCancellationHandle IbCancellationHandle;

typedef struct IbByteSpanV1 {
    uint32_t struct_size;
    const uint8_t *data;
    size_t len;
} IbByteSpanV1;

typedef struct IbOwnedBytesV1 {
    uint32_t struct_size;
    uint8_t *data;
    size_t len;
} IbOwnedBytesV1;

typedef struct IbHarnessConfigV1 {
    uint32_t struct_size;
    uint32_t abi_version;
    IbByteSpanV1 root;
    uint8_t maximum_level;
    uint8_t reserved[7];
} IbHarnessConfigV1;

uint32_t ib_harness_api_version_v1(void);
int32_t ib_harness_create_v1(const IbHarnessConfigV1 *config, IbHarnessHandle **out_handle);
int32_t ib_harness_destroy_v1(IbHarnessHandle *handle);
int32_t ib_harness_cancel_create_v1(IbCancellationHandle **out_handle);
int32_t ib_harness_cancel_request_v1(IbCancellationHandle *handle, uint8_t cause);
int32_t ib_harness_cancel_destroy_v1(IbCancellationHandle *handle);
int32_t ib_harness_route_v1(IbHarnessHandle *handle, IbByteSpanV1 prompt,
                            int8_t explicit_level, uint8_t *out_level);
int32_t ib_harness_run_v1(IbHarnessHandle *handle, IbByteSpanV1 prompt,
                          int8_t explicit_level, IbOwnedBytesV1 *out_bytes);
int32_t ib_harness_run_with_cancel_v1(IbHarnessHandle *handle, IbByteSpanV1 prompt,
                                      int8_t explicit_level,
                                      IbCancellationHandle *cancellation,
                                      IbOwnedBytesV1 *out_bytes);
int32_t ib_harness_bytes_free_v1(IbOwnedBytesV1 *bytes);
const char *ib_harness_status_message_v1(int32_t status);

#ifdef __cplusplus
}
#endif

#endif
