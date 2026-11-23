#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Same-named internal-linkage C++ functions in two translation units, plus a
# header `inline` compiled into all three: the C++ shapes of #36. -O0 so the
# copies survive as real symbols, and -fno-exceptions -fno-rtti to keep out
# object code with no source counterpart.
set -eu
cd "$(dirname "$0")"
# The tools that read the profile must come from the compiler that wrote
# it, so one variable names it and both the lookup and the build use that.
CXX="${CXX:-clang++}"
LLVM_FROM="$CXX"
. ../adapter.sh
. ../llvm-env.sh
begin_output build
COMMON="-std=c++17 -Wall -Wextra -g -Iinc -fno-exceptions -fno-rtti"
set -- $COMMON
"$CXX" "$@" -O0 -fprofile-instr-generate -fcoverage-mapping \
      src/geom.cpp src/engine.cpp src/main.cpp -o build/app-instr
( cd build && LLVM_PROFILE_FILE=app.profraw ./app-instr > run.txt )
"$LLVM_PROFDATA" merge -sparse build/app.profraw -o build/app.profdata
"$LLVM_COV" export -format=text -instr-profile=build/app.profdata build/app-instr > build/coverage.json
"$CXX" "$@" -O0 src/geom.cpp src/engine.cpp src/main.cpp -o build/app
# clang++ runs dsymutil itself when it compiles and links in one step, so the
# dSYM beside build/app already carries the debug info. Calling dsymutil again
# here produced an *empty* bundle and silently removed the line table. The
# one it ran is the one it names, and build-info.txt records that (#289).
if [ "$(uname -s)" = Darwin ]; then resolve_tool "$CXX" dsymutil; fi
write_build_info build/build-info.txt "$CXX" "$COMMON -O0 -fprofile-instr-generate -fcoverage-mapping" "$COMMON -O0"
end_output
echo "ok: $(ls build)"
