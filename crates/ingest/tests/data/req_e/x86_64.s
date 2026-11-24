# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 Dhakshnamoorthy Durgaraj
# Hand-written x86-64 ELF input for crates/ingest/tests/req_e_elf_images.rs.
# Regenerate with ./build.sh. Every shape is the one clang emits.
        .text
# --- an exception table: one call site with a landing pad, one without ---
        .globl  thrower
        .type   thrower,@function
thrower:
        .cfi_startproc
        .cfi_personality 0x1b, personality
        .cfi_lsda 0x1b, .Lexception0
        pushq   %rbp
        .cfi_def_cfa_offset 16
.Lcs0_begin:
        callq   may_throw@PLT
.Lcs0_end:
.Lcs1_begin:
        callq   may_throw@PLT
.Lcs1_end:
        popq    %rbp
        retq
.Lpad0:
        popq    %rbp
        retq
        .cfi_endproc
        .size   thrower, .-thrower

        .type   personality,@function
personality:
        retq
        .size   personality, .-personality

# --- calls through GOT slots bound by R_X86_64_GLOB_DAT (-fno-plt) ---
        .globl  gotcaller
        .type   gotcaller,@function
gotcaller:
        pushq   %rax
        callq   *ext_fn@GOTPCREL(%rip)
        popq    %rax
        jmpq    *ext_tail@GOTPCREL(%rip)
        .size   gotcaller, .-gotcaller

# --- a switch through a table of 32-bit offsets from the table ---
        .globl  dispatch
        .type   dispatch,@function
dispatch:
        cmpl    $0x3, %edi
        ja      .Ldefault
        movl    %edi, %eax
        leaq    .Ltable(%rip), %rcx
        movslq  (%rcx,%rax,4), %rax
        addq    %rcx, %rax
        jmpq    *%rax
.Lc0:   movl    $10, %eax
        retq
.Lc1:   movl    $11, %eax
        retq
.Lc2:   movl    $12, %eax
        retq
.Lc3:   movl    $13, %eax
        retq
.Ldefault:
        xorl    %eax, %eax
        retq
        .size   dispatch, .-dispatch

# --- data coupling: lea then a read-modify-write through the register, and a
#     direct rip-relative store ---
        .globl  writer
        .type   writer,@function
writer:
        leaq    counter(%rip), %rax
        incl    (%rax)
        movl    $5, flag(%rip)
        movl    flag(%rip), %eax
        retq
        .size   writer, .-writer

        .section .rodata,"a",@progbits
        .p2align 2
.Ltable:
        .long   .Lc0-.Ltable
        .long   .Lc1-.Ltable
        .long   .Lc2-.Ltable
        .long   .Lc3-.Ltable

        .section .gcc_except_table,"a",@progbits
.Lexception0:
        .byte   0xff                    # @LPStart: omit
        .byte   0xff                    # @TType: omit
        .byte   0x01                    # call-site encoding: uleb128
        .uleb128 .Lcst_end0-.Lcst_begin0
.Lcst_begin0:
        .uleb128 .Lcs0_begin-thrower
        .uleb128 .Lcs0_end-.Lcs0_begin
        .uleb128 .Lpad0-thrower
        .byte   0
        .uleb128 .Lcs1_begin-thrower
        .uleb128 .Lcs1_end-.Lcs1_begin
        .byte   0                       # no landing pad: unwind further
        .byte   0
.Lcst_end0:

        .data
        .p2align 2
        .hidden counter
        .globl  counter
        .type   counter,@object
        .size   counter, 4
counter:
        .long   0
        .type   flag,@object
        .size   flag, 4
flag:
        .long   0

        .section .stack_sizes,"o",@progbits,.text
        .quad   thrower
        .uleb128 16
        .quad   writer
        .uleb128 0
