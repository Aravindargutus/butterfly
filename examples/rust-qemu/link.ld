/* SPDX-License-Identifier: MIT OR Apache-2.0 */
/* Copyright (c) 2026 Dhakshnamoorthy Durgaraj */
/* QEMU's `virt` machine has RAM at 0x4000_0000 and puts the device tree at
   its base when given an ELF, so the image starts a little above it. */
ENTRY(_start)
SECTIONS
{
  . = 0x40080000;
  .text   : { KEEP(*(.text._start)) *(.text .text.*) }
  .rodata : { *(.rodata .rodata.*) }
  .data   : { *(.data .data.*) }
  .bss    : { *(.bss .bss.*) *(COMMON) }
  . = ALIGN(16);
  . += 0x10000;
  __stack_top = .;
  /DISCARD/ : { *(.comment) *(.ARM.exidx*) }
}
