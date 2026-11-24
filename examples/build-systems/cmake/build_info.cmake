# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# build-info.txt: every key=value line is recorded in the evidence provenance.
# `compiler`, `flags_instr` and `flags_target` are the ones butterfly names in
# the baseline; anything else you add is carried through too. The compiler
# is recorded by the absolute path CMake resolved (CMAKE_C_COMPILER) and the
# target it compiles for, not just its banner (#245).
execute_process(COMMAND "${CC}" --version OUTPUT_VARIABLE _v ERROR_QUIET)
string(REGEX REPLACE "\n.*" "" _v "${_v}")
execute_process(COMMAND "${CC}" -dumpmachine OUTPUT_VARIABLE _t OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
file(WRITE "${OUT}" "compiler=${_v}\ncompiler_path=${CC}\ntarget=${_t}\nflags_instr=${INSTR}\nflags_target=${TARGET}\nllvm_bin=${LLVM_BIN}\nbuild_system=cmake\n")
# The other tools that shaped the evidence, by the path that ran (#289).
if(DSYMUTIL)
  file(APPEND "${OUT}" "dsymutil_path=${DSYMUTIL}\n")
endif()

# The line of `TOOL --version` that identifies it, as butterfly reads one
# (examples/adapter.sh, version_of): the first non-empty line with a digit,
# trimmed -- LLVM's tools print a URL banner first -- else the first line.
function(version_of tool out)
  execute_process(COMMAND "${tool}" --version OUTPUT_VARIABLE _o ERROR_QUIET)
  string(REGEX MATCH "[^\n]*[0-9][^\n]*" _l "${_o}")
  if(_l STREQUAL "")
    string(STRIP "${_o}" _l)
    string(REGEX REPLACE "\n.*" "" _l "${_l}")
  endif()
  string(STRIP "${_l}" _l)
  set(${out} "${_l}" PARENT_SCOPE)
endfunction()

# And what produced coverage.json, under the keys `butterfly build` writes
# (#298): the compiler of the instrumented target -- CMAKE_C_COMPILER, as
# for the analysed one -- and the llvm-profdata and llvm-cov that read its
# profile, by the path that ran and by version.
version_of("${PROFDATA}" _pv)
version_of("${COV}" _cv)
file(APPEND "${OUT}" "compiler_instr=${_v}\ncompiler_instr_path=${CC}\nllvm_profdata=${_pv}\nllvm_profdata_path=${PROFDATA}\nllvm_cov=${_cv}\nllvm_cov_path=${COV}\n")
