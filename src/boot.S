// Точка входа AArch64. Заголовок arm64 Image (64 байта) для ABL/fastboot.
// LOAD_BASE = 0x80080000 (DDR на SM6375 начинается с 0x80000000, text_offset = 0x80000)

.section .text.boot, "ax"
.global _start
_start:
    b       _real_start             // code0
    .long   0                       // code1
    .quad   0x80000                 // text_offset (LOAD_BASE = 0x80080000)
    .quad   __image_size            // effective image size
    .quad   0x2                     // flags: little-endian, 4K pages
    .quad   0                       // res2
    .quad   0                       // res3
    .quad   0                       // res4
    .long   0x644d5241              // magic "ARM\x64"
    .long   0                       // res5

_real_start:
    mov     x19, x0                 // x19 = DTB pointer (callee-saved)
    msr     daifset, #0xf           // mask D, A, I, F

    // Skip MPIDR check (SM6375 CPU0 has Aff1=1)
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.eq    el2_entry
    cmp     x0, #1
    b.eq    el1_entry
    b       park

// ---- EL2 -> EL1 ----
el2_entry:
    mov     x0, #(1 << 31)          // HCR_EL2.RW = 1
    msr     hcr_el2, x0

    mrs     x0, cnthctl_el2
    orr     x0, x0, #3
    msr     cnthctl_el2, x0
    msr     cntvoff_el2, xzr

    mrs     x0, midr_el1
    msr     vpidr_el2, x0
    mrs     x0, mpidr_el1
    msr     vmpidr_el2, x0

    mov     x0, #0x32ff
    msr     cptr_el2, x0
    msr     hstr_el2, xzr
    msr     mdcr_el2, xzr

    // GICv3 system register access
    mrs     x0, id_aa64pfr0_el1
    ubfx    x0, x0, #24, #4
    cbz     x0, 1f
    mov     x0, #0xf
    msr     S3_4_C12_C9_5, x0       // ICC_SRE_EL2
    isb
1:
    mov     x0, #0x3c5              // SPSR_EL2: DAIF masked, EL1h
    msr     spsr_el2, x0
    adr     x0, el1_entry
    msr     elr_el2, x0
    eret

// ---- EL1 ----
el1_entry:
    // SCTLR_EL1: MMU/Caches OFF, alignment checks OFF
    mov     x0, #0x0800
    movk    x0, #0x30d0, lsl #16
    msr     sctlr_el1, x0
    isb

    // Stack
    adrp    x0, __stack_top
    add     x0, x0, :lo12:__stack_top
    mov     sp, x0

    // VBAR
    adrp    x0, vectors
    add     x0, x0, :lo12:vectors
    msr     vbar_el1, x0

    // FP/SIMD
    mov     x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb

    // Clear BSS
    adrp    x0, __bss_start
    add     x0, x0, :lo12:__bss_start
    adrp    x1, __bss_end
    add     x1, x1, :lo12:__bss_end
2:
    cmp     x0, x1
    b.hs    3f
    stp     xzr, xzr, [x0], #16
    b       2b
3:

    mov     x0, x19                 // DTB pointer
    bl      kernel_main

park:
    wfe
    b       park

// ==== Exception vectors ====
.macro ventry idx
    .balign 0x80
    mov     x0, #\idx
    b       exc_common
.endm

.macro ventry_irq
    .balign 0x80
    b       irq_entry
.endm

.macro ventry_irq_lower
    .balign 0x80
    b       irq_entry_lower
.endm

.macro ventry_sync_lower
    .balign 0x80
    b       sync_lower_entry
.endm

.section .text.vectors, "ax"
.balign 0x800
vectors:
    ventry  0
    ventry  1
    ventry  2
    ventry  3
    ventry  4
    ventry_irq
    ventry  6
    ventry  7
    ventry_sync_lower
    ventry_irq_lower
    ventry  10
    ventry  11
    ventry  12
    ventry  13
    ventry  14
    ventry  15

exc_common:
    // Crash signature: 0xDEADBEEF at known address
    // Используем адрес в пределах загруженного образа (после image_size)
    adrp    x0, __kernel_end
    add     x0, x0, #0x1000
    mov     x1, #0xDEAD
    movk    x1, #0xBEEF, lsl #16
    str     x1, [x0]

    mrs     x1, esr_el1
    mrs     x2, elr_el1
    mrs     x3, far_el1
    mov     x4, x30
    mrs     x5, spsr_el1
    mov     x6, x29
    bl      exception_handler
    b       park

.macro SAVE_FRAME
    sub     sp, sp, #0xC0
    stp     x0, x1, [sp, #0x00]
    stp     x2, x3, [sp, #0x10]
    stp     x4, x5, [sp, #0x20]
    stp     x6, x7, [sp, #0x30]
    stp     x8, x9, [sp, #0x40]
    stp     x10, x11, [sp, #0x50]
    stp     x12, x13, [sp, #0x60]
    stp     x14, x15, [sp, #0x70]
    stp     x16, x17, [sp, #0x80]
    stp     x18, x29, [sp, #0x90]
    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    stp     x30, x0, [sp, #0xA0]
    str     x1, [sp, #0xB0]
.endm

.macro RESTORE_FRAME
    ldr     x1, [sp, #0xB0]
    ldp     x30, x0, [sp, #0xA0]
    msr     elr_el1, x0
    msr     spsr_el1, x1
    ldp     x18, x29, [sp, #0x90]
    ldp     x16, x17, [sp, #0x80]
    ldp     x14, x15, [sp, #0x70]
    ldp     x12, x13, [sp, #0x60]
    ldp     x10, x11, [sp, #0x50]
    ldp     x8, x9, [sp, #0x40]
    ldp     x6, x7, [sp, #0x30]
    ldp     x4, x5, [sp, #0x20]
    ldp     x2, x3, [sp, #0x10]
    ldp     x0, x1, [sp, #0x00]
    add     sp, sp, #0xC0
    eret
.endm

irq_entry:
    SAVE_FRAME
    mov     x0, #0
    bl      irq_handler
    RESTORE_FRAME

irq_entry_lower:
    SAVE_FRAME
    mov     x0, #1
    bl      irq_handler
    RESTORE_FRAME

sync_lower_entry:
    SAVE_FRAME
    mov     x0, sp
    bl      sync_lower_handler
    RESTORE_FRAME

// ==== Secondary CPU entry (PSCI CPU_ON) ====
.section .text, "ax"
.global secondary_entry
secondary_entry:
    msr     daifset, #0xf
    mov     x19, x0
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.ne    1f

    // EL2 -> EL1
    mov     x2, #(1 << 31)
    msr     hcr_el2, x2
    mrs     x2, cnthctl_el2
    orr     x2, x2, #3
    msr     cnthctl_el2, x2
    msr     cntvoff_el2, xzr
    mrs     x2, midr_el1
    msr     vpidr_el2, x2
    mrs     x2, mpidr_el1
    msr     vmpidr_el2, x2
    mov     x2, #0x32ff
    msr     cptr_el2, x2
    msr     hstr_el2, xzr
    msr     mdcr_el2, xzr
    mrs     x2, id_aa64pfr0_el1
    ubfx    x2, x2, #24, #4
    cbz     x2, 2f
    mov     x2, #0xf
    msr     S3_4_C12_C9_5, x2
    isb
2:
    mov     x2, #0x3c5
    msr     spsr_el2, x2
    adr     x2, secondary_el1
    msr     elr_el2, x2
    eret

1:
secondary_el1:
    mov     x0, #0x0800
    movk    x0, #0x30d0, lsl #16
    msr     sctlr_el1, x0
    isb

    adrp    x1, __secondary_stacks
    add     x1, x1, :lo12:__secondary_stacks
    mov     x2, #0x8000
    madd    x1, x19, x2, x1
    add     x1, x1, x2
    mov     sp, x1

    adrp    x0, vectors
    add     x0, x0, :lo12:vectors
    msr     vbar_el1, x0
    mov     x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb

    mov     x0, x19
    bl      secondary_main
    b       park

.section .bss
.balign 16
.global __secondary_stacks
__secondary_stacks:
    .space 4 * 0x8000