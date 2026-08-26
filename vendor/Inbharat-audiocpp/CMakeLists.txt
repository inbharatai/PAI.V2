cmake_minimum_required(VERSION 3.20)

project(InBharatAudio VERSION 0.1.0 LANGUAGES C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

option(IBAUDIO_BUILD_SHARED "Build the versioned shared C ABI library" ON)
option(IBAUDIO_BUILD_CLI "Build the ibaudio command-line tool" ON)
option(IBAUDIO_BUILD_TESTS "Build release-candidate tests" ON)
option(IBAUDIO_BUILD_ANDROID_JNI "Build thin Android JNI bridge" ${ANDROID})
option(IBAUDIO_ENABLE_VULKAN_PROBE "Compile non-inference Vulkan loader probes" OFF)
option(IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER "Compile isolated pinned audio.cpp adapter scaffolding" OFF)
option(IBAUDIO_ENABLE_SANITIZERS "Enable AddressSanitizer and UndefinedBehaviorSanitizer" OFF)
option(IBAUDIO_WARNINGS_AS_ERRORS "Treat first-party warnings as errors" ON)
option(IBAUDIO_ENABLE_ELF_VERSION_SCRIPT "Apply GNU-compatible ELF symbol version script" OFF)
option(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES "Compile experimental research modules (neural_codec, prosody_controller, voice_clone_engine) that are placeholders and not production-honest" OFF)
option(IBAUDIO_REMOTE_PROVIDERS "Compile remote providers (e.g. Sarvam). OFF by default; an offline deployment contains no remote code path at all." OFF)
set(IBAUDIO_AUDIO_CPP_SOURCE_DIR "" CACHE PATH "Pristine audio.cpp checkout at the pinned commit")
set(IBAUDIO_PINNED_AUDIO_CPP_COMMIT "26dcb5c4cf5aa016ae6285096a7b45f2671e5d17" CACHE STRING "Reviewed audio.cpp pin (release-0.6.1)")

# Lightweight distribution profiles. A profile selects which optional components are
# compiled; the core runtime is always built. Profiles are composable build presets,
# not separate products — the C ABI is identical across all of them.
#   core     — runtime + reference provider only (smallest)
#   minimal  — core + CLI
#   india    — desktop + Bharat adaptation layer + en-IN/hi-IN/hinglish packs
#   desktop  — minimal + innovation heuristic modules + Vulkan probe
#   pi       — core + india, CPU-first, no Vulkan probe (ARM64 lean)
#   android  — india + JNI bridge, remote providers OFF
#   full     — everything including experimental research modules and the pinned
#              audio.cpp adapter scaffold
set(IBAUDIO_PROFILE "desktop" CACHE STRING "Distribution profile: core|minimal|india|desktop|pi|android|full")
set_property(CACHE IBAUDIO_PROFILE PROPERTY STRINGS core minimal india desktop pi android full)

# The profile is a convenience default for the experimental-research switch (the only
# component that changes the compiled source set today). An explicit
# -DIBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=ON on the command line still wins because
# option() above already recorded the default before this block runs. core/minimal/india/
# desktop/pi/android all build the same lean native runtime (experimental OFF); full adds
# the gated placeholder modules for research builds.
if(IBAUDIO_PROFILE STREQUAL "full" AND NOT IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES)
    set(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES ON CACHE BOOL "full profile enables research modules" FORCE)
endif()
message(STATUS "InBharat Audio profile: ${IBAUDIO_PROFILE} (experimental=${IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES})")

find_package(Threads REQUIRED)

set(IBAUDIO_SOURCES
    src/adaptation/personal_adaptation.cpp
    src/audio.cpp
    src/backend_probe.cpp
    src/facade_util.cpp
    src/language/language.cpp
    src/mesh/speech_mesh.cpp
    src/packs/pack_registry.cpp
    src/pipeline/speech_to_speech.cpp
    src/provider.cpp
    src/providers/reference_provider.cpp
    src/providers/ai4bharat_provider.cpp
    src/runtime.cpp
    src/session.cpp
    src/sha256.cpp
    src/stream.cpp
    src/transport/transport.cpp
    src/innovation/turn/turn_manager.cpp
    src/innovation/conversation/conversation_state.cpp
    src/innovation/environment/environment_adapter.cpp
    src/innovation/codeswitch/codeswitch_detector.cpp
    src/innovation/context/context_aware_output.cpp)

# Experimental research modules: placeholder implementations whose public names
# overclaim their algorithms (see docs/audit/03_CURRENT_PRODUCTION_VS_RESEARCH.md).
# Excluded from the default build so the stable ABI does not promise a neural codec,
# a voice cloner, or a prosody inference path it does not have.
# IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=ON restores them for research builds.
if(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES)
    list(APPEND IBAUDIO_SOURCES
        src/innovation/prosody/prosody_controller.cpp
        src/innovation/voice/voice_clone_engine.cpp
        src/innovation/codec/neural_codec.cpp)
endif()

# Remote providers (e.g. Sarvam) are excluded from the default build entirely.
if(IBAUDIO_REMOTE_PROVIDERS)
    list(APPEND IBAUDIO_SOURCES src/providers/sarvam_provider.cpp)
endif()

if(IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER)
    if(NOT IBAUDIO_AUDIO_CPP_SOURCE_DIR)
        message(FATAL_ERROR "IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER requires IBAUDIO_AUDIO_CPP_SOURCE_DIR")
    endif()
    find_package(Git REQUIRED)
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${IBAUDIO_AUDIO_CPP_SOURCE_DIR}" rev-parse HEAD
        OUTPUT_VARIABLE IBAUDIO_AUDIO_CPP_ACTUAL_COMMIT
        OUTPUT_STRIP_TRAILING_WHITESPACE
        RESULT_VARIABLE IBAUDIO_AUDIO_CPP_GIT_RESULT)
    if(NOT IBAUDIO_AUDIO_CPP_GIT_RESULT EQUAL 0 OR
       NOT IBAUDIO_AUDIO_CPP_ACTUAL_COMMIT STREQUAL IBAUDIO_PINNED_AUDIO_CPP_COMMIT)
        message(FATAL_ERROR
            "audio.cpp adapter requires pristine pin ${IBAUDIO_PINNED_AUDIO_CPP_COMMIT}; got '${IBAUDIO_AUDIO_CPP_ACTUAL_COMMIT}'")
    endif()
    execute_process(
        COMMAND "${GIT_EXECUTABLE}" -C "${IBAUDIO_AUDIO_CPP_SOURCE_DIR}" status --porcelain
        OUTPUT_VARIABLE IBAUDIO_AUDIO_CPP_STATUS
        OUTPUT_STRIP_TRAILING_WHITESPACE)
    if(IBAUDIO_AUDIO_CPP_STATUS)
        message(FATAL_ERROR "audio.cpp adapter refuses a modified upstream checkout")
    endif()
    list(APPEND IBAUDIO_SOURCES src/adapters/audio_cpp/audio_cpp_adapter.cpp)

    # Real audio.cpp provider (VAD via bundled Silero weights; ASR/TTS gated). Requires
    # the pinned upstream to have been built already (scripts/build_audio_cpp_provider.sh
    # builds it). The upstream build dir holds the static libs we link against.
    set(IBAUDIO_AUDIO_CPP_BUILD_DIR "" CACHE PATH "Built audio.cpp build dir (contains libengine_runtime.a, ggml, sentencepiece)")
    if(NOT IBAUDIO_AUDIO_CPP_BUILD_DIR)
        message(FATAL_ERROR "audio.cpp provider requires IBAUDIO_AUDIO_CPP_BUILD_DIR (build the pinned audio.cpp first)")
    endif()
    list(APPEND IBAUDIO_SOURCES src/adapters/audio_cpp/audio_cpp_provider.cpp)
    list(APPEND IBAUDIO_SOURCES src/adapters/audio_cpp/audio_cpp_asr_provider.cpp)
    set(_ac "${IBAUDIO_AUDIO_CPP_SOURCE_DIR}")
    set(_acb "${IBAUDIO_AUDIO_CPP_BUILD_DIR}")
    set(IBAUDIO_AUDIO_CPP_INCLUDE_DIRS
        "${_ac}/include"
        "${_ac}/external/ggml/include"
        "${_ac}/external/sentencepiece/src"
        "${_ac}/external/cJSON"
        "${_ac}/external/libyaml/include")
    # The pinned audio.cpp ships as Unix static archives (lib<name>.a). On MSVC the
    # Visual Studio multi-config generator instead emits <name>.lib under a per-
    # configuration subdirectory (e.g. build/Release/engine_runtime.lib). Both the
    # import prefix ("lib") and the suffix (".a"->".lib") differ, so pick the right
    # layout per toolchain; the upstream build is otherwise the same set of targets.
    if(MSVC)
        set(IBAUDIO_AUDIO_CPP_LIBS
            "${_acb}/Release/engine_runtime.lib"
            "${_acb}/external/sentencepiece/src/Release/sentencepiece.lib"
            "${_acb}/ggml/src/Release/ggml.lib"
            "${_acb}/ggml/src/Release/ggml-base.lib"
            "${_acb}/ggml/src/Release/ggml-cpu.lib"
            "${_acb}/Release/cjson_vendor.lib"
            "${_acb}/Release/yaml_vendor.lib")
    else()
        set(IBAUDIO_AUDIO_CPP_LIBS
            "${_acb}/libengine_runtime.a"
            "${_acb}/external/sentencepiece/src/libsentencepiece.a"
            "${_acb}/ggml/src/libggml.a"
            "${_acb}/ggml/src/libggml-base.a"
            "${_acb}/ggml/src/libggml-cpu.a"
            "${_acb}/libcjson_vendor.a"
            "${_acb}/libyaml_vendor.a")
    endif()
    # The bundled Silero VAD model root (weights ship inside the pristine checkout).
    set(IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT "${_ac}/assets/framework/models/silero_vad")
    # Licensed, integrity-verified ASR model root (e.g. Qwen/Qwen3-ASR-0.6B, Apache-2.0,
    # SHA-256 verified against the official LFS object id). Empty = ASR provider UNAVAILABLE.
    set(IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT "" CACHE PATH "Licensed, hash-verified ASR model directory (e.g. Qwen3-ASR-0.6B)")
    # The ASR/VAD provider TUs include third-party ggml/audio.cpp headers that trip
    # -Wshadow/-Wconversion under the strict -Werror profile. Relax warnings there.
    # The -Wno-* switches are GCC/Clang-only; on MSVC cl.exe does not recognise them
    # (the global /W4 already governs the warning level for those TUs).
    if(NOT MSVC)
        set_source_files_properties(src/adapters/audio_cpp/audio_cpp_asr_provider.cpp
            PROPERTIES COMPILE_OPTIONS "-Wno-shadow;-Wno-conversion;-Wno-unused-parameter")
    endif()
endif()

if(IBAUDIO_BUILD_SHARED)
    add_library(ibaudio SHARED ${IBAUDIO_SOURCES})
    target_compile_definitions(ibaudio PRIVATE IBAUDIO_BUILDING_LIBRARY PUBLIC IBAUDIO_SHARED)
    set_target_properties(ibaudio PROPERTIES
        C_VISIBILITY_PRESET hidden
        CXX_VISIBILITY_PRESET hidden
        VISIBILITY_INLINES_HIDDEN YES)
    if(NOT ANDROID)
        set_target_properties(ibaudio PROPERTIES VERSION ${PROJECT_VERSION} SOVERSION 1)
    endif()
else()
    add_library(ibaudio STATIC ${IBAUDIO_SOURCES})
endif()
add_library(InBharat::Audio ALIAS ibaudio)

target_include_directories(ibaudio
    PUBLIC
        $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
        $<INSTALL_INTERFACE:include>
    PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/src)
target_link_libraries(ibaudio PRIVATE Threads::Threads)
if(CMAKE_DL_LIBS AND NOT WIN32)
    target_link_libraries(ibaudio PRIVATE ${CMAKE_DL_LIBS})
endif()
target_compile_definitions(ibaudio PRIVATE
    $<$<BOOL:${IBAUDIO_ENABLE_VULKAN_PROBE}>:IBAUDIO_ENABLE_VULKAN_PROBE=1>
    $<$<BOOL:${IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER}>:IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=1>
    $<$<BOOL:${IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES}>:IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=1>
    $<$<BOOL:${IBAUDIO_REMOTE_PROVIDERS}>:IBAUDIO_REMOTE_PROVIDERS=1>)

# audio.cpp provider: static libs and the bundled Silero VAD model root. Upstream headers
# are added as SYSTEM below (they are third-party and trip -Wshadow/-Wconversion).
if(IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER)
    find_package(OpenMP)  # ggml's CPU backend uses OpenMP
    target_link_libraries(ibaudio PRIVATE ${IBAUDIO_AUDIO_CPP_LIBS})
    if(OpenMP_CXX_FOUND)
        target_link_libraries(ibaudio PRIVATE OpenMP::OpenMP_CXX)
    endif()
    target_compile_definitions(ibaudio PRIVATE
        IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT="${IBAUDIO_AUDIO_CPP_SILERO_VAD_ROOT}"
        IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT="${IBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT}")
    # Upstream ggml/audio.cpp headers are third-party and trip -Wshadow/-Wconversion under
    # our strict -Werror profile. Relax warnings for the VAD adapter TU only; the C ABI core
    # stays warnings-as-errors. Mark upstream includes SYSTEM to silence their warnings.
    # The -Wno-* switches are GCC/Clang-only; MSVC's /W4 already governs these TUs.
    if(NOT MSVC)
        set_source_files_properties(src/adapters/audio_cpp/audio_cpp_provider.cpp
            PROPERTIES COMPILE_OPTIONS "-Wno-shadow;-Wno-conversion;-Wno-unused-parameter")
    endif()
    target_include_directories(ibaudio SYSTEM PRIVATE ${IBAUDIO_AUDIO_CPP_INCLUDE_DIRS})
endif()

if(MSVC)
    target_compile_options(ibaudio PRIVATE /W4 /permissive- /EHsc)
    if(IBAUDIO_WARNINGS_AS_ERRORS)
        target_compile_options(ibaudio PRIVATE /WX)
    endif()
else()
    target_compile_options(ibaudio PRIVATE -Wall -Wextra -Wpedantic -Wconversion -Wshadow)
    if(IBAUDIO_WARNINGS_AS_ERRORS)
        target_compile_options(ibaudio PRIVATE -Werror)
    endif()
endif()

if(ANDROID)
    # CPU-first baseline: no host-native ISA, OpenMP, CUDA, HIP, or Metal flags.
    target_compile_options(ibaudio PRIVATE -fno-openmp)
    target_link_options(ibaudio PRIVATE -Wl,--no-undefined)
    target_link_libraries(ibaudio PRIVATE log android)
endif()

if(IBAUDIO_ENABLE_SANITIZERS AND NOT MSVC)
    target_compile_options(ibaudio PRIVATE -fno-omit-frame-pointer -fsanitize=address,undefined)
    target_link_options(ibaudio PRIVATE -fsanitize=address,undefined)
endif()

if(UNIX AND NOT APPLE AND IBAUDIO_BUILD_SHARED)
    target_link_options(ibaudio PRIVATE "LINKER:--no-undefined")
    if(IBAUDIO_ENABLE_ELF_VERSION_SCRIPT)
        target_link_options(ibaudio PRIVATE
            "LINKER:--version-script=${CMAKE_CURRENT_SOURCE_DIR}/cmake/ibaudio.map")
    endif()
endif()

if(IBAUDIO_BUILD_CLI)
    add_executable(ibaudio_cli tools/ibaudio/main.cpp)
    set_target_properties(ibaudio_cli PROPERTIES OUTPUT_NAME ibaudio)
    target_link_libraries(ibaudio_cli PRIVATE InBharat::Audio)
    if(MSVC)
        target_compile_options(ibaudio_cli PRIVATE /W4 /permissive-)
    else()
        target_compile_options(ibaudio_cli PRIVATE -Wall -Wextra -Wpedantic)
    endif()
    if(IBAUDIO_ENABLE_SANITIZERS AND NOT MSVC)
        target_compile_options(ibaudio_cli PRIVATE -fno-omit-frame-pointer -fsanitize=address,undefined)
        target_link_options(ibaudio_cli PRIVATE -fsanitize=address,undefined)
    endif()
endif()

option(IBAUDIO_BUILD_MCP "Build the ibaudio-mcp MCP gateway (control plane; no PCM over MCP)" ON)
if(IBAUDIO_BUILD_MCP)
    add_executable(ibaudio_mcp tools/ibaudio_mcp/main.cpp)
    set_target_properties(ibaudio_mcp PROPERTIES OUTPUT_NAME ibaudio-mcp)
    target_link_libraries(ibaudio_mcp PRIVATE InBharat::Audio)
    if(MSVC)
        target_compile_options(ibaudio_mcp PRIVATE /W4 /permissive-)
    else()
        target_compile_options(ibaudio_mcp PRIVATE -Wall -Wextra -Wpedantic)
    endif()
    if(IBAUDIO_ENABLE_SANITIZERS AND NOT MSVC)
        target_compile_options(ibaudio_mcp PRIVATE -fno-omit-frame-pointer -fsanitize=address,undefined)
        target_link_options(ibaudio_mcp PRIVATE -fsanitize=address,undefined)
    endif()
endif()

if(IBAUDIO_BUILD_ANDROID_JNI)
    if(NOT ANDROID)
        message(FATAL_ERROR "IBAUDIO_BUILD_ANDROID_JNI is only valid with an Android toolchain")
    endif()
    add_library(ibaudio_jni SHARED android/inbharat-audio/src/main/cpp/ibaudio_jni.cpp)
    target_include_directories(ibaudio_jni PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/include)
    target_link_libraries(ibaudio_jni PRIVATE ibaudio log android)
    target_compile_options(ibaudio_jni PRIVATE -Wall -Wextra -Wpedantic -fno-openmp)
    target_link_options(ibaudio_jni PRIVATE -Wl,--no-undefined)
    set_target_properties(ibaudio_jni PROPERTIES
        CXX_VISIBILITY_PRESET hidden VISIBILITY_INLINES_HIDDEN YES)
endif()

include(GNUInstallDirs)
include(CMakePackageConfigHelpers)
configure_package_config_file(
    cmake/InBharatAudioConfig.cmake.in
    "${CMAKE_CURRENT_BINARY_DIR}/InBharatAudioConfig.cmake"
    INSTALL_DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/InBharatAudio")
write_basic_package_version_file(
    "${CMAKE_CURRENT_BINARY_DIR}/InBharatAudioConfigVersion.cmake"
    VERSION ${PROJECT_VERSION}
    COMPATIBILITY SameMajorVersion)
install(TARGETS ibaudio
    EXPORT InBharatAudioTargets
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
    LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR})
install(FILES include/inbharat/ibaudio.h DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}/inbharat)
install(EXPORT InBharatAudioTargets
    FILE InBharatAudioTargets.cmake
    NAMESPACE InBharat::
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/InBharatAudio)
install(FILES
    "${CMAKE_CURRENT_BINARY_DIR}/InBharatAudioConfig.cmake"
    "${CMAKE_CURRENT_BINARY_DIR}/InBharatAudioConfigVersion.cmake"
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/InBharatAudio)

include(CTest)
if(BUILD_TESTING AND IBAUDIO_BUILD_TESTS)
    add_subdirectory(tests)
endif()

message(STATUS "InBharat Audio ${PROJECT_VERSION}: CPU reference backend enabled")
message(STATUS "audio.cpp adapter: ${IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER} (default build has no upstream dependency)")
message(STATUS "Vulkan: probe-only=${IBAUDIO_ENABLE_VULKAN_PROBE}; inference adapter intentionally unavailable")
