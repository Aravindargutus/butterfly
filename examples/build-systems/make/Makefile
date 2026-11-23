# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
#
# The examples/c-lib library, built for butterfly by make instead of by
# build.sh. Same sources, same two builds, same artifacts -- so the evidence
# this produces should match what build.sh produces (see README.md).
#
# The shape to copy into your own project is the two builds: one instrumented
# and run for coverage, one clean and certified. They must come from the same
# source list, or the coverage you measured is not the code you analysed.

# `CC ?= clang` would NOT work: make predefines CC as `cc`, so `?=` never
# fires. On macOS cc happens to be clang and the mistake hides; on Linux cc is
# usually gcc, which has no -fprofile-instr-generate at all. Override only the
# built-in default, leaving `make CC=...` and an exported CC alone.
ifeq ($(origin CC),default)
CC := clang
endif
BUILD   ?= build
LIB     := ../../c-lib

# llvm-profdata must match the clang that wrote the profile: raw profiles
# carry a format version that has to agree exactly. Ask the compiler where its
# own tools are; override with `make LLVM_BIN=/usr/lib/llvm-18/bin`.
#
# Only an answer from the compiler's own installation counts: without an
# llvm-profdata of its own, clang names one it found on PATH, from any LLVM,
# and this used to fall back to a bare `llvm-profdata` besides. Either way the
# pair went unverified (#289), so that stops here, as examples/llvm-env.sh does.
ifeq ($(origin LLVM_BIN),undefined)
_PROFDATA := $(shell $(CC) -print-prog-name=llvm-profdata 2>/dev/null)
_OWN_BIN  := $(shell $(CC) --version 2>/dev/null | sed -n 's/^InstalledDir: //p')
LLVM_BIN  := $(patsubst %/,%,$(dir $(_PROFDATA)))
ifneq ($(MAKECMDGOALS),clean)
ifneq ($(LLVM_BIN),$(_OWN_BIN))
$(error $(CC) has no llvm-profdata of its own (installed in: $(or $(_OWN_BIN),unknown)); it named '$(_PROFDATA)', so the pair cannot be verified. Install the LLVM tools that go with it, or pass LLVM_BIN=<the directory of the llvm-profdata that matches it>)
endif
endif
endif
PROFDATA := $(LLVM_BIN)/llvm-profdata
COV      := $(LLVM_BIN)/llvm-cov
# The dsymutil the compiler would run itself for a one-step link; recorded.
DSYMUTIL := $(shell $(CC) -print-prog-name=dsymutil 2>/dev/null)

NAMES   := util state mode crc sched main
SRC     := $(addprefix $(LIB)/src/,$(addsuffix .c,$(NAMES)))
OBJ     := $(addprefix $(BUILD)/obj/,$(addsuffix .o,$(NAMES)))

COMMON  := -std=c11 -Wall -Wextra -g -O0 -I$(LIB)/include
INSTR   := -fprofile-instr-generate -fcoverage-mapping -fcoverage-mcdc

# Everything `butterfly ingest` looks for in one directory.
.PHONY: all
all: $(BUILD)/coverage.json $(BUILD)/flight $(BUILD)/build-info.txt
	@echo "ok: $(BUILD) is ready to ingest"

# ------------------------------------------------------------ 1. coverage
# Instrumented, run once, merged, exported. Nothing here is certified: it
# exists only to measure which source constructs were exercised.
$(BUILD)/flight-instr: $(SRC) | $(BUILD)
	$(CC) $(COMMON) $(INSTR) $(SRC) -o $@

# LLVM_PROFILE_FILE names the raw profile; without it the program writes
# default.profraw into whatever the working directory happens to be.
$(BUILD)/flight.profraw: $(BUILD)/flight-instr
	LLVM_PROFILE_FILE=$@ ./$< > $(BUILD)/run.txt

$(BUILD)/coverage.json: $(BUILD)/flight.profraw
	$(PROFDATA) merge -sparse $< -o $(BUILD)/flight.profdata
	$(COV) export -format=text -instr-profile=$(BUILD)/flight.profdata $(BUILD)/flight-instr > $@

# -------------------------------------------------------- 2. the real build
# No instrumentation. This is the image that is certified and the one
# butterfly disassembles; the coverage above is only projected onto it.
#
# Compiled one translation unit at a time on purpose: -fstack-usage writes one
# .su per object, so a single compile-and-link of all six would give one .su
# for the lot and butterfly would fall back to estimating frames from the
# prologue for every function.
$(BUILD)/obj/%.o: $(LIB)/src/%.c | $(BUILD)
	$(CC) $(COMMON) -fstack-usage -c $< -o $@
	@# no `|| true`: a .su the build declared and did not write stops it (#245)
	@mv $(BUILD)/obj/$*.su $(BUILD)/$*.c.su

$(BUILD)/flight: $(OBJ)
	$(CC) $(COMMON) $(OBJ) -o $@
# On macOS a *link* of separate object files leaves the debug info in those
# .o files behind a debug map; only a single-step compile+link runs dsymutil.
# Without the dSYM the binary has no line table, every instruction lands in
# "no source line", and the projection is worthless. Harmless elsewhere.
# A dsymutil that is missing or fails stops the build rather than leaving an
# image with no line table (#245).
	@if [ "$$(uname -s)" = Darwin ]; then "$(DSYMUTIL)" $@ >/dev/null || { echo "make: $(DSYMUTIL) failed on $@" >&2; exit 1; }; fi

# --------------------------------------------------------------- 3. provenance
# Every key=value line is recorded in the evidence provenance, so a reviewer
# can see which compiler and which flags produced it.
$(BUILD)/build-info.txt: | $(BUILD)
	@printf 'compiler=%s\ncompiler_path=%s\ntarget=%s\nflags_instr=%s\nflags_target=%s\nllvm_bin=%s\nbuild_system=make\n' \
	  "$$($(CC) --version | head -1)" \
	  "$$(command -v $(CC))" \
	  "$$($(CC) -dumpmachine)" \
	  "$(COMMON) $(INSTR)" \
	  "$(COMMON) -fstack-usage" \
	  "$(LLVM_BIN)" > $@
	@if [ "$$(uname -s)" = Darwin ]; then printf 'dsymutil_path=%s\n' "$(DSYMUTIL)" >> $@; fi
# And what produced coverage.json, under the keys `butterfly build` writes
# (#298): the compiler of the instrumented build, and the llvm-profdata and
# llvm-cov that read its profile, by the path that ran and by version.
	@printf 'compiler_instr=%s\ncompiler_instr_path=%s\nllvm_profdata=%s\nllvm_profdata_path=%s\nllvm_cov=%s\nllvm_cov_path=%s\n' \
	  "$$($(CC) --version | head -1)" \
	  "$$(command -v $(CC))" \
	  "$$($(call VERSION_OF,$(PROFDATA)))" \
	  "$(PROFDATA)" \
	  "$$($(call VERSION_OF,$(COV)))" \
	  "$(COV)" >> $@

# The line of `TOOL --version` that identifies it, as butterfly reads one
# (examples/adapter.sh, version_of): the first non-empty line with a digit,
# trimmed -- LLVM's tools print a URL banner first -- else the first line.
VERSION_OF = "$(1)" --version 2>/dev/null | awk '{ sub(/^[[:space:]]+/, ""); sub(/[[:space:]]+$$/, "") } $$0 == "" { next } first == "" { first = $$0 } /[0-9]/ { print; found = 1; exit } END { if (!found && first != "") print first }'

$(BUILD):
	@mkdir -p $(BUILD)/obj

.PHONY: clean
clean:
	rm -rf $(BUILD)

# What butterfly does with the result, for copy-and-paste.
.PHONY: evidence
evidence: all
	butterfly ingest $(BUILD) --source-root $(LIB) -o $(BUILD)/evidence.json
