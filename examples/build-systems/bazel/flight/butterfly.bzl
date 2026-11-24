# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# One rule that turns the two cc_binary targets into the directory
# `butterfly ingest` expects.
#
# Bazel keeps build outputs scattered through bazel-out and treats the sandbox
# as read-only, so unlike CMake or make there is no "build directory" to point
# butterfly at. This rule makes one.
#
# Every build option the evidence depends on is checked here, and the rule
# fails naming it, rather than producing a bundle that looks complete (#245):
#
#   -c dbg                  checked at analysis: fastbuild drops the debug info
#   --define llvm_bin=DIR   required: the profile tools must match the compiler
#   --spawn_strategy=local  checked in the action: under a sandbox the .su
#                           files and the objects dsymutil reads are discarded,
#                           so one .su per object is required and dsymutil
#                           must succeed
#   --strip=never           checked in the action: the image must still carry
#                           a line table
#
# What the build was is taken from Bazel itself, not from PATH or a repeated
# flag string: the compiler is the C++ toolchain's, and the flags recorded
# are the toolchain's effective command line for each target's own copts.

load("@rules_cc//cc:action_names.bzl", "ACTION_NAMES")
load("@rules_cc//cc:find_cc_toolchain.bzl", "find_cc_toolchain", "use_cc_toolchain")
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")

_CoptsInfo = provider(doc = "A cc target's own copts, read off the rule.", fields = ["copts"])

def _copts_aspect_impl(target, ctx):
    return [_CoptsInfo(copts = list(getattr(ctx.rule.attr, "copts", [])))]

_copts_aspect = aspect(
    implementation = _copts_aspect_impl,
    doc = "Reads the copts a cc_binary was declared with, so the recorded flags are the target's, not a copy.",
)

def _effective_flags(cc_toolchain, feature_configuration, copts):
    """The toolchain's C compile command line for these copts: its own defaults, then the target's."""
    variables = cc_common.create_compile_variables(
        cc_toolchain = cc_toolchain,
        feature_configuration = feature_configuration,
        user_compile_flags = copts,
    )
    return cc_common.get_memory_inefficient_command_line(
        feature_configuration = feature_configuration,
        action_name = ACTION_NAMES.c_compile,
        variables = variables,
    )

def _has_debug(flags):
    debug = False
    for f in flags:
        if f == "-g0":
            debug = False
        elif f.startswith("-g") and not f.startswith("-gno-"):
            debug = True
    return debug

def _butterfly_evidence_dir_impl(ctx):
    # ---- options checked before anything runs --------------------------------
    mode = ctx.var.get("COMPILATION_MODE", "")
    if mode != "dbg":
        fail("butterfly_evidence_dir: build with -c dbg (this is -c %s). Bazel's other modes drop or strip the debug information, so every instruction would land in \"no source line\"." % mode)
    llvm_bin = ctx.var.get("llvm_bin", "")
    if not llvm_bin:
        fail("butterfly_evidence_dir: pass --define llvm_bin=<dir of the llvm-profdata that matches the compiler>. A raw profile's format must match the clang that wrote it, and an action's PATH is not a statement of which one that is.")

    cc_toolchain = find_cc_toolchain(ctx)
    feature_configuration = cc_common.configure_features(
        ctx = ctx,
        cc_toolchain = cc_toolchain,
        requested_features = ctx.features,
        unsupported_features = ctx.disabled_features,
    )
    analysis_copts = ctx.attr.analysis_binary[_CoptsInfo].copts
    instr_copts = ctx.attr.instrumented_binary[_CoptsInfo].copts
    if "-fstack-usage" not in analysis_copts:
        fail("butterfly_evidence_dir: %s must be compiled with -fstack-usage; the .su files are part of the evidence this rule assembles." % ctx.attr.analysis_binary.label)
    flags_target = _effective_flags(cc_toolchain, feature_configuration, analysis_copts)
    flags_instr = _effective_flags(cc_toolchain, feature_configuration, instr_copts)
    if not _has_debug(flags_target):
        fail("butterfly_evidence_dir: %s's effective compile flags carry no debug information (no -g, or a later -g0): %s" % (ctx.attr.analysis_binary.label, " ".join(flags_target)))

    out = ctx.actions.declare_directory(ctx.label.name)
    instr = ctx.executable.instrumented_binary
    analysis = ctx.executable.analysis_binary

    ctx.actions.run_shell(
        inputs = depset([instr, analysis], transitive = [cc_toolchain.all_files]),
        outputs = [out],
        tools = [instr, analysis],
        # dsymutil (macOS) is found through the default environment; the LLVM
        # tools never are -- they come from llvm_bin or not at all
        use_default_shell_env = True,
        command = """
set -eu
OUT="$1"; INSTR="$2"; ANALYSIS="$3"; LLVM_BIN="$4"; CC="$5"
FLAGS_INSTR="$6"; FLAGS_TARGET="$7"; TOOLCHAIN="$8"; TARGET="$9"; CPU="${10}"
mkdir -p "$OUT"

PROFDATA="$LLVM_BIN/llvm-profdata"; COV="$LLVM_BIN/llvm-cov"; DWARFDUMP="$LLVM_BIN/llvm-dwarfdump"
for t in "$PROFDATA" "$COV" "$DWARFDUMP"; do
  [ -x "$t" ] || { echo "butterfly: $t is not there; --define llvm_bin must name the LLVM that matches the compiler" >&2; exit 1; }
done

# 1. run the instrumented build for a profile
LLVM_PROFILE_FILE="$OUT/flight.profraw" "$INSTR" > "$OUT/run.txt"
"$PROFDATA" merge -sparse "$OUT/flight.profraw" -o "$OUT/flight.profdata"
"$COV" export -format=text -instr-profile="$OUT/flight.profdata" "$INSTR" > "$OUT/coverage.json"

# 2. the image under analysis, beside it
cp "$ANALYSIS" "$OUT/flight"

# On macOS a link of separate objects leaves the debug info in the .o files
# behind a debug map. Under a sandbox those objects are gone by now, and
# dsymutil fails or writes an empty bundle: --spawn_strategy=local.
LINES="$OUT/flight"
if [ "$(uname -s)" = Darwin ]; then
  dsymutil "$OUT/flight" >/dev/null || { echo "butterfly: dsymutil failed on the analysis image; build with --spawn_strategy=local so the objects its debug map names still exist" >&2; exit 1; }
  LINES="$OUT/flight.dSYM"
fi
# Stripped, or the dSYM came out empty: every instruction would have no line.
if ! "$DWARFDUMP" --debug-line "$LINES" 2>/dev/null | grep -q '^Address'; then
  echo "butterfly: the analysis image has no line table. Build with -c dbg --strip=never (and --spawn_strategy=local on macOS)." >&2
  exit 1
fi

# 3. the .su files -- only the analysis target's own, in its own
# configuration, and one for every object. `find bazel-out` broadly would also
# sweep up other targets and stale configurations (review of #33); under a
# sandbox there are none at all, because nothing declared them.
# _objs holds one directory per target in the package, so the instrumented
# build's objects sit beside these: take only the analysis target's own.
SU_DIR="$(dirname "$ANALYSIS")/_objs/$(basename "$ANALYSIS")"
[ -d "$SU_DIR" ] || { echo "butterfly: no objects under $SU_DIR; build with --spawn_strategy=local" >&2; exit 1; }
OBJS=$(find "$SU_DIR" -name '*.o' | wc -l | tr -d ' ')
SUS=$(find "$SU_DIR" -name '*.su' | wc -l | tr -d ' ')
if [ "$SUS" -ne "$OBJS" ]; then
  echo "butterfly: $OBJS object(s) but $SUS .su file(s) under $SU_DIR. A sandbox discards the undeclared .su files: build with --spawn_strategy=local." >&2
  exit 1
fi
find "$SU_DIR" -name '*.su' | while IFS= read -r su; do
  dest="$OUT/$(basename "$su")"
  if [ -e "$dest" ]; then
    echo "butterfly: two stack-usage files named $(basename "$su") under $SU_DIR;" >&2
    echo "  refusing to let copy order decide which one is the evidence." >&2
    exit 1
  fi
  cp "$su" "$dest"
done || exit 1

# 4. provenance, from the toolchain Bazel actually used.
# The line of `TOOL --version` that identifies it, as butterfly reads one
# (examples/adapter.sh, version_of): the first non-empty line with a digit,
# trimmed -- LLVM's tools print a URL banner first -- else the first line.
version_of() {
  "$1" --version 2>/dev/null | awk '
    { sub(/^[[:space:]]+/, ""); sub(/[[:space:]]+$/, "") }
    $0 == "" { next }
    first == "" { first = $0 }
    /[0-9]/ { print; found = 1; exit }
    END { if (!found && first != "") print first }'
}
{
  printf 'compiler=%s\\n' "$("$CC" --version 2>/dev/null | head -1)"
  printf 'compiler_path=%s\\n' "$CC"
  printf 'toolchain_compiler=%s\\n' "$TOOLCHAIN"
  printf 'target=%s\\n' "$TARGET"
  printf 'target_cpu=%s\\n' "$CPU"
  printf 'compilation_mode=dbg\\n'
  printf 'flags_instr=%s\\n' "$FLAGS_INSTR"
  printf 'flags_target=%s\\n' "$FLAGS_TARGET"
  printf 'llvm_bin=%s\\n' "$LLVM_BIN"
  printf 'build_system=bazel\\n'
  # what produced coverage.json, under the keys `butterfly build` writes
  # (#298): the toolchain's compiler, which built the instrumented binary
  # as well, and the llvm-profdata and llvm-cov that read its profile
  printf 'compiler_instr=%s\\n' "$("$CC" --version 2>/dev/null | head -1)"
  printf 'compiler_instr_path=%s\\n' "$CC"
  printf 'llvm_profdata=%s\\n' "$(version_of "$PROFDATA")"
  printf 'llvm_profdata_path=%s\\n' "$PROFDATA"
  printf 'llvm_cov=%s\\n' "$(version_of "$COV")"
  printf 'llvm_cov_path=%s\\n' "$COV"
} > "$OUT/build-info.txt"
""",
        arguments = [
            out.path,
            instr.path,
            analysis.path,
            llvm_bin,
            cc_toolchain.compiler_executable,
            " ".join(flags_instr),
            " ".join(flags_target),
            cc_toolchain.compiler,
            cc_toolchain.target_gnu_system_name,
            cc_toolchain.cpu,
        ],
        mnemonic = "ButterflyEvidence",
        progress_message = "Assembling a butterfly ingest directory for %s" % ctx.label,
    )
    return [DefaultInfo(files = depset([out]))]

butterfly_evidence_dir = rule(
    implementation = _butterfly_evidence_dir_impl,
    doc = "Assembles the artifacts `butterfly ingest` expects into one directory, failing if any build option they depend on is missing.",
    attrs = {
        "instrumented_binary": attr.label(
            mandatory = True,
            executable = True,
            cfg = "target",
            aspects = [_copts_aspect],
            doc = "The -fprofile-instr-generate build, run to produce a profile.",
        ),
        "analysis_binary": attr.label(
            mandatory = True,
            executable = True,
            cfg = "target",
            aspects = [_copts_aspect],
            doc = "The clean, certified build: the one butterfly disassembles. Must use -fstack-usage.",
        ),
    },
    fragments = ["cpp"],
    toolchains = use_cc_toolchain(),
)
