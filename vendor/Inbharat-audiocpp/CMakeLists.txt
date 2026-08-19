cmake_minimum_required(VERSION 3.20)

project(InBharatAudio VERSION 0.2.0 LANGUAGES C CXX)

set(CMAKE_C_STANDARD 99)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

option(IBAUDIO_BUILD_SHARED "Build the versioned shared C ABI library" ON)
option(IBAUDIO_BUILD_CLI "Build the ibaudio command-line tool" ON)
option(IBAUDIO_BUILD_TESTS "Build release-candidate tests" OFF)
option(IBAUDIO_BUILD_ANDROID_JNI "Build thin Android JNI bridge" ${ANDROID})
option(IBAUDIO_ENABLE_TEST_FIXTURE_MODELS "Compile deterministic reference ASR/TTS fixtures (tests only; never production)" OFF)
option(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES "Compile non-production research/heuristic innovation modules" OFF)
option(IBAUDIO_ENABLE_VULKAN_PROBE "Compile non-inference Vulkan loader probes" OFF)
option(IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER "Compile isolated pinned audio.cpp adapter scaffolding" OFF)
option(IBAUDIO_ENABLE_SANITIZERS "Enable AddressSanitizer and UndefinedBehaviorSanitizer" OFF)
option(IBAUDIO_WARNINGS_AS_ERRORS "Treat first-party warnings as errors" ON)
option(IBAUDIO_ENABLE_ELF_VERSION_SCRIPT "Apply GNU-compatible ELF symbol version script" OFF)
set(IBAUDIO_AUDIO_CPP_SOURCE_DIR "" CACHE PATH "Pristine audio.cpp checkout at the pinned commit")
set(IBAUDIO_PINNED_AUDIO_CPP_COMMIT "bb15edd78b56e035967e0eb999a6b28a62337db4" CACHE STRING "Reviewed audio.cpp pin")

find_package(Threads REQUIRED)

set(IBAUDIO_SOURCES
    src/audio.cpp
    src/backend_probe.cpp
    src/facade_util.cpp
    src/runtime.cpp
    src/session.cpp
    src/sha256.cpp
    src/stream.cpp)

if(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES)
    list(APPEND IBAUDIO_SOURCES
        src/innovation/prosody/prosody_controller.cpp
        src/innovation/turn/turn_manager.cpp
        src/innovation/conversation/conversation_state.cpp
        src/innovation/environment/environment_adapter.cpp
        src/innovation/voice/voice_clone_engine.cpp
        src/innovation/codeswitch/codeswitch_detector.cpp
        src/innovation/codec/neural_codec.cpp
        src/innovation/context/context_aware_output.cpp)
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
target_link_libraries(ibaudio PRIVATE Threads::Threads ${CMAKE_DL_LIBS})
target_compile_definitions(ibaudio
    PRIVATE
        $<$<BOOL:${IBAUDIO_ENABLE_VULKAN_PROBE}>:IBAUDIO_ENABLE_VULKAN_PROBE=1>
        $<$<BOOL:${IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER}>:IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=1>
        $<$<BOOL:${IBAUDIO_ENABLE_TEST_FIXTURE_MODELS}>:IBAUDIO_ENABLE_TEST_FIXTURE_MODELS=1>
    PUBLIC
        $<$<BOOL:${IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES}>:IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_API=1>)


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
    target_compile_definitions(ibaudio_cli PRIVATE
        $<$<BOOL:${IBAUDIO_ENABLE_TEST_FIXTURE_MODELS}>:IBAUDIO_ENABLE_TEST_FIXTURE_MODELS=1>)
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
    if(NOT IBAUDIO_ENABLE_TEST_FIXTURE_MODELS)
        message(FATAL_ERROR "IBAUDIO_BUILD_TESTS requires IBAUDIO_ENABLE_TEST_FIXTURE_MODELS=ON; production builds must keep fixtures OFF")
    endif()
    add_subdirectory(tests)
endif()

message(STATUS "InBharat Audio ${PROJECT_VERSION}: production runtime; deterministic ASR/TTS fixtures=${IBAUDIO_ENABLE_TEST_FIXTURE_MODELS}")
message(STATUS "research/heuristic innovation modules=${IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES} (must remain OFF for production)")
message(STATUS "audio.cpp adapter: ${IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER} (default build has no upstream dependency)")
message(STATUS "Vulkan: probe-only=${IBAUDIO_ENABLE_VULKAN_PROBE}; inference adapter intentionally unavailable")
