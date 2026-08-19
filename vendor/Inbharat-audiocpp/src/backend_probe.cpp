#include "internal.hpp"

#if defined(IBAUDIO_ENABLE_VULKAN_PROBE)
#  if defined(_WIN32)
#    define WIN32_LEAN_AND_MEAN
#    include <windows.h>
#  else
#    include <dlfcn.h>
#  endif
#endif

namespace ibaudio {

AcceleratorProbe probe_vulkan_loader() {
    AcceleratorProbe result;
#if !defined(IBAUDIO_ENABLE_VULKAN_PROBE)
    result.availability = IBAUDIO_BACKEND_NOT_BUILT;
    result.compiled = false;
    result.reason = "Vulkan loader probe disabled; inference remains CPU-only";
#elif defined(_WIN32)
    result.compiled = true;
    HMODULE library = LoadLibraryA("vulkan-1.dll");
    if (library == nullptr) {
        result.availability = IBAUDIO_BACKEND_PROBE_FAILED;
        result.reason = "Vulkan loader probe compiled, but vulkan-1.dll was not loadable; CPU fallback is required";
    } else {
        const bool symbol = GetProcAddress(library, "vkGetInstanceProcAddr") != nullptr;
        FreeLibrary(library);
        result.availability = symbol ? IBAUDIO_BACKEND_ADAPTER_UNAVAILABLE : IBAUDIO_BACKEND_PROBE_FAILED;
        result.device = symbol ? "Vulkan loader present (device not enumerated)" : "none";
        result.reason = symbol
            ? "Vulkan loader symbol probe passed, but no model inference adapter is approved; CPU fallback is required"
            : "Vulkan loader lacks vkGetInstanceProcAddr; CPU fallback is required";
    }
#else
    result.compiled = true;
    void *library = dlopen("libvulkan.so.1", RTLD_NOW | RTLD_LOCAL);
    if (library == nullptr) library = dlopen("libvulkan.so", RTLD_NOW | RTLD_LOCAL);
    if (library == nullptr) {
        result.availability = IBAUDIO_BACKEND_PROBE_FAILED;
        result.reason = "Vulkan loader probe compiled, but no Vulkan loader was loadable; CPU fallback is required";
    } else {
        const bool symbol = dlsym(library, "vkGetInstanceProcAddr") != nullptr;
        dlclose(library);
        result.availability = symbol ? IBAUDIO_BACKEND_ADAPTER_UNAVAILABLE : IBAUDIO_BACKEND_PROBE_FAILED;
        result.device = symbol ? "Vulkan loader present (device not enumerated)" : "none";
        result.reason = symbol
            ? "Vulkan loader symbol probe passed, but no model inference adapter is approved; CPU fallback is required"
            : "Vulkan loader lacks vkGetInstanceProcAddr; CPU fallback is required";
    }
#endif
    return result;
}

} // namespace ibaudio
