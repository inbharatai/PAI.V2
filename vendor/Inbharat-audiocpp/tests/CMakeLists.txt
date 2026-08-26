# CMake Release defines NDEBUG globally, but these tests intentionally use standard
# assert() for invariant checks. Undefine it for every C++/C test target in this
# directory so Release CTest executes the same assertions as sanitizer/debug lanes.
if(MSVC)
    add_compile_options(/UNDEBUG)
else()
    add_compile_options(-UNDEBUG)
endif()

add_executable(ibaudio_tests test_main.cpp)
target_link_libraries(ibaudio_tests PRIVATE InBharat::Audio Threads::Threads)
target_compile_definitions(ibaudio_tests PRIVATE
    IBAUDIO_TEST_SOURCE_DIR="${CMAKE_CURRENT_SOURCE_DIR}"
    IBAUDIO_TEST_BINARY_DIR="${CMAKE_CURRENT_BINARY_DIR}"
    $<$<BOOL:${IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER}>:IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=1>)
if(MSVC)
    target_compile_options(ibaudio_tests PRIVATE /W4 /permissive-)
else()
    target_compile_options(ibaudio_tests PRIVATE -Wall -Wextra -Wpedantic)
endif()
if(IBAUDIO_ENABLE_SANITIZERS AND NOT MSVC)
    target_compile_options(ibaudio_tests PRIVATE -fno-omit-frame-pointer -fsanitize=address,undefined)
    target_link_options(ibaudio_tests PRIVATE -fsanitize=address,undefined)
endif()

foreach(group IN ITEMS unit streaming lifecycle concurrency cancellation malformed stress fuzz)
    add_test(NAME ibaudio.${group} COMMAND ibaudio_tests ${group})
    set_tests_properties(ibaudio.${group} PROPERTIES TIMEOUT 120 LABELS "rc;${group}")
endforeach()

add_executable(ibaudio_abi_c_smoke abi_c_smoke.c)
target_link_libraries(ibaudio_abi_c_smoke PRIVATE InBharat::Audio)
target_compile_definitions(ibaudio_abi_c_smoke PRIVATE
    $<$<BOOL:${IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER}>:IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=1>)
set_target_properties(ibaudio_abi_c_smoke PROPERTIES LINKER_LANGUAGE CXX)
if(IBAUDIO_ENABLE_SANITIZERS AND NOT MSVC)
    target_compile_options(ibaudio_abi_c_smoke PRIVATE -fno-omit-frame-pointer -fsanitize=address,undefined)
    target_link_options(ibaudio_abi_c_smoke PRIVATE -fsanitize=address,undefined)
endif()
add_test(NAME ibaudio.abi-c99 COMMAND ibaudio_abi_c_smoke)
set_tests_properties(ibaudio.abi-c99 PROPERTIES TIMEOUT 30 LABELS "rc;abi")

if(IBAUDIO_BUILD_CLI)
    find_package(Python3 COMPONENTS Interpreter REQUIRED)
    add_test(NAME ibaudio.cli
        COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/cli_test.py"
                "$<TARGET_FILE:ibaudio_cli>" "${CMAKE_CURRENT_SOURCE_DIR}")
    set_tests_properties(ibaudio.cli PROPERTIES TIMEOUT 60 LABELS "rc;cli")
    add_test(NAME ibaudio.metadata
        COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/metadata_test.py"
                "$<TARGET_FILE:ibaudio_cli>" "${CMAKE_CURRENT_SOURCE_DIR}/..")
    set_tests_properties(ibaudio.metadata PROPERTIES TIMEOUT 30 LABELS "rc;metadata")
    add_test(NAME ibaudio.language-packs
        COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/../scripts/validate_22_language_packs.py")
    set_tests_properties(ibaudio.language-packs PROPERTIES TIMEOUT 30 LABELS "rc;language;packs")
    add_test(NAME ibaudio.language-pack-validator-security
        COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/test_language_pack_validator_security.py")
    set_tests_properties(ibaudio.language-pack-validator-security PROPERTIES TIMEOUT 60 LABELS "rc;language;packs;security")
    add_test(NAME ibaudio.india22-scorer
        COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/test_india22_scorer.py")
    set_tests_properties(ibaudio.india22-scorer PROPERTIES TIMEOUT 30 LABELS "rc;benchmark;india22;stt")
    add_test(NAME ibaudio.india22-tts-scorer
        COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/test_india22_tts_scorer.py")
    set_tests_properties(ibaudio.india22-tts-scorer PROPERTIES TIMEOUT 30 LABELS "rc;benchmark;india22;tts")
    if(IBAUDIO_BUILD_MCP)
        add_test(NAME ibaudio.mcp
            COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/mcp_test.py"
                    "$<TARGET_FILE:ibaudio_mcp>" "${CMAKE_CURRENT_SOURCE_DIR}/..")
        set_tests_properties(ibaudio.mcp PROPERTIES TIMEOUT 30 LABELS "rc;mcp;packs")
    endif()
endif()

option(IBAUDIO_BUILD_LIBFUZZER "Build LLVM libFuzzer WAV harness" OFF)
if(IBAUDIO_BUILD_LIBFUZZER AND CMAKE_CXX_COMPILER_ID MATCHES "Clang")
    add_executable(ibaudio_wav_fuzzer fuzz/wav_fuzzer.cpp)
    target_link_libraries(ibaudio_wav_fuzzer PRIVATE InBharat::Audio)
    target_compile_options(ibaudio_wav_fuzzer PRIVATE -fsanitize=fuzzer,address,undefined -fno-omit-frame-pointer)
    target_link_options(ibaudio_wav_fuzzer PRIVATE -fsanitize=fuzzer,address,undefined)
endif()

# Helper: link the sanitizer runtime into a test target when sanitizers are on.
# Required because these test executables don't inherit the library's link flags, and
# ASan refuses to run when its runtime isn't first in the initial library list.
function(ibaudio_test_sanitizers target)
    if(IBAUDIO_ENABLE_SANITIZERS AND NOT MSVC)
        target_compile_options(${target} PRIVATE -fno-omit-frame-pointer -fsanitize=address,undefined)
        target_link_options(${target} PRIVATE -fsanitize=address,undefined)
    endif()
endfunction()

# Bharat adaptation layer tests. The language module is internal (hidden visibility),
# so the test compiles the module source directly rather than linking the shared lib.
add_executable(ibaudio_language_tests language_tests.cpp ${CMAKE_CURRENT_SOURCE_DIR}/../src/language/language.cpp)
target_include_directories(ibaudio_language_tests PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/..)
ibaudio_test_sanitizers(ibaudio_language_tests)
add_test(NAME ibaudio.language COMMAND ibaudio_language_tests)
set_tests_properties(ibaudio.language PROPERTIES TIMEOUT 30 LABELS "rc;language")

# Native transport frame codec tests (internal module — compiles the source directly)
add_executable(ibaudio_transport_tests transport_tests.cpp ${CMAKE_CURRENT_SOURCE_DIR}/../src/transport/transport.cpp)
target_include_directories(ibaudio_transport_tests PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/..)
ibaudio_test_sanitizers(ibaudio_transport_tests)
add_test(NAME ibaudio.transport COMMAND ibaudio_transport_tests)
set_tests_properties(ibaudio.transport PROPERTIES TIMEOUT 30 LABELS "rc;transport")

# Stage-gated speech-to-speech orchestration tests. Provider-neutral internal module;
# test success, abstention, missing-stage refusal, confidence propagation and cancellation.
add_executable(ibaudio_speech_to_speech_tests
    speech_to_speech_tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/pipeline/speech_to_speech.cpp)
target_include_directories(ibaudio_speech_to_speech_tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/..
    ${CMAKE_CURRENT_SOURCE_DIR}/../include)
ibaudio_test_sanitizers(ibaudio_speech_to_speech_tests)
add_test(NAME ibaudio.speech-to-speech COMMAND ibaudio_speech_to_speech_tests)
set_tests_properties(ibaudio.speech-to-speech PROPERTIES TIMEOUT 30 LABELS "rc;pipeline;stt;tts;sts;cancellation")

# Reversible personal-adaptation store tests. Internal hidden module; compile it and
# SHA-256 directly to verify fingerprint/rollback behavior without exporting ABI.
add_executable(ibaudio_personal_adaptation_tests
    personal_adaptation_tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/adaptation/personal_adaptation.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/sha256.cpp)
target_include_directories(ibaudio_personal_adaptation_tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/..
    ${CMAKE_CURRENT_SOURCE_DIR}/../include
    ${CMAKE_CURRENT_SOURCE_DIR}/../src)
ibaudio_test_sanitizers(ibaudio_personal_adaptation_tests)
add_test(NAME ibaudio.personal-adaptation COMMAND ibaudio_personal_adaptation_tests)
set_tests_properties(ibaudio.personal-adaptation PROPERTIES TIMEOUT 30 LABELS "rc;language;adaptation;rollback")

# Bharat Speech Mesh evidence-router and output-arbitration tests. The mesh is an
# internal hidden-visibility module, so compile its source directly into the test.
add_executable(ibaudio_speech_mesh_tests
    speech_mesh_tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/mesh/speech_mesh.cpp)
target_include_directories(ibaudio_speech_mesh_tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/..
    ${CMAKE_CURRENT_SOURCE_DIR}/../include)
ibaudio_test_sanitizers(ibaudio_speech_mesh_tests)
add_test(NAME ibaudio.speech-mesh COMMAND ibaudio_speech_mesh_tests)
set_tests_properties(ibaudio.speech-mesh PROPERTIES TIMEOUT 30 LABELS "rc;mesh;routing")

# Hash-verified hot-swap language-pack registry tests. Internal hidden module; compile
# registry + SHA-256 implementation directly and point it at the generated pack root.
add_executable(ibaudio_pack_registry_tests
    pack_registry_tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/packs/pack_registry.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/sha256.cpp)
target_include_directories(ibaudio_pack_registry_tests PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/..
    ${CMAKE_CURRENT_SOURCE_DIR}/../include
    ${CMAKE_CURRENT_SOURCE_DIR}/../src)
target_compile_definitions(ibaudio_pack_registry_tests PRIVATE
    IBAUDIO_TEST_PACK_ROOT="${CMAKE_CURRENT_SOURCE_DIR}/../packs")
ibaudio_test_sanitizers(ibaudio_pack_registry_tests)
add_test(NAME ibaudio.pack-registry COMMAND ibaudio_pack_registry_tests)
set_tests_properties(ibaudio.pack-registry PROPERTIES TIMEOUT 30 LABELS "rc;language;packs;security")

# Provider registry / capability router contract tests. The router's remote-gate test
# exercises the internal registry directly with a stub provider, so only the registry
# internals (provider.cpp) are compiled in — the reference provider itself is already
# covered through the public-ABI roundtrips against the shared library.
add_executable(ibaudio_provider_tests
    provider_tests.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../src/provider.cpp)
target_include_directories(ibaudio_provider_tests PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_link_libraries(ibaudio_provider_tests PRIVATE InBharat::Audio)
target_compile_definitions(ibaudio_provider_tests PRIVATE
    $<$<BOOL:${IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER}>:IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=1>)
ibaudio_test_sanitizers(ibaudio_provider_tests)
add_test(NAME ibaudio.provider COMMAND ibaudio_provider_tests)
set_tests_properties(ibaudio.provider PROPERTIES TIMEOUT 30 LABELS "rc;provider")

# Innovation module tests (default-build heuristic modules)
add_executable(ibaudio_innovation_tests innovation/innovation_tests.cpp)
target_link_libraries(ibaudio_innovation_tests PRIVATE InBharat::Audio)
ibaudio_test_sanitizers(ibaudio_innovation_tests)
add_test(NAME ibaudio.innovation COMMAND ibaudio_innovation_tests)
set_tests_properties(ibaudio.innovation PROPERTIES TIMEOUT 30 LABELS "rc;innovation")

# Experimental research-module tests (only when the gated modules are compiled in)
if(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES)
    add_executable(ibaudio_innovation_experimental_tests innovation/innovation_experimental_tests.cpp)
    target_link_libraries(ibaudio_innovation_experimental_tests PRIVATE InBharat::Audio)
    target_compile_definitions(ibaudio_innovation_experimental_tests PRIVATE IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=1)
    ibaudio_test_sanitizers(ibaudio_innovation_experimental_tests)
    add_test(NAME ibaudio.innovation-experimental COMMAND ibaudio_innovation_experimental_tests)
    set_tests_properties(ibaudio.innovation-experimental PROPERTIES TIMEOUT 30 LABELS "experimental;innovation")
endif()

# Windows shared-build test deployment. The ibaudio DLL lands in <binary>/Release
# while these test executables land in <binary>/tests/Release; the Windows loader
# searches the executable's own directory first, so without a copy the test
# binaries fail to load the DLL (STATUS_DLL_NOT_FOUND 0xc0000135). Linux does not
# have this problem (RPATH/runpath). Copy the DLL next to each test binary as a
# post-build step so ctest is self-contained on Windows.
if(IBAUDIO_BUILD_SHARED AND WIN32)
    foreach(test_target
            ibaudio_tests ibaudio_abi_c_smoke ibaudio_language_tests
            ibaudio_transport_tests ibaudio_speech_to_speech_tests
            ibaudio_personal_adaptation_tests ibaudio_speech_mesh_tests
            ibaudio_pack_registry_tests ibaudio_provider_tests
            ibaudio_innovation_tests ibaudio_innovation_experimental_tests
            ibaudio_wav_fuzzer)
        if(TARGET ${test_target})
            add_custom_command(TARGET ${test_target} POST_BUILD
                COMMAND ${CMAKE_COMMAND} -E copy_if_different
                        "$<TARGET_FILE:ibaudio>" "$<TARGET_FILE_DIR:${test_target}>")
        endif()
    endforeach()
endif()
