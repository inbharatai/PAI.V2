# Backend policy

## CPU baseline

The portable CPU backend is the only inference backend in RC1. It is always compiled, deterministic mode is default, and no native-host ISA or OpenMP flag is required. Runtime diagnostics name the selected backend.

## Unavailable accelerators

CUDA, HIP/ROCm, Metal, NNAPI, Core ML, and DirectML have explicit catalog rows with `NOT_BUILT` and a reason. They are not silently omitted. Vulkan can be compiled as a **loader-only probe**: it attempts to load the platform Vulkan library and resolve `vkGetInstanceProcAddr`. Even a successful loader probe reports `ADAPTER_UNAVAILABLE`, because no graph/model/device parity adapter is approved.

## Selection and fallback

- `CPU`: succeeds.
- `AUTO`: selects CPU in RC1.
- Explicit accelerator + fallback disabled: returns `UNAVAILABLE`.
- Explicit accelerator + fallback allowed: records a diagnostic/metric and creates a CPU runtime.
- Model-level explicit accelerator load never silently changes backend.

A future accelerator must pass compile, runtime device enumeration, operation support, per-model precision parity, cancellation, memory, and physical-device gates. Because no mixed graph scheduler exists in the planned audio.cpp adapter, failure requires session destruction and CPU recreation—not mid-graph migration.
