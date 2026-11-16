/*
 * yumekawaOS kernel linker script (x86_64, Limine boot protocol)
 *
 * カーネルは仮想アドレス 0xffffffff80000000 (上位 2GiB, code-model=kernel)
 * に配置する。Limine はこの ELF の PT_LOAD セグメントを物理メモリへ読み込み、
 * PHDRS の FLAGS に従った権限でマップしてからエントリ (kmain) へジャンプする。
 */
OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kmain)

PHDRS
{
    /* FLAGS: PF_X = 1, PF_W = 2, PF_R = 4 */
    limine_requests PT_LOAD FLAGS(6);  /* RW: Limine が応答ポインタを書き込む */
    text            PT_LOAD FLAGS(5);  /* RX */
    rodata          PT_LOAD FLAGS(4);  /* R  */
    data            PT_LOAD FLAGS(6);  /* RW */
}

SECTIONS
{
    . = 0xffffffff80000000;
    __kernel_start = .;

    /* Limine のリクエストは開始/終了マーカーの間に置く */
    .limine_requests : {
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
    } :limine_requests

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .data : {
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.interp)
    }
}
