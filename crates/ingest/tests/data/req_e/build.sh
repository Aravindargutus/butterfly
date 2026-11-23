#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Regenerates the ELF images req_e_elf_images.rs reads. Needs clang and an
# ld.lld (rustup's llvm-tools ship one as gcc-ld/ld.lld). The images are
# committed so the suite runs on any host, with no linker. --no-relax keeps
# the instruction shapes clang emits (lld would turn adrp+add into nop+adr).
set -eu
cd "$(dirname "$0")"
LLD=${LLD:-$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/gcc-ld/ld.lld | head -1)}
for a in x86_64 aarch64; do
  clang -target "$a-unknown-linux-gnu" -c "$a.s" -o "$a.o"
  "$LLD" -shared --no-relax --hash-style=gnu -z noexecstack -o "$a.so" "$a.o"
  rm "$a.o"
done
