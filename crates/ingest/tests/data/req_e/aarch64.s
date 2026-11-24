// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Dhakshnamoorthy Durgaraj
// Hand-written AArch64 ELF input for crates/ingest/tests/req_e_elf_images.rs.
// Regenerate with ./build.sh. Every shape is the one clang emits.
        .text
// --- an exception table: one call site with a landing pad, one without ---
        .globl  thrower
        .type   thrower,@function
thrower:
        .cfi_startproc
        .cfi_personality 0x1b, personality
        .cfi_lsda 0x1b, .Lexception0
        stp     x29, x30, [sp, #-16]!
        .cfi_def_cfa_offset 16
.Lcs0_begin:
        bl      may_throw
.Lcs0_end:
.Lcs1_begin:
        bl      may_throw
.Lcs1_end:
        ldp     x29, x30, [sp], #16
        ret
.Lpad0:
        ldp     x29, x30, [sp], #16
        ret
        .cfi_endproc
        .size   thrower, .-thrower

        .type   personality,@function
personality:
        ret
        .size   personality, .-personality

// --- a call and a tail call through GOT slots bound by R_AARCH64_GLOB_DAT ---
        .globl  gotcaller
        .type   gotcaller,@function
gotcaller:
        stp     x29, x30, [sp, #-16]!
        adrp    x16, :got:ext_fn
        ldr     x16, [x16, :got_lo12:ext_fn]
        blr     x16
        ldp     x29, x30, [sp], #16
        adrp    x16, :got:ext_tail
        ldr     x16, [x16, :got_lo12:ext_tail]
        br      x16
        .size   gotcaller, .-gotcaller

// --- a switch: byte entries, scaled by four, from an adr base (clang -O2) ---
        .globl  dispatch
        .type   dispatch,@function
dispatch:
        cmp     w0, #3
        b.hi    .Ldefault
        mov     w8, w0
        adrp    x9, .Ltable
        add     x9, x9, :lo12:.Ltable
        adr     x10, .Lc0
        ldrb    w11, [x9, x8]
        add     x10, x10, x11, lsl #2
        br      x10
.Lc0:   mov     w0, #10
        ret
.Lc1:   mov     w0, #11
        ret
.Lc2:   mov     w0, #12
        ret
.Lc3:   mov     w0, #13
        ret
.Ldefault:
        mov     w0, wzr
        ret
        .size   dispatch, .-dispatch

// --- data coupling: adrp+add takes the address, adrp+ldr reads, adrp+str writes ---
        .globl  writer
        .type   writer,@function
writer:
        adrp    x8, counter
        add     x8, x8, :lo12:counter
        adrp    x9, flag
        ldr     w10, [x9, :lo12:flag]
        mov     w11, #5
        str     w11, [x9, :lo12:flag]
        ret
        .size   writer, .-writer

        .section .rodata,"a",@progbits
.Ltable:
        .byte   (.Lc0-.Lc0)>>2
        .byte   (.Lc1-.Lc0)>>2
        .byte   (.Lc2-.Lc0)>>2
        .byte   (.Lc3-.Lc0)>>2

        .section .gcc_except_table,"a",@progbits
.Lexception0:
        .byte   0xff
        .byte   0xff
        .byte   0x01
        .uleb128 .Lcst_end0-.Lcst_begin0
.Lcst_begin0:
        .uleb128 .Lcs0_begin-thrower
        .uleb128 .Lcs0_end-.Lcs0_begin
        .uleb128 .Lpad0-thrower
        .byte   0
        .uleb128 .Lcs1_begin-thrower
        .uleb128 .Lcs1_end-.Lcs1_begin
        .byte   0
        .byte   0
.Lcst_end0:

        .data
        .p2align 2
        .hidden counter
        .globl  counter
        .type   counter,@object
        .size   counter, 4
counter:
        .word   0
        .type   flag,@object
        .size   flag, 4
flag:
        .word   0

        .section .stack_sizes,"o",@progbits,.text
        .xword  thrower
        .uleb128 16
        .xword  writer
        .uleb128 0
