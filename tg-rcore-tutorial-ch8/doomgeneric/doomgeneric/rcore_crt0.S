    .section .text._start
    .globl _start
    .type  _start, @function
_start:
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop

    /* The kernel already sets sp via from_elf (2 pages at top of user space).
       Override with a larger stack in BSS for Doom's deeper call chains. */
    la sp, __rcore_stack_top

    li a0, 0
    li a1, 0
    call main

    /* exit(main return value) */
    mv a0, a0
    li a7, 93
    ecall

    .section .bss.__rcore_stack,"aw",@nobits
    .align 4
    .globl __rcore_stack_bottom
    .globl __rcore_stack_top
__rcore_stack_bottom:
    .space 131072   /* 128 KiB stack */
__rcore_stack_top:
