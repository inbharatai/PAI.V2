//! Versioned, size-tagged C ABI for embedding the Harness core.
//!
//! The ABI uses opaque handles, caller-visible status codes, UTF-8 spans, and explicit
//! ownership. Panics are contained at every exported boundary.

use inbharat_harness_core::error::{ErrorCode, Failure};
use inbharat_harness_core::routing::ExecutionLevel;
use inbharat_harness_core::runtime::{HarnessBuilder, RunOptions};
use inbharat_harness_core::{CancelCause, CancellationToken, Harness};
use std::ffi::{c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

/// ABI major version supported by this library.
pub const IB_HARNESS_ABI_VERSION: u32 = 1;
/// Successful operation.
pub const IB_STATUS_OK: i32 = 0;
/// Invalid pointer, size tag, UTF-8, or argument.
pub const IB_STATUS_INVALID_ARGUMENT: i32 = 1;
/// Policy denied the operation.
pub const IB_STATUS_DENIED: i32 = 2;
/// Requested capability was unavailable.
pub const IB_STATUS_UNAVAILABLE: i32 = 3;
/// Operation was cancelled.
pub const IB_STATUS_CANCELLED: i32 = 4;
/// Budget or deadline was exhausted.
pub const IB_STATUS_RESOURCE_EXHAUSTED: i32 = 5;
/// Provider, tool, or persistence failure.
pub const IB_STATUS_OPERATION_FAILED: i32 = 6;
/// A Rust panic was caught at the ABI boundary.
pub const IB_STATUS_PANIC: i32 = 255;

/// Borrowed UTF-8 bytes. Ownership remains with the caller for the duration of the call.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbByteSpanV1 {
    pub struct_size: u32,
    pub data: *const u8,
    pub len: usize,
}

/// Library-owned result bytes. Release exactly once with `ib_harness_bytes_free_v1`.
#[repr(C)]
#[derive(Debug)]
pub struct IbOwnedBytesV1 {
    pub struct_size: u32,
    pub data: *mut u8,
    pub len: usize,
}

impl Default for IbOwnedBytesV1 {
    fn default() -> Self {
        Self {
            struct_size: u32::try_from(std::mem::size_of::<Self>()).unwrap_or(u32::MAX),
            data: ptr::null_mut(),
            len: 0,
        }
    }
}

/// Size-tagged creation configuration.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IbHarnessConfigV1 {
    pub struct_size: u32,
    pub abi_version: u32,
    pub root: IbByteSpanV1,
    pub maximum_level: u8,
    pub reserved: [u8; 7],
}

/// Magic tag proving a harness handle is one this library created and has not
/// destroyed. Checked on every dereference and zeroed on destroy so a stale,
/// swapped, or type-confused pointer fails with IB_STATUS_INVALID_ARGUMENT
/// instead of causing undefined behavior.
const HARNESS_HANDLE_MAGIC: u64 = 0x4942_4841_5253_0031; // "IBHARS\0\x31"
/// Magic tag for cancellation handles (distinct from the harness tag so a
/// `*mut IbHarnessHandle` can never be accepted where a cancellation handle is
/// expected and vice versa).
const CANCEL_HANDLE_MAGIC: u64 = 0x4942_4341_4E43_0031; // "IBCANC\0\x31"

/// Opaque allocation returned to C.
#[repr(C)]
pub struct IbHarnessHandle {
    magic: u64,
    harness: Harness,
}

/// Opaque cancellation allocation safe to request from another caller thread.
#[repr(C)]
pub struct IbCancellationHandle {
    magic: u64,
    token: CancellationToken,
}

/// Returns ABI major version 1.
#[unsafe(no_mangle)]
pub extern "C" fn ib_harness_api_version_v1() -> u32 {
    IB_HARNESS_ABI_VERSION
}

/// Creates one harness handle. `out_handle` must point to writable storage.
///
/// # Safety
/// `config` and `out_handle` must be valid, aligned pointers for this call. Every span in
/// `config` must remain readable for its declared length. The returned handle must be destroyed
/// exactly once with `ib_harness_destroy_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_create_v1(
    config: *const IbHarnessConfigV1,
    out_handle: *mut *mut IbHarnessHandle,
) -> i32 {
    ffi_boundary(|| {
        if config.is_null() || out_handle.is_null() {
            return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
        }
        // SAFETY: non-null was checked; the caller contract requires a valid aligned struct.
        let config = unsafe { &*config };
        validate_size::<IbHarnessConfigV1>(config.struct_size)?;
        if config.abi_version != IB_HARNESS_ABI_VERSION {
            return Err(AbiFailure::Status(IB_STATUS_UNAVAILABLE));
        }
        let root = read_utf8(config.root)?;
        let maximum_level = level_from_u8(config.maximum_level)?;
        let harness = HarnessBuilder::local(root)
            .map_err(AbiFailure::Harness)?
            .route_policy(inbharat_harness_core::RoutePolicy {
                maximum_level,
                allow_explicit_escalation: true,
            })
            .build();
        let handle = Box::new(IbHarnessHandle {
            magic: HARNESS_HANDLE_MAGIC,
            harness,
        });
        // SAFETY: out_handle is non-null writable storage by the caller contract.
        unsafe { *out_handle = Box::into_raw(handle) };
        Ok(())
    })
}

/// Destroys one handle. A null handle is accepted as a no-op.
///
/// # Safety
/// A non-null handle must have been returned by `ib_harness_create_v1`, remain live, and not have
/// been destroyed previously. No concurrent call may use it during destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_destroy_v1(handle: *mut IbHarnessHandle) -> i32 {
    ffi_boundary(|| {
        if !handle.is_null() {
            // SAFETY: handles are created by ib_harness_create_v1 and must be freed once.
            // Zero the magic tag before dropping so a use-after-destroy or
            // double-destroy is caught by the tag check on any later call
            // instead of dereferencing freed memory.
            unsafe {
                let mut boxed = Box::from_raw(handle);
                boxed.magic = 0;
                drop(boxed);
            }
        }
        Ok(())
    })
}

/// Creates a cancellation handle for cross-thread run cancellation.
///
/// # Safety
/// `out_handle` must point to writable storage. The returned handle must be destroyed exactly once
/// with `ib_harness_cancel_destroy_v1` and remain live while any run uses it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_cancel_create_v1(
    out_handle: *mut *mut IbCancellationHandle,
) -> i32 {
    ffi_boundary(|| {
        if out_handle.is_null() {
            return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
        }
        let handle = Box::new(IbCancellationHandle {
            magic: CANCEL_HANDLE_MAGIC,
            token: CancellationToken::new(),
        });
        // SAFETY: out_handle is non-null writable storage by the caller contract.
        unsafe { *out_handle = Box::into_raw(handle) };
        Ok(())
    })
}

/// Requests first-cause-wins cancellation (0 user, 1 parent, 2 deadline, 3 policy, 4 shutdown,
/// 5 disposed). Repeated requests succeed but do not replace the first cause.
///
/// # Safety
/// `handle` must be a live cancellation handle and must not be concurrently destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_cancel_request_v1(
    handle: *mut IbCancellationHandle,
    cause: u8,
) -> i32 {
    ffi_boundary(|| {
        let handle = cancellation_ref(handle)?;
        let cause = cancel_cause_from_u8(cause)?;
        handle.token.cancel(cause);
        Ok(())
    })
}

/// Destroys a cancellation handle. A null handle is a no-op.
///
/// # Safety
/// A non-null handle must have been returned by `ib_harness_cancel_create_v1`, must not have been
/// destroyed already, and must not be in use by a concurrent run.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_cancel_destroy_v1(handle: *mut IbCancellationHandle) -> i32 {
    ffi_boundary(|| {
        if !handle.is_null() {
            // SAFETY: caller guarantees a unique live handle from cancel_create.
            // Zero the magic tag before dropping so a later stale/destroyed
            // handle fails the tag check instead of dereferencing freed memory.
            unsafe {
                let mut boxed = Box::from_raw(handle);
                boxed.magic = 0;
                drop(boxed);
            }
        }
        Ok(())
    })
}

/// Routes UTF-8 input and returns level 0..3 through `out_level`.
///
/// # Safety
/// `handle` must be live, `out_level` must be writable, and `prompt` must describe readable bytes
/// for the duration of the call. These pointers must not be concurrently invalidated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_route_v1(
    handle: *mut IbHarnessHandle,
    prompt: IbByteSpanV1,
    explicit_level: i8,
    out_level: *mut u8,
) -> i32 {
    ffi_boundary(|| {
        let handle = handle_ref(handle)?;
        if out_level.is_null() {
            return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
        }
        let prompt = read_utf8(prompt)?;
        let options = RunOptions {
            explicit_level: explicit_level_option(explicit_level)?,
            ..RunOptions::default()
        };
        let decision = handle
            .harness
            .route(prompt, &options)
            .map_err(AbiFailure::Harness)?;
        // SAFETY: out_level is non-null writable storage by the caller contract.
        unsafe { *out_level = decision.level as u8 };
        Ok(())
    })
}

/// Routes and runs one bounded task. Output is UTF-8 owned by the library.
///
/// # Safety
/// `handle` must be live, `out_bytes` must be writable, and `prompt` must describe readable bytes
/// for the call. A successful output must be released once with `ib_harness_bytes_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_run_v1(
    handle: *mut IbHarnessHandle,
    prompt: IbByteSpanV1,
    explicit_level: i8,
    out_bytes: *mut IbOwnedBytesV1,
) -> i32 {
    ffi_boundary(|| {
        let handle = handle_ref(handle)?;
        if out_bytes.is_null() {
            return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
        }
        // Validate every argument BEFORE writing to caller memory. Clearing
        // `*out_bytes` first (the old order) would clobber a caller's live
        // allocation pointer on a failing call, leaking it. Now the caller's
        // storage is only touched once the call is known to proceed.
        let prompt = read_utf8(prompt)?;
        let options = RunOptions {
            explicit_level: explicit_level_option(explicit_level)?,
            ..RunOptions::default()
        };
        // SAFETY: out_bytes is non-null writable storage by the caller contract.
        unsafe { *out_bytes = IbOwnedBytesV1::default() };
        let (outcome, _session) = handle
            .harness
            .run(prompt, &options, &CancellationToken::new())
            .map_err(AbiFailure::Harness)?;
        write_owned(out_bytes, outcome.output)?;
        Ok(())
    })
}

/// Runs one bounded task using a caller-owned cancellation handle.
///
/// # Safety
/// Harness/cancellation handles must remain live and not be destroyed for the call. `prompt` must
/// be readable and `out_bytes` writable. Cancellation may be requested concurrently from another
/// thread. Successful output must be released once with `ib_harness_bytes_free_v1`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_run_with_cancel_v1(
    handle: *mut IbHarnessHandle,
    prompt: IbByteSpanV1,
    explicit_level: i8,
    cancellation: *mut IbCancellationHandle,
    out_bytes: *mut IbOwnedBytesV1,
) -> i32 {
    ffi_boundary(|| {
        let handle = handle_ref(handle)?;
        let cancellation = cancellation_ref(cancellation)?;
        if out_bytes.is_null() {
            return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
        }
        // Validate every argument BEFORE writing to caller memory (see
        // ib_harness_run_v1): clearing `*out_bytes` before validation would
        // clobber a caller's live allocation pointer on a failing call.
        let prompt = read_utf8(prompt)?;
        let options = RunOptions {
            explicit_level: explicit_level_option(explicit_level)?,
            ..RunOptions::default()
        };
        // SAFETY: out_bytes is non-null writable storage by the caller contract.
        unsafe { *out_bytes = IbOwnedBytesV1::default() };
        let (outcome, _session) = handle
            .harness
            .run(prompt, &options, &cancellation.token)
            .map_err(AbiFailure::Harness)?;
        write_owned(out_bytes, outcome.output)?;
        Ok(())
    })
}

/// Releases bytes returned by the library. The struct is reset to an empty value.
///
/// # Safety
/// A non-null pointer must reference writable `IbOwnedBytesV1` storage. Its data must either be
/// null or be the still-owned allocation returned by `ib_harness_run_v1`, not previously freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ib_harness_bytes_free_v1(bytes: *mut IbOwnedBytesV1) -> i32 {
    ffi_boundary(|| {
        if bytes.is_null() {
            return Ok(());
        }
        // SAFETY: non-null was checked and caller contract supplies writable storage.
        let bytes = unsafe { &mut *bytes };
        validate_size::<IbOwnedBytesV1>(bytes.struct_size)?;
        if !bytes.data.is_null() {
            // SAFETY: the pointer/length pair came from write_owned and is released once.
            let slice = ptr::slice_from_raw_parts_mut(bytes.data, bytes.len);
            // SAFETY: slice was allocated by Box<[u8]> in write_owned.
            unsafe { drop(Box::from_raw(slice)) };
        }
        *bytes = IbOwnedBytesV1::default();
        Ok(())
    })
}

/// Returns a static human-readable status label. The pointer is NUL-terminated.
#[unsafe(no_mangle)]
pub extern "C" fn ib_harness_status_message_v1(status: i32) -> *const c_char {
    let message: &'static [u8] = match status {
        IB_STATUS_OK => b"ok\0",
        IB_STATUS_INVALID_ARGUMENT => b"invalid argument\0",
        IB_STATUS_DENIED => b"denied\0",
        IB_STATUS_UNAVAILABLE => b"unavailable\0",
        IB_STATUS_CANCELLED => b"cancelled\0",
        IB_STATUS_RESOURCE_EXHAUSTED => b"resource exhausted\0",
        IB_STATUS_OPERATION_FAILED => b"operation failed\0",
        IB_STATUS_PANIC => b"panic contained\0",
        _ => b"unknown status\0",
    };
    message.as_ptr().cast()
}

fn handle_ref<'a>(handle: *mut IbHarnessHandle) -> Result<&'a IbHarnessHandle, AbiFailure> {
    if handle.is_null() {
        return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
    }
    // SAFETY: non-null was checked. Reading the leading tag field is valid for
    // any pointer the caller passes; the tag check below rejects any pointer
    // that is not a live handle this library created (stale, swapped, or
    // type-confused), turning potential UB into a clean status error.
    let reference = unsafe { &*handle };
    if reference.magic != HARNESS_HANDLE_MAGIC {
        return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
    }
    Ok(reference)
}

fn cancellation_ref<'a>(
    handle: *mut IbCancellationHandle,
) -> Result<&'a IbCancellationHandle, AbiFailure> {
    if handle.is_null() {
        return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
    }
    // SAFETY: as above; the tag check rejects stale/swapped/type-confused
    // pointers before the handle body is touched.
    let reference = unsafe { &*handle };
    if reference.magic != CANCEL_HANDLE_MAGIC {
        return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
    }
    Ok(reference)
}

fn read_utf8<'a>(span: IbByteSpanV1) -> Result<&'a str, AbiFailure> {
    validate_size::<IbByteSpanV1>(span.struct_size)?;
    if span.len > 8 * 1024 * 1024 || (span.data.is_null() && span.len != 0) {
        return Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT));
    }
    let bytes = if span.len == 0 {
        &[]
    } else {
        // SAFETY: pointer/length validity is part of the C caller contract.
        unsafe { std::slice::from_raw_parts(span.data, span.len) }
    };
    std::str::from_utf8(bytes).map_err(|_| AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT))
}

fn validate_size<T>(size: u32) -> Result<(), AbiFailure> {
    let expected = u32::try_from(std::mem::size_of::<T>()).unwrap_or(u32::MAX);
    if size < expected {
        Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT))
    } else {
        Ok(())
    }
}

fn level_from_u8(level: u8) -> Result<ExecutionLevel, AbiFailure> {
    match level {
        0 => Ok(ExecutionLevel::L0),
        1 => Ok(ExecutionLevel::L1),
        2 => Ok(ExecutionLevel::L2),
        3 => Ok(ExecutionLevel::L3),
        _ => Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT)),
    }
}

fn cancel_cause_from_u8(cause: u8) -> Result<CancelCause, AbiFailure> {
    match cause {
        0 => Ok(CancelCause::User),
        1 => Ok(CancelCause::Parent),
        2 => Ok(CancelCause::Deadline),
        3 => Ok(CancelCause::Policy),
        4 => Ok(CancelCause::Shutdown),
        5 => Ok(CancelCause::Disposed),
        _ => Err(AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT)),
    }
}

fn explicit_level_option(level: i8) -> Result<Option<ExecutionLevel>, AbiFailure> {
    if level < 0 {
        Ok(None)
    } else {
        let level =
            u8::try_from(level).map_err(|_| AbiFailure::Status(IB_STATUS_INVALID_ARGUMENT))?;
        level_from_u8(level).map(Some)
    }
}

fn write_owned(out: *mut IbOwnedBytesV1, value: String) -> Result<(), AbiFailure> {
    let mut boxed = value.into_bytes().into_boxed_slice();
    let len = boxed.len();
    let data = boxed.as_mut_ptr();
    std::mem::forget(boxed);
    // SAFETY: out is validated non-null by the exported caller.
    unsafe {
        (*out).data = data;
        (*out).len = len;
    }
    Ok(())
}

#[derive(Debug)]
enum AbiFailure {
    Status(i32),
    Harness(Failure),
}

fn ffi_boundary(operation: impl FnOnce() -> Result<(), AbiFailure>) -> i32 {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => IB_STATUS_OK,
        Ok(Err(AbiFailure::Status(status))) => status,
        Ok(Err(AbiFailure::Harness(failure))) => status_for_failure(&failure),
        Err(_panic) => IB_STATUS_PANIC,
    }
}

fn status_for_failure(failure: &Failure) -> i32 {
    match failure.code {
        ErrorCode::InvalidInput => IB_STATUS_INVALID_ARGUMENT,
        ErrorCode::RouteDenied
        | ErrorCode::PermissionDenied
        | ErrorCode::ConfirmationRequired
        | ErrorCode::FilesystemDenied
        | ErrorCode::SubprocessDenied => IB_STATUS_DENIED,
        ErrorCode::CapabilityUnavailable | ErrorCode::SandboxUnavailable | ErrorCode::NotFound => {
            IB_STATUS_UNAVAILABLE
        }
        ErrorCode::Cancelled => IB_STATUS_CANCELLED,
        ErrorCode::BudgetExceeded | ErrorCode::Timeout | ErrorCode::RecoveryExhausted => {
            IB_STATUS_RESOURCE_EXHAUSTED
        }
        ErrorCode::ProviderFailed
        | ErrorCode::ToolFailed
        | ErrorCode::VerificationFailed
        | ErrorCode::SessionCorrupt
        | ErrorCode::Conflict
        | ErrorCode::Internal => IB_STATUS_OPERATION_FAILED,
        _ => IB_STATUS_OPERATION_FAILED,
    }
}

/// Reserved user data type for future callback structs; never interpreted in ABI v1.
pub type IbUserData = *mut c_void;

#[cfg(test)]
mod tests {
    use super::*;

    fn span(value: &str) -> IbByteSpanV1 {
        IbByteSpanV1 {
            struct_size: u32::try_from(std::mem::size_of::<IbByteSpanV1>()).unwrap_or(u32::MAX),
            data: value.as_ptr(),
            len: value.len(),
        }
    }

    #[test]
    fn abi_create_route_run_destroy() -> inbharat_harness_core::HarnessResult<()> {
        let config = IbHarnessConfigV1 {
            struct_size: u32::try_from(std::mem::size_of::<IbHarnessConfigV1>())
                .unwrap_or(u32::MAX),
            abi_version: IB_HARNESS_ABI_VERSION,
            root: span("."),
            maximum_level: 3,
            reserved: [0; 7],
        };
        let mut handle = ptr::null_mut();
        // SAFETY: test supplies valid size-tagged pointers for the duration of each call.
        let status = unsafe { ib_harness_create_v1(&config, &mut handle) };
        assert_eq!(status, IB_STATUS_OK);
        let mut level = 255_u8;
        // SAFETY: handle and output pointer are live.
        let status = unsafe { ib_harness_route_v1(handle, span("hello"), -1, &mut level) };
        assert_eq!(status, IB_STATUS_OK);
        assert_eq!(level, 0);
        // The standalone C ABI intentionally ships no model provider: production
        // embedders register a real provider (for Pocket AI: local llama-server).
        // With no model registered, run must fail closed (IB_STATUS_UNAVAILABLE)
        // rather than fabricate output. This validates the no-fake-default gate.
        let mut output = IbOwnedBytesV1::default();
        // SAFETY: handle and output struct are valid.
        let status = unsafe { ib_harness_run_v1(handle, span("hello"), 0, &mut output) };
        assert_eq!(status, IB_STATUS_UNAVAILABLE);
        assert!(output.data.is_null());
        // SAFETY: default/null output is accepted as a no-op free.
        assert_eq!(
            unsafe { ib_harness_bytes_free_v1(&mut output) },
            IB_STATUS_OK
        );
        let mut cancellation = ptr::null_mut();
        // SAFETY: output pointer is valid and receives one owned cancellation handle.
        assert_eq!(
            unsafe { ib_harness_cancel_create_v1(&mut cancellation) },
            IB_STATUS_OK
        );
        // SAFETY: cancellation is live and requested before the run.
        assert_eq!(
            unsafe { ib_harness_cancel_request_v1(cancellation, 0) },
            IB_STATUS_OK
        );
        let mut cancelled_output = IbOwnedBytesV1::default();
        // SAFETY: all handles and spans remain live for the call. The token was
        // already requested, so cancellation is honoured before model resolution
        // and the run closes as cancelled even though no model is registered.
        assert_eq!(
            unsafe {
                ib_harness_run_with_cancel_v1(
                    handle,
                    span("cancel me"),
                    0,
                    cancellation,
                    &mut cancelled_output,
                )
            },
            IB_STATUS_CANCELLED
        );
        assert!(cancelled_output.data.is_null());
        // SAFETY: cancellation handle is destroyed exactly once after use.
        assert_eq!(
            unsafe { ib_harness_cancel_destroy_v1(cancellation) },
            IB_STATUS_OK
        );
        // SAFETY: handle was returned by create and is destroyed once.
        assert_eq!(unsafe { ib_harness_destroy_v1(handle) }, IB_STATUS_OK);
        Ok(())
    }

    #[test]
    #[allow(clippy::panic)]
    fn panic_is_contained_at_ffi_boundary() {
        assert_eq!(
            ffi_boundary(|| -> Result<(), AbiFailure> { panic!("contained test panic") }),
            IB_STATUS_PANIC
        );
    }

    #[test]
    fn stale_or_confused_handle_is_rejected_by_magic_tag() {
        // Create a real harness handle, then corrupt its magic tag to simulate
        // a stale / type-confused / use-after-destroy pointer. The tag check in
        // handle_ref must reject it with IB_STATUS_INVALID_ARGUMENT before the
        // harness body is ever touched.
        let config = IbHarnessConfigV1 {
            struct_size: u32::try_from(std::mem::size_of::<IbHarnessConfigV1>())
                .unwrap_or(u32::MAX),
            abi_version: IB_HARNESS_ABI_VERSION,
            root: span("."),
            maximum_level: 3,
            reserved: [0; 7],
        };
        let mut handle = ptr::null_mut();
        // SAFETY: valid size-tagged pointers for the call duration.
        assert_eq!(
            unsafe { ib_harness_create_v1(&config, &mut handle) },
            IB_STATUS_OK
        );
        assert!(!handle.is_null());

        // Sanity: the intact handle works.
        let mut level = 255_u8;
        // SAFETY: real handle and live output pointer.
        assert_eq!(
            unsafe { ib_harness_route_v1(handle, span("hello"), -1, &mut level) },
            IB_STATUS_OK
        );

        // Corrupt the magic tag (as a stale or foreign pointer would have), and
        // confirm the call is now rejected instead of dereferenced.
        // SAFETY: handle is a live, owned handle; we only rewrite the leading tag.
        unsafe { (*handle).magic = 0 };
        let mut level2 = 255_u8;
        // SAFETY: handle points to a live struct; only its tag is wrong, and the
        // tag check must fire before the harness field is read.
        let status = unsafe { ib_harness_route_v1(handle, span("x"), -1, &mut level2) };
        assert_eq!(status, IB_STATUS_INVALID_ARGUMENT);

        // Restore the tag so destroy's drop path operates on a valid handle, then
        // destroy exactly once.
        // SAFETY: handle is still the live owned handle.
        unsafe { (*handle).magic = HARNESS_HANDLE_MAGIC };
        // SAFETY: handle was returned by create and is destroyed once.
        assert_eq!(unsafe { ib_harness_destroy_v1(handle) }, IB_STATUS_OK);
    }
}
