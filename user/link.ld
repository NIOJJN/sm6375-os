/* Пользовательский образ: линкуется по фиксированному адресу окна процесса (USER_BASE, см. ядро aspace.rs).
   Первая страница оставлена под заголовки ELF. */
ENTRY(_start)

SECTIONS
{
    . = 0x4000001000;

    .text : ALIGN(4096)
    {
        *(.text._start)
        *(.text .text.*)
    }

    .rodata : ALIGN(4096)
    {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096)
    {
        *(.data .data.*)
        *(.got .got.plt)
    }

    .bss : ALIGN(4096)
    {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : { *(.comment) *(.eh_frame*) *(.note*) *(.ARM.attributes) }
}
