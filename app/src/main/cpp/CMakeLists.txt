cmake_minimum_required(VERSION 3.31.6)
project(prohori_llama LANGUAGES C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT DEFINED PROHORI_LLAMA_SRC OR NOT EXISTS "${PROHORI_LLAMA_SRC}/CMakeLists.txt")
    message(FATAL_ERROR "PROHORI_LLAMA_SRC must point to the pinned llama.cpp checkout")
endif()

# Portable CPU settings are intentional. They produce one binary per Android ABI without
# assuming the judge's phone has a particular ARM extension. A later measured device pack
# may add optimized variants, but the safety floor must load everywhere the APK installs.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
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
target_include_directories(prohori_llama PRIVATE "${PROHORI_LLAMA_SRC}/include")
target_link_libraries(prohori_llama PRIVATE llama android log)
