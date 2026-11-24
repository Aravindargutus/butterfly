# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The examples/c-lib library, built for butterfly by CMake instead of by
# build.sh. Same sources, same two builds, same artifacts -- so the evidence
# this produces should match what build.sh produces (see README.md).
#
# The shape to copy into your own project is the two targets: one instrumented
# and run for coverage, one clean and certified. They must come from the same
# source list, or the coverage you measured is not the code you analysed.
cmake_minimum_required(VERSION 3.20)
project(butterfly_cmake_example C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

# Where the library actually lives. Sharing one copy with the other examples
# is deliberate: three copies of the same library would drift, and the point
# here is that the *build system* changes while the sources do not.
set(LIB_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../c-lib")
set(SOURCES
    "${LIB_DIR}/src/util.c"
    "${LIB_DIR}/src/state.c"
    "${LIB_DIR}/src/mode.c"
    "${LIB_DIR}/src/crc.c"
    "${LIB_DIR}/src/sched.c"
    "${LIB_DIR}/src/main.c")

# butterfly disassembles the binary and needs the LLVM that matches the clang
# which wrote the profile: raw profiles carry a format version that has to
# agree exactly. Prefer an explicit -DLLVM_BIN=..., else the tools installed
# beside this clang. Only those count: without an llvm-profdata of its own,
# clang names one it found on PATH, from any LLVM, and this used to fall back
# to a bare `llvm-profdata` besides -- the pair unverified either way (#289).
if(NOT DEFINED LLVM_BIN)
  execute_process(COMMAND "${CMAKE_C_COMPILER}" -print-prog-name=llvm-profdata
                  OUTPUT_VARIABLE _profdata OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
  execute_process(COMMAND "${CMAKE_C_COMPILER}" --version
                  OUTPUT_VARIABLE _version ERROR_QUIET)
  set(_own "")
  if(_version MATCHES "InstalledDir: ([^\n]*)")
    set(_own "${CMAKE_MATCH_1}")
  endif()
  get_filename_component(_dir "${_profdata}" DIRECTORY)
  if(NOT IS_ABSOLUTE "${_profdata}" OR _own STREQUAL "" OR NOT _dir STREQUAL _own)
    message(FATAL_ERROR "${CMAKE_C_COMPILER} has no llvm-profdata of its own (installed in: ${_own}); "
                        "it named '${_profdata}', so the pair cannot be verified. Install the LLVM tools "
                        "that go with it, or pass -DLLVM_BIN=<the directory of the llvm-profdata that matches it>.")
  endif()
  set(LLVM_BIN "${_dir}")
endif()
set(PROFDATA "${LLVM_BIN}/llvm-profdata")
set(COV      "${LLVM_BIN}/llvm-cov")
# The dsymutil the compiler would run itself for a one-step link; recorded.
execute_process(COMMAND "${CMAKE_C_COMPILER}" -print-prog-name=dsymutil
                OUTPUT_VARIABLE DSYMUTIL OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)

set(COMMON_FLAGS -std=c11 -Wall -Wextra -g -O0)
set(INSTR_FLAGS  -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc)

# ---------------------------------------------------------------- 1. coverage
# Instrumented, run once, merged, exported. Nothing here is certified: it
# exists only to measure which source constructs were exercised.
add_executable(flight-instr ${SOURCES})
target_include_directories(flight-instr PRIVATE "${LIB_DIR}/include")
target_compile_options(flight-instr PRIVATE ${COMMON_FLAGS} ${INSTR_FLAGS})
# the instrumentation runtime has to be linked in as well as compiled in
target_link_options(flight-instr PRIVATE -fprofile-instr-generate)

# One OUTPUT, not two. Naming both the profile and run.txt makes two make
# targets share one recipe, and a parallel build can then run it twice at
# once -- which is how `coverage.json` came to be generated from a profile
# that was still being written.
add_custom_command(
  OUTPUT  "${CMAKE_BINARY_DIR}/flight.profraw"
  # LLVM_PROFILE_FILE names the raw profile; without it the program writes
  # default.profraw into whatever the working directory happens to be
  COMMAND ${CMAKE_COMMAND} -E env "LLVM_PROFILE_FILE=${CMAKE_BINARY_DIR}/flight.profraw"
          $<TARGET_FILE:flight-instr> > "${CMAKE_BINARY_DIR}/run.txt"
  BYPRODUCTS "${CMAKE_BINARY_DIR}/run.txt"
  DEPENDS flight-instr
  COMMENT "Running the instrumented build to produce a profile"
  VERBATIM)

add_custom_command(
  OUTPUT  "${CMAKE_BINARY_DIR}/coverage.json"
  COMMAND "${PROFDATA}" merge -sparse "${CMAKE_BINARY_DIR}/flight.profraw" -o "${CMAKE_BINARY_DIR}/flight.profdata"
  # No quotes inside the =value: VERBATIM quotes each argument for the shell
  # already, and quoting it here too reaches llvm-cov as a path with literal
  # `"` characters -- which it reports as "could not read profile data".
  COMMAND "${COV}" export -format=text -instr-profile=${CMAKE_BINARY_DIR}/flight.profdata
          $<TARGET_FILE:flight-instr> > "${CMAKE_BINARY_DIR}/coverage.json"
  BYPRODUCTS "${CMAKE_BINARY_DIR}/flight.profdata"
  DEPENDS "${CMAKE_BINARY_DIR}/flight.profraw" flight-instr
  COMMENT "llvm-profdata merge -> llvm-cov export -> coverage.json"
  VERBATIM)

# ---------------------------------------------------------- 2. the real build
# No instrumentation. This is the image that is certified and the one
# butterfly disassembles; the coverage above is only projected onto it.
add_executable(flight ${SOURCES})
target_include_directories(flight PRIVATE "${LIB_DIR}/include")
target_compile_options(flight PRIVATE ${COMMON_FLAGS} -fstack-usage)

# -fstack-usage drops each .su beside its object file, which for CMake means
# deep inside CMakeFiles/. butterfly looks for them in the ingest directory,
# so collect them where it will look.
add_custom_command(TARGET flight POST_BUILD
  COMMAND ${CMAKE_COMMAND}
          -DFROM=${CMAKE_BINARY_DIR}/CMakeFiles/flight.dir
          -DTO=${CMAKE_BINARY_DIR}
          -P "${CMAKE_CURRENT_SOURCE_DIR}/collect_su.cmake"
  COMMENT "Collecting .su stack-usage files"
  VERBATIM)

# On macOS a *link* of separate object files leaves the debug info in those
# .o files behind a debug map; only a single-step compile+link runs dsymutil.
# Without the dSYM the binary has no line table, every instruction lands in
# "no source line", and the projection is worthless. Harmless elsewhere.
if(APPLE)
  add_custom_command(TARGET flight POST_BUILD
    COMMAND "${DSYMUTIL}" $<TARGET_FILE:flight>
    COMMENT "dsymutil: the line table lives in the .o files until this runs"
    VERBATIM)
endif()

# --------------------------------------------------------------- 3. provenance
# What butterfly records so a reviewer can see which compiler and which flags
# produced the evidence.
string(JOIN " " _instr ${COMMON_FLAGS} ${INSTR_FLAGS})
string(JOIN " " _target ${COMMON_FLAGS} -fstack-usage)
add_custom_command(
  OUTPUT "${CMAKE_BINARY_DIR}/build-info.txt"
  COMMAND ${CMAKE_COMMAND}
          -DCC=${CMAKE_C_COMPILER} -DINSTR=${_instr} -DTARGET=${_target}
          -DLLVM_BIN=${LLVM_BIN} -DDSYMUTIL=$<$<BOOL:${APPLE}>:${DSYMUTIL}>
          -DPROFDATA=${PROFDATA} -DCOV=${COV}
          -DOUT=${CMAKE_BINARY_DIR}/build-info.txt
          -P "${CMAKE_CURRENT_SOURCE_DIR}/build_info.cmake"
  COMMENT "Recording compiler and flags in build-info.txt"
  VERBATIM)

# One target that produces everything `butterfly ingest` looks for.
add_custom_target(butterfly-evidence ALL
  DEPENDS flight "${CMAKE_BINARY_DIR}/coverage.json" "${CMAKE_BINARY_DIR}/build-info.txt")
