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
    __limine_requests_end = .;

    /*
     * カーネルは Phase 2 で自前のページテーブルへ切り替える際、以下のシンボルで
     * 各セクションの範囲を知り W^X の権限でマップする (kernel/src/memory/kernel_space.rs)。
     * 各セクションの開始/終了はページ境界に揃える。
     */
    __text_start = .;
    .text : {
        *(.text .text.*)
    } :text
    . = ALIGN(CONSTANT(MAXPAGESIZE));
    __text_end = .;

    __rodata_start = .;
    .rodata : {
        *(.rodata .rodata.*)
        /* 例外テーブル: (フォルトし得る命令, 復帰先) の組 (kernel/src/arch/x86_64/probe.rs) */
        . = ALIGN(8);
        __yk_extable_start = .;
        KEEP(*(.yk_extable))
        __yk_extable_end = .;
    } :rodata
    . = ALIGN(CONSTANT(MAXPAGESIZE));
    __rodata_end = .;

    __data_start = .;
    .data : {
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data
    . = ALIGN(CONSTANT(MAXPAGESIZE));
    __data_end = .;

    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.interp)
    }
}
