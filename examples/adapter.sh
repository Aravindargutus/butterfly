# shellcheck shell=sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# What every example build does to hand butterfly a directory it can trust,
# done one way (#245). Sourced by the example build scripts.
#
# Each of these used to be a line in every script ending in `|| true`: a
# missing .su, a dsymutil that failed, a build-info.txt that recorded the
# flags a script meant to pass rather than the ones it did. Each silently
# produced a bundle that looked complete. `butterfly ingest` now holds a
# build to what its build-info.txt declares, so the scripts must declare the
# truth -- and should stop here, naming the problem, before ingest has to.

# begin_output DIR [MORE_DIRS...]
#   Start DIR empty, and make every other directory named (DIR's own
#   subdirectories). DIR is where `butterfly ingest` is pointed, and it only
#   holds the evidence once end_output says the build finished. A step that
#   fails stops the script, but used to leave behind what came before it --
#   the coverage export, and often the linked image -- with no build-info.txt
#   to hold them to, which ingest accepted as a complete build (#286). So
#   until end_output runs, leaving the script for any reason moves DIR to
#   DIR.incomplete: kept for diagnosis, and named for what it is.
begin_output() {
  _bf_out="$1"
  rm -rf "$_bf_out" "$_bf_out.incomplete"
  mkdir -p "$@"
  trap _bf_set_aside EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
}

# end_output
#   The build finished, and DIR holds its evidence: every step ran, and
#   build-info.txt describes them. Call it last.
end_output() {
  _bf_done=1
}

_bf_set_aside() {
  [ -z "${_bf_done:-}" ] && [ -d "$_bf_out" ] || return 0
  mv "$_bf_out" "$_bf_out.incomplete" &&
    echo "build: stopped before $_bf_out was complete; what it made is in $_bf_out.incomplete" >&2
}

# compile_each OBJDIR COMPILER "FLAGS" SOURCE...
#   Compile each source to its own object. -fstack-usage names its .su after
#   the object, so one compile-and-link of several sources gave them all one
#   name -- each overwrote the last, and all but one unit's frames silently
#   fell back to prologue estimates. Objects are named from the whole source
#   path (src/alpha/dup.c -> src_alpha_dup.o), so two files of the same name
#   in different directories stay apart. Sets OBJECTS to the objects in
#   source order, so the link order -- and so the image -- is unchanged.
compile_each() {
  _obj="$1"; _cc="$2"; _fl="$3"; shift 3
  mkdir -p "$_obj"
  OBJECTS=""
  for _s in "$@"; do
    _o="$_obj/$(printf '%s' "${_s%.*}" | tr '/' '_').o"
    # shellcheck disable=SC2086 # the flags are a word list on purpose
    "$_cc" $_fl -c "$_s" -o "$_o" || exit 1
    OBJECTS="$OBJECTS $_o"
  done
  unset _obj _cc _fl _s _o
}

# collect_su OBJDIR DEST
#   Move every .su under OBJDIR into DEST. Two with the same name would have
#   one overwrite the other, and copy order would decide which frame is the
#   evidence; that stops the build. So does finding none: the caller asked
#   for stack usage, and the compiler wrote nothing.
collect_su() {
  _from="$1"; _to="$2"; _n=0
  _list=$(find "$_from" -name '*.su' | sort)
  while IFS= read -r _su; do
    [ -n "$_su" ] || continue
    _dest="$_to/$(basename "$_su")"
    if [ -e "$_dest" ]; then
      echo "build: two stack-usage files named $(basename "$_su") under $_from;" >&2
      echo "build: refusing to let copy order decide which one is the evidence. Give the objects distinct names." >&2
      exit 1
    fi
    mv "$_su" "$_dest"
    _n=$((_n + 1))
  done <<SU_LIST
$_list
SU_LIST
  if [ "$_n" -eq 0 ]; then
    echo "build: -fstack-usage was asked for, but no .su files were written under $_from" >&2
    exit 1
  fi
  unset _from _to _n _su _dest _list
}

# The tools other than the compiler that shaped the evidence, one
# `key=path` line each, for write_build_info. Started empty here, so nothing
# in the caller's environment can put a tool in the record that did not run.
_bf_tools=""

# record_tool KEY PATH
#   Name a tool in build-info.txt by the path that runs (#289). The compiler
#   was always recorded that way; the tools after it were not, so a directory
#   earlier in PATH could replace dsymutil or ar without a trace.
record_tool() {
  _bf_tools="$_bf_tools
$1=$2"
}

# version_of TOOL
#   The line of `TOOL --version` that identifies it, as butterfly reads one
#   (crates/ingest/src/provenance.rs): the first non-empty line with a digit
#   in it, trimmed -- LLVM's tools print a URL banner first -- else the first
#   non-empty line. Empty when TOOL gives no answer. Asked in the directory
#   the build runs its tools in, which is where it is called from.
version_of() {
  "$1" --version 2>/dev/null | awk '
    { sub(/^[[:space:]]+/, ""); sub(/[[:space:]]+$/, "") }
    $0 == "" { next }
    first == "" { first = $0 }
    /[0-9]/ { print; found = 1; exit }
    END { if (!found && first != "") print first }'
}

# record_coverage_compiler COMPILER
#   Name the compiler that built the instrumented binary -- the one that
#   wrote the profile -- by its banner and the path that ran (#298). The
#   keys are the ones `butterfly build` writes.
record_coverage_compiler() {
  _p=$(command -v "$1" 2>/dev/null || true)
  case "$_p" in /*) ;; *) _p="$1" ;; esac
  record_tool compiler_instr "$("$1" --version 2>/dev/null | head -1)"
  record_tool compiler_instr_path "$_p"
  unset _p
}

# record_profile_tools LLVM_PROFDATA LLVM_COV
#   Name the llvm-profdata that merged the profile and the llvm-cov that
#   exported coverage.json, by the path that ran and by version (#298).
#   Their formats are tied to one LLVM, so the pairing has to be checkable
#   from the evidence afterwards, not only refused at build time (#289).
record_profile_tools() {
  record_tool llvm_profdata "$(version_of "$1")"
  record_tool llvm_profdata_path "$1"
  record_tool llvm_cov "$(version_of "$2")"
  record_tool llvm_cov_path "$2"
}

# recorded_tools
#   Every line record_tool has collected, for a build that writes its
#   build-info.txt itself rather than through write_build_info.
recorded_tools() {
  [ -z "$_bf_tools" ] || printf '%s\n' "${_bf_tools#?}"
}

# resolve_tool COMPILER NAME
#   Set TOOL to the absolute path of NAME as COMPILER would run it
#   (-print-prog-name), else as PATH finds it, and record it as NAME_path.
#   Stops the build when there is none.
resolve_tool() {
  TOOL=$("$1" -print-prog-name="$2" 2>/dev/null || true)
  case "$TOOL" in /*) ;; *) TOOL=$(command -v "$2" 2>/dev/null || true) ;; esac
  case "$TOOL" in
    /*) [ -x "$TOOL" ] || { echo "build: $TOOL is not executable" >&2; exit 1; } ;;
    *) echo "build: $2 is not installed, and $1 cannot name one" >&2; exit 1 ;;
  esac
  record_tool "${2}_path" "$TOOL"
}

# link_debug COMPILER BINARY
#   On macOS a link of separate objects leaves the debug info in the .o files
#   behind a debug map, and only dsymutil turns it into the .dSYM butterfly
#   reads. Without it every instruction has no source line, so a dsymutil that
#   is missing or fails stops the build. It is the dsymutil COMPILER would run
#   itself for a one-step link, and build-info.txt names it. Elsewhere the
#   line table is in the binary and there is nothing to do.
link_debug() {
  if [ "$(uname -s)" = Darwin ]; then
    resolve_tool "$1" dsymutil
    "$TOOL" "$2" >/dev/null || { echo "build: $TOOL failed on $2" >&2; exit 1; }
  fi
}

# write_build_info OUT COMPILER FLAGS_INSTR FLAGS_TARGET [key=value ...]
#   build-info.txt, with the compiler's identity as well as its banner: the
#   absolute path of the one that ran (PATH decides which `clang` that is),
#   and the target it compiles for, and every tool record_tool named.
#   FLAGS_TARGET must be the flags the
#   analysed image was really built with -- butterfly holds the build to it.
write_build_info() {
  _out="$1"; _cc="$2"; _fi="$3"; _ft="$4"; shift 4
  _path=$(command -v "$_cc" 2>/dev/null || true)
  case "$_path" in /*) ;; *) _path="$_cc" ;; esac
  _target=$("$_cc" -dumpmachine 2>/dev/null || "$_cc" -vV 2>/dev/null | sed -n 's/^host: //p' || true)
  {
    printf 'compiler=%s\n' "$("$_cc" --version | head -1)"
    printf 'compiler_path=%s\n' "$_path"
    printf 'target=%s\n' "$_target"
    printf 'flags_instr=%s\n' "$_fi"
    printf 'flags_target=%s\n' "$_ft"
    printf 'build_system=%s\n' "${BUILD_SYSTEM:-shell}"
    recorded_tools
    for _kv in "$@"; do printf '%s\n' "$_kv"; done
  } > "$_out"
  unset _out _cc _fi _ft _path _target _kv
}
