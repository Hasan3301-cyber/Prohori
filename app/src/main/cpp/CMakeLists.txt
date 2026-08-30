cmake_minimum_required(VERSION 3.31.6)
project(prohori_llama LANGUAGES C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT DEFINED PROHORI_LLAMA_SRC OR NOT EXISTS "${PROHORI_LLAMA_SRC}/CMakeLists.txt")
    message(FATAL_ERROR "PROHORI_LLAMA_SRC must point to the pinned llama.cpp checkout")
endif()

set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
# Package llama.cpp's Android CPU variants for arm64 and select the fastest compatible
# one at runtime. The previous single baseline took about a minute to prefill 38 tokens
# on the test phone. llama.cpp's variant set is not valid for the 32-bit v7a toolchain,
# so that compatibility ABI retains the portable statically linked CPU backend.
if(ANDROID_ABI STREQUAL "arm64-v8a")
    set(BUILD_SHARED_LIBS ON CACHE BOOL "" FORCE)
    set(GGML_BACKEND_DL ON CACHE BOOL "" FORCE)
    set(GGML_CPU_ALL_VARIANTS ON CACHE BOOL "" FORCE)
    set(GGML_CPU_KLEIDIAI ON CACHE BOOL "" FORCE)
    message(STATUS "prohori: ${ANDROID_ABI} CPU backend = runtime variants + KleidiAI (fast path)")
else()
    set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
    set(GGML_BACKEND_DL OFF CACHE BOOL "" FORCE)
    set(GGML_CPU_ALL_VARIANTS OFF CACHE BOOL "" FORCE)
    set(GGML_CPU_KLEIDIAI OFF CACHE BOOL "" FORCE)
    # Loud on purpose. A phone that lands here prefills at roughly a token a second, which the
    # app reports to its owner as a time limit rather than as a build mistake, so an arm64
    # slice must never be built this way by accident.
    message(WARNING "prohori: ${ANDROID_ABI} CPU backend = single static baseline (slow path)")
endif()
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
set(GGML_LLAMAFILE OFF CACHE BOOL "" FORCE)
set(LLAMA_CURL OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_COMMON OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TOOLS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_SERVER OFF CACHE BOOL "" FORCE)

add_subdirectory("${PROHORI_LLAMA_SRC}" llama-build EXCLUDE_FROM_ALL)

add_library(prohori_llama SHARED prohori_llama.cpp)
target_include_directories(
    prohori_llama
    PRIVATE
        "${PROHORI_LLAMA_SRC}/include"
        "${PROHORI_LLAMA_SRC}/ggml/include"
)
target_link_libraries(prohori_llama PRIVATE llama android log)
