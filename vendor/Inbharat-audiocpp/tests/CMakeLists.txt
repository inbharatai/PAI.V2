add_executable(ibaudio_tests test_main.cpp)
target_link_libraries(ibaudio_tests PRIVATE InBharat::Audio Threads::Threads)
target_compile_definitions(ibaudio_tests PRIVATE
    IBAUDIO_TEST_SOURCE_DIR="${CMAKE_CURRENT_SOURCE_DIR}"
    IBAUDIO_TEST_BINARY_DIR="${CMAKE_CURRENT_BINARY_DIR}")
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
endif()

option(IBAUDIO_BUILD_LIBFUZZER "Build LLVM libFuzzer WAV harness" OFF)
if(IBAUDIO_BUILD_LIBFUZZER AND CMAKE_CXX_COMPILER_ID MATCHES "Clang")
    add_executable(ibaudio_wav_fuzzer fuzz/wav_fuzzer.cpp)
    target_link_libraries(ibaudio_wav_fuzzer PRIVATE InBharat::Audio)
    target_compile_options(ibaudio_wav_fuzzer PRIVATE -fsanitize=fuzzer,address,undefined -fno-omit-frame-pointer)
    target_link_options(ibaudio_wav_fuzzer PRIVATE -fsanitize=fuzzer,address,undefined)
endif()

# Research/heuristic innovation modules are intentionally excluded from
# production. Their tests are built only when a developer explicitly enables
# the experimental surface.
if(IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES)
    add_executable(ibaudio_innovation_tests innovation/innovation_tests.cpp)
    target_link_libraries(ibaudio_innovation_tests PRIVATE InBharat::Audio)
    add_test(NAME ibaudio.innovation COMMAND ibaudio_innovation_tests)
    set_tests_properties(ibaudio.innovation PROPERTIES TIMEOUT 30 LABELS "rc;innovation")
endif()
