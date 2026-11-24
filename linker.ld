ENTRY(_start)

SECTIONS
{
    . = DEFINED(__load_base) ? __load_base : 0x40080000;

    .text : ALIGN(4096)
    {
        __kernel_start = .;
        __text_start = .;
        KEEP(*(.text.boot))
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    }

    .rodata : ALIGN(4096)
    {
        __rodata_start = .;
        *(.rodata .rodata.*)
        . = ALIGN(4096);
        __rodata_end = .;
    }

    .data : ALIGN(4096)
    {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.plt)
        . = ALIGN(4096);
    }

    .bss (NOLOAD) : ALIGN(4096)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    .stack (NOLOAD) : ALIGN(4096)
    {
        . += 0x10000;
        __stack_top = .;
        __kernel_end = .;
    }

    __image_size = ABSOLUTE(__kernel_end) - ABSOLUTE(__kernel_start);

    /DISCARD/ : { *(.comment) *(.eh_frame*) *(.note*) *(.ARM.attributes) }
}
