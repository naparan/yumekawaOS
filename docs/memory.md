# メモリ管理 (Phase 2)

要件: [requirements.md](requirements.md) §8。実装: `kernel/src/memory/`。

## 1. 仮想アドレス空間レイアウト

4 段ページング (48bit, 正規形)。定数は `kernel/src/memory/layout.rs`。

```text
0x0000_0000_0000_0000 ┌──────────────────────────────┐
                      │ 下位半分 (PML4 0〜255)         │ プロセスごと (Phase 7 のユーザー空間)
                      │ カーネル空間の PML4 では空      │ ※セルフテストが一時的に user ページを置く
0x0000_7fff_ffff_ffff └──────────────────────────────┘
        (非正規形の穴)
0xffff_8000_0000_0000 ┌──────────────────────────────┐ PML4 256
                      │ HHDM: 物理 p ↔ 仮想 p + offset │ offset は Limine の値 (QEMU では左端)
                      │ RAM / ACPI / framebuffer のみ   │ 最大 64TiB (HHDM_LIMIT まで)
0xffff_c000_0000_0000 ├──────────────────────────────┤ PML4 384
                      │ カーネルヒープ (最大 64GiB)     │ 必要な分だけ 4KiB ページを追加
0xffff_c800_0000_0000 ├──────────────────────────────┤ PML4 400
                      │ カーネルスタック                │ 1MiB スロット × 1024
                      │  [ガード(未マップ) | スタック]    │
0xffff_d000_0000_0000 ├──────────────────────────────┤ PML4 416
                      │ カーネル仮想メモリ (匿名マップ)  │ vm::KernelMapping
0xffff_e000_0000_0000 ├──────────────────────────────┤ PML4 448
                      │ MMIO (UC)                     │ LAPIC / IOAPIC / (将来 HPET, PCI BAR)
0xffff_f000_0000_0000 ├──────────────────────────────┤
                      │ (未使用)                       │
0xffff_ffff_8000_0000 ├──────────────────────────────┤ PML4 511
                      │ カーネルイメージ (linker.ld)    │ .limine_requests / .text / .rodata / .data+.bss
0xffff_ffff_ffff_ffff └──────────────────────────────┘
```

上位半分の PML4 エントリ (256〜511) は、カーネル空間の構築時に **すべて PDPT を割り当てて**
埋めておき、以後変更しない (1MiB = 256 フレーム)。

- プロセスの PML4 (`AddressSpace::new_user`) は上位 256 エントリをコピーするだけでカーネル空間を共有でき、
  カーネル側でマップを追加・削除しても (PDPT より下の変更なので) 全プロセスに反映される。
- 上位半分の PDPT は解放しない (unmap で PD/PT が空になれば PD/PT は解放する)。

## 2. カーネルのページテーブル

`kernel_space::build_and_activate` が Limine のページテーブルから切り替える。Limine の HHDM と
**同じオフセット**で HHDM を作り直すので、切り替えの前後で HHDM 上のポインタ (framebuffer、
Limine のスタック、ページテーブル用フレーム) はそのまま有効。

| 領域 | ページ | 権限 | キャッシュ |
| --- | --- | --- | --- |
| `.limine_requests` | 4KiB | RW, NX | WB |
| `.text` | 4KiB | R, X | WB |
| `.rodata` (+ 例外テーブル `.yk_extable`) | 4KiB | R, NX | WB |
| `.data` / `.bss` | 4KiB | RW, NX | WB |
| HHDM: usable / bootloader-reclaimable | 1GiB → 2MiB → 4KiB (揃う範囲で最大) | RW, NX | WB |
| HHDM: kernel-and-modules (カーネルイメージの別名) | 同上 | **R**, NX | WB |
| HHDM: ACPI reclaimable / NVS | 同上 | R, NX | WB |
| HHDM: framebuffer | 同上 | RW, NX | **WC** |
| ACPI テーブルが reserved 領域にある場合 | 4KiB (必要時に追加) | R, NX | WB |
| MMIO 領域 | 4KiB | RW, NX | **UC** |

- **W^X**: 書き込み可能かつ実行可能なマップは `AddressSpace::map` / `protect` が拒否する。
  起動時に上位半分全体を監査 (`kernel_space::audit_wx`) し、結果をログに出す
  (QEMU: `3703 leaves, 43 executable, 0 writable+executable, 0 user`)。
  カーネルイメージの HHDM 側の別名も読み取り専用にしているので、`.text` を HHDM 経由で書き換えることもできない。
- **Global ページ**: カーネル空間の葉は G ビット付き (CR4.PGE)。CR3 を切り替えても TLB に残る。
- **PAT**: `IA32_PAT = 0x0007_0501_0007_0406` (index 4 を WC に変更)。PAT 非対応 CPU では WC の代わりに UC。
- **NX**: CPU が非対応なら NX ビットを立てない (予約ビット違反になるため)。ログに警告を出す。
- 大きいページの一部だけの unmap/remap (分割) は未対応 (`MapError::HugePageConflict`)。
- 切り替え前に「現在のスタック・カーネルコード・カーネルデータ・framebuffer が新しいテーブルでマップされているか」を
  `translate` で確認し、されていなければ切り替えずに FATAL で停止する。

QEMU (`-m 512M`) の実測: 4KiB=3083, 2MiB=250, 1GiB=0 ページ、ページテーブル 269 フレーム。
`-m 3G` + `pdpe1gb` では 1GiB ページが使われる (scripts/test-boot.sh の memory ケースで検証)。

## 3. 物理フレームアロケータ (`memory/frame.rs`)

- ビットマップ方式 (1 ビット = 4KiB、1 = 使用中)。512MiB で 16KiB。ビットマップ自身も
  usable 領域から切り出し、使用中にする。
- 確保可能なのは `usable` と、回収後の `bootloader-reclaimable`。**物理 0〜1MiB は確保しない**
  (null と区別するため、および将来の SMP の AP 起動トランポリン用)。
- API: `alloc()` / `alloc_zeroed()` / `alloc_contiguous(count, align_frames)` / `unsafe free(frame)` /
  `unsafe free_contiguous` / `is_allocated` / `stats()`。
- 解放時の検査: 確保可能な領域外 (`NotAllocatable`)・二重解放 (`DoubleFree`) をエラーにする。
  「まだ使われているフレームの解放」は検出できないので `free` は `unsafe fn`。
- 次の検索位置を覚える next-fit。連続確保は領域ごとに線形探索。

### bootloader-reclaimable の回収 (実施)

`boot::take()` が Limine の応答のうち必要な情報 (メモリマップ最大 512 領域、コマンドライン 512 バイト、
ブートローダ名/バージョン) を**カーネル内にコピー**するようにしたうえで、次の状態になってから回収する:

1. GDT / IDT がカーネルのもの (Limine の GDT を参照しない)
2. CR3 がカーネルのページテーブル
3. スタックがカーネルのスタック (Limine のスタックから移った後)
4. Limine の応答構造体をこれ以降参照しない (`take()` は 1 度しか成功しない)

QEMU では 41MiB (10546 フレーム) を回収している。ACPI reclaimable 領域は回収しない
(ACPI の解析結果・テーブルを後から参照できるようにするため)。

## 4. ページマッパ API (`memory/paging.rs`)

| API | safe? | 内容 |
| --- | --- | --- |
| `AddressSpace::new_kernel()` | safe | PML4 + 上位 256 個の PDPT を確保 |
| `AddressSpace::new_user(&kernel)` | safe | 上位半分を共有したプロセス用の空間 (Phase 7 用。セルフテストで切り替えまで検証) |
| `map(virt, phys, size, flags)` | `unsafe` | 任意の物理メモリを別名でマップし得るため |
| `unmap(virt)` | `unsafe` | 使用中のページを外し得るため。空になった中間テーブルは解放、`invlpg` |
| `protect(virt, flags)` | `unsafe` | 権限を落とすと使用中のコードに影響し得るため |
| `translate(virt)` | safe | 物理アドレス・ページサイズ・権限 |
| `activate()` | `unsafe` | CR3 のロード |
| `destroy()` | `unsafe` | ユーザー空間のテーブルを解放 |

上位層向けの safe API:

| API | 内容 |
| --- | --- |
| `vm::KernelMapping::new_anonymous(pages, flags)` | フレームを確保してカーネル仮想領域にマップ。drop でマップ解除 + フレーム解放。`as_mut_slice()` / `set_flags()` / `phys_of_page()` / `unmap()` |
| `kernel_space::translate(virt)` | カーネル空間の変換 |
| `kernel_space::audit_wx()` | W^X 監査 |
| `stack::allocate(name, size)` | ガード付きスタック |
| `unsafe mmio::map(phys, len)` → `MmioRegion` | 範囲・整列チェック付きの volatile `read_u32` / `write_u32` / `read_u64` |
| `unsafe map_firmware_readonly(phys, len)` | ACPI テーブル等を `&'static [u8]` として読む |

### TLB

単一 CPU (BSP) 前提。カーネル空間 (上位半分, Global) の変更、または現在の空間の変更では `invlpg` する。
`invlpg` は該当ページの TLB と全ページング構造キャッシュを無効化するので、中間テーブルの解放にも足りる。
**SMP 導入時は TLB shootdown (IPI) が必要。**

## 5. カーネルヒープ (`memory/heap.rs`)

- `#[global_allocator]`。`alloc` クレート (Box / Vec / String / format! 等) が使える。
- 空き管理は `linked_list_allocator` 0.10.6 (first-fit + 解放時の隣接結合)。外側を `IrqSpinLock` で保護。
- 初期 1MiB。確保に失敗したら `max(要求 + 余白, 1MiB)` をヒープ末尾に追加マップして `Heap::extend` し、再試行する。
  上限 64GiB (仮想領域)。
- **確保失敗時**: `GlobalAlloc::alloc` は null を返す。失敗しない API (`Box::new` 等) は Rust 既定の
  alloc error handler により `panic!("memory allocation of N bytes failed")` → Kernel Panic。
  `try_reserve` 等の fallible API はエラーを返す (セルフテスト `heap.alloc_failure_reported` で確認)。
  stable では `#[alloc_error_handler]` を定義できないため独自ハンドラは無い。
- **割り込みハンドラ内での確保は禁止** (debug ビルドでは `debug_assert!` で検出)。
- 統計: `heap::stats()` (mapped / size / used / free / grow_count / failed_allocs / peak_used)。

## 6. カーネルスタック (`memory/stack.rs`)

| スタック | 大きさ | 用途 |
| --- | --- | --- |
| `kernel-main` | 256KiB | kmain 以降のカーネル (Limine のスタックから移る) |
| `ist-double-fault` | 64KiB | #DF (IST1)。パニック画面の描画もここで行う |
| `ist-nmi` | 64KiB | NMI (IST2) |
| `ist-machine-check` | 64KiB | #MC (IST3) |

- 1MiB のスロットの上端にスタックを置き、下側 (768KiB 以上) は未マップのガード。
- x86_64-unknown-none は inline stack probe が有効なので、大きいスタックフレームでもガードを飛び越えない。
- **オーバーフローの検出**: ガードへのアクセスで #PF → 例外フレームを溢れたスタックに積めないので #DF →
  IST1 の専用スタックで #DF ハンドラが動き、CR2 / RSP がガード内なら
  `kernel stack overflow detected: stack 'kernel-main' ... ran into its guard page` として Kernel Panic。
  (scripts/test-boot.sh の stackoverflow ケースで検証)
- GDT/IDT の初期化直後はまだページング管理が無いので `.bss` の静的 IST スタック (16KiB, ガード無し) を使い、
  スタック確保後に `gdt::set_ist` でガード付きのものへ差し替える。
- スタックの解放は未実装 (起動時の 4 本だけ)。Phase 5 でタスクごとのスタックを作る際に追加する。
