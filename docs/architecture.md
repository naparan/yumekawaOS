# yumekawaOS アーキテクチャ (Phase 2 時点)

本書は Phase 2 (Memory / Interrupt) 時点で**実装済みのもの**だけを記述する。将来の構成は
[requirements.md](requirements.md) §7, §33 を参照。詳細は次の 2 文書に分けている。

- [memory.md](memory.md) — 仮想アドレス空間レイアウト、ページテーブル (W^X)、フレームアロケータ、ヒープ、スタック
- [interrupts.md](interrupts.md) — GDT/IDT/IST、ベクタ割り当て、例外、Kernel Panic、ACPI/APIC、タイマ、キーボード

## 1. ブートフローと初期化順序

```text
[UEFI ファームウェア (OVMF / 実機)]
   │  CD-ROM: El Torito の EFI ブートイメージ / USB・HDD: GPT の EFI System Partition
   ▼
[Limine 11.4.1  EFI/BOOT/BOOTX64.EFI]
   │  /boot/kernel (ELF64) を 0xffffffff80000000 にマップ、HHDM・framebuffer を用意、
   │  Boot Services を終了し、Long Mode・割り込み無効・Limine のスタックで kmain へ
   ▼
[kmain]  (Limine のスタック / Limine のページテーブル)               kernel/src/main.rs
   1. log::init()                COM1 (ループバック試験)。失敗してもリングバッファには記録
   2. boot::take()               メモリマップ・文字列をカーネル内へコピー → cmdline 解釈
   3. gdt::init(), idt::init()   自前の GDT + TSS (IST は .bss の静的スタック) / 256 ベクタの IDT
   4. enable_protection()        CR0.WP, EFER.NXE, CR4.PGE / SMEP / SMAP / UMIP (CPUID で対応を確認)
   5. frame::init()              usable 領域のビットマップアロケータ (bootloader-reclaimable は保留)
   6. build_and_activate()       カーネルのページテーブル (W^X, HHDM, framebuffer=WC) + PAT → CR3
   7. display::install(fb)       framebuffer をグローバルに所有 (以降パニック画面を出せる)
   8. stack::allocate() ×4       IST ×3 (TSS を差し替え) + メイン 256KiB、それぞれガード付き
      switch_stack()             メインスタックへ移って kernel_entry へ (戻らない)
[kernel_main]  (カーネルのスタック)
   9. frame::reclaim()           bootloader-reclaimable を回収 (QEMU で 41MiB)
  10. heap::init()               #[global_allocator] (1MiB から自動拡張)
  11. acpi::parse()              RSDP → XSDT → MADT / FADT / HPET (チェックサム検証)
      pic::remap_and_mask_all()  8259 を 0x20/0x28 へリマップして全マスク
      apic::init_local()         Local APIC (xAPIC MMIO / 既に有効なら x2APIC)
      apic::init_io()            I/O APIC (全エントリをマスク)
  12. time::calibrate()          LAPIC タイマを ACPI PM timer (無ければ PIT) で校正、TSC も
  13. ps2::init()                i8042 + キーボード (タイムアウト付き) → IRQ1 を I/O APIC で有効化
  14. draw_splash()              スプラッシュ (ステータス行は phase 2)
      time::start()              1000Hz 周期割り込み
      sti                        割り込み許可 → tick が進むことを確認 (500ms 以内に 5 tick)
      audit_wx()                 カーネル空間の W^X 監査をログへ
  15. selftest::run()            cmdline の selftest=... (テスト用)
      "YUMEKAWAOS_BOOT_OK phase=2"
      idle_loop()                cli → 仕事が無ければ sti; hlt / スキャンコードのログ / 10 秒ごとのハートビート
```

スプラッシュの描画は時間がかかり (framebuffer のロック保持中 = 割り込み禁止)、tick を取りこぼすため、
**タイマを開始する前**に描く。

失敗時の扱い:

| 状況 | 動作 |
| --- | --- |
| COM1 が無い (実機) | シリアル出力は破棄。リングバッファ (パニック画面の直近ログ) には残る |
| ブート情報が不正 / framebuffer が使えない / ページテーブル構築失敗 (手順 1〜8) | `[FATAL]` をシリアルへ出して停止 (まだ画面を所有していない) |
| ACPI (MADT) が無い / APIC 初期化失敗 / タイマ校正失敗 / tick が進まない | `panic!` → Kernel Panic 画面 |
| i8042 が無い・応答しない | 警告を出してキーボード無しで続行 (ステータス行に `(no keyboard)`) |
| 回復不能な例外 / パニック | Kernel Panic 画面 ([interrupts.md](interrupts.md) §4) |

## 2. ISO の構成 (scripts/make_iso.sh)

```text
yumekawaOS.iso (ISO9660 + Rock Ridge + Joliet + HFS+)
├── boot/
│   ├── kernel                      カーネル ELF
│   └── limine/
│       ├── limine.conf
│       ├── limine-uefi-cd.bin      FAT イメージ (中に EFI/BOOT/BOOTX64.EFI)
│       └── LICENSE.limine
└── EFI/BOOT/BOOTX64.EFI
```

`limine-uefi-cd.bin` が El Torito の EFI ブートイメージと GPT の EFI System Partition の両方として参照され、
同じ ISO が CD-ROM と USB メモリの両方で UEFI 起動できる (Phase 1 から変更なし)。

## 3. カーネルイメージ

`kernel/linker.ld` は各セクションの境界をページ境界に揃え、範囲をシンボルで公開する
(`__text_start/end`, `__rodata_start/end`, `__data_start/end`, `__limine_requests_end`)。
カーネルのページテーブルはこれを使ってセクションごとの権限 (W^X) でマップする。

| セクション | 権限 | 内容 |
| --- | --- | --- |
| `.limine_requests` | RW, NX | Limine リクエスト |
| `.text` | R, X | コード (割り込みスタブ `.text.yk_interrupt_vectors` を含む) |
| `.rodata` | R, NX | 定数、例外テーブル `.yk_extable`、割り込みスタブのアドレス表 |
| `.data` / `.bss` | RW, NX | 可変データ (静的 IST スタック、GDT/TSS/IDT 等) |

`kernel/build.rs` は 256 個の割り込みエントリスタブ (アセンブリ) を生成し、`trap.rs` が `global_asm!` で取り込む。

## 4. モジュール構成

```text
main.rs (kmain / kernel_main / idle_loop)
  ├── log            ── drivers::serial                        ログ (シリアル + リングバッファ)
  ├── boot           ── (limine クレート)                       ブート情報 (コピー済み)
  ├── arch::x86_64   cpu / registers / port / gdt / idt / trap / probe
  ├── memory         addr / layout / hhdm / frame / paging / kernel_space / vm / mmio / stack / heap
  ├── acpi           RSDP / XSDT / MADT / FADT
  ├── interrupts     dispatch / register / exceptions / vectors
  ├── drivers        serial / pic / apic / pit / pm_timer / ps2 (keyboard)
  ├── time           校正・tick・uptime・フック
  ├── graphics       framebuffer / font / color / splash / display / panic_screen
  ├── panic          Kernel Panic (#[panic_handler] / 例外)
  ├── sync           IrqSpinLock / Once / SpscRing
  ├── selftest       memory / exceptions / timer (cmdline で起動)
  └── fmtbuf         StackString
```

| モジュール | 主な公開 API |
| --- | --- |
| `arch::x86_64::cpu` | `enable_interrupts`, `disable_interrupts`, `without_interrupts`, `enable_interrupts_and_halt`, `halt_forever`, `breakpoint`, `software_interrupt::<N>`, `rdtsc`, `CpuIdentity`, `CpuFeatures` |
| `arch::x86_64::probe` | `read_u8`, `read_u64`, `unsafe write_u8`, `unsafe read_u8_user`, `unsafe exec`, `invalid_opcode`, `divide`, `Fault` |
| `memory::frame` | `alloc`, `alloc_zeroed`, `alloc_contiguous`, `unsafe free`, `unsafe free_contiguous`, `is_allocated`, `stats` |
| `memory::paging` | `AddressSpace` (`new_kernel`/`new_user`/`unsafe map`/`unsafe unmap`/`unsafe protect`/`translate`/`unsafe activate`/`unsafe destroy`), `PageFlags`, `CacheMode`, `MapError` |
| `memory::kernel_space` | `translate`, `audit_wx`, `new_user_space`, `kernel_pml4`, `unsafe activate_kernel_space` |
| `memory::vm` | `KernelMapping` (safe な匿名マッピング), `LowerHalfTestPage` (テスト専用) |
| `memory::mmio` | `unsafe map` → `MmioRegion::{read_u32, write_u32, read_u64}` |
| `memory::stack` | `allocate`, `find_guard` |
| `memory::heap` | `#[global_allocator]`, `stats`, `try_stats` |
| `acpi` | `parse`, `publish`, `info` → `AcpiInfo { madt, fadt, hpet_address, .. }` |
| `interrupts` | `register(vector, fn(&mut TrapFrame))`, `in_interrupt`, `count`, `stats` |
| `drivers::apic` | `local_apic`, `route_isa_irq`, `mask_gsi`, `eoi_if_in_service` |
| `drivers::ps2` | `init`, `info`, `send_to_port2`; `keyboard::{interrupt_handler, pop_scancode, stats}` |
| `time` | `ticks`, `uptime_ns`, `uptime_ms`, `uptime`, `sleep`, `Deadline`, `set_tick_hook`, `calibration` |
| `graphics::display` | `install`, `with(|fb| ...)`, `unsafe force_for_panic` |
| `log` | `trace!`〜`fatal!`, `set_min_level`, `recent`, `unsafe recent_for_panic` |
| `sync` | `IrqSpinLock`, `Once`, `SpscRing` |

### 4.1 Phase 1 からの公開 API の変更

| 変更 | 理由 |
| --- | --- |
| `BootInfo::cmdline`: `&'static str` → `BootString<512>` (`as_str()`) | 実体が bootloader-reclaimable 領域にあり、回収後にダングリングになるため。カーネル内にコピーする |
| `BootloaderInfo::{name, version}`: `&'static str` → `BootString<32>` | 同上 |
| `MemoryMap`: Limine の応答への参照 → 最大 512 領域のコピー (`iter()` 等の API は同じ) | 同上。512 を超えると `BootError::MemoryMapTooLarge` |
| `sync::SpinLock` を削除し `sync::IrqSpinLock` を追加 | ロガーを含め共有状態はすべて割り込みから触れうるため。割り込みを無効化しない SpinLock は使い道が無くなった (dead code になる) |
| `graphics::Framebuffer` に `Send` を実装 | `display` がグローバルに所有するため |
| `selftest=panic` の実行タイミング: cmdline 解釈時 → 初期化完了後 | パニック画面 (display) を検証するため。パニックの場所は引き続き `kernel/src/main.rs` |

## 5. ロック規約

| ロック | 型 | 保護する状態 | 割り込みハンドラから |
| --- | --- | --- | --- |
| `log::LOGGER` | IrqSpinLock | COM1 + 直近ログのリングバッファ (16KiB) | 使う (ハンドラ内は `try_lock`、取れなければ COM1 へ直接) |
| `frame::FRAMES` | IrqSpinLock | フレームアロケータ | 使わない |
| `kernel_space::KERNEL_SPACE` | IrqSpinLock | カーネルのページテーブル | 使わない |
| `heap::HEAP` | IrqSpinLock | ヒープ | **禁止** (debug_assert) |
| `display::DISPLAY` | IrqSpinLock | framebuffer | 使わない (パニック時のみ強制取得) |
| `apic::IOAPICS` | IrqSpinLock | I/O APIC の IOREGSEL/IOWIN | 使わない |
| `ps2::IO` | IrqSpinLock | i8042 のコマンド送信 | 使わない (IRQ1 は読むだけ) |
| `probe::LAST_FAULT` | IrqSpinLock | 直近の回復フォルト | 例外ハンドラは `try_lock` |
| `stack::RECORDS` | IrqSpinLock | スタックの一覧 | #DF/#PF ハンドラは `try_lock` |
| キーボードの受信バッファ | SpscRing (ロックフリー) | スキャンコード | 生産者 |

- `IrqSpinLock` は取得時に IF を保存して `cli`、解放時に元へ戻す。単一 CPU では
  「ロック保持中に割り込みが入り、ハンドラが同じロックで待つ」デッドロックが起きない。
- **ロック順序**: `HEAP → KERNEL_SPACE → FRAMES`。ロガーは最後に取る (どのロックを保持していてもログを出してよい)。
  ログ出力中に他のロックを取らない。
- NMI・致命的例外・パニック経路では `lock()` を使わない (`try_lock` / `force_lock` / `emergency_write`)。
- ロック保持中は割り込み禁止なので、長い処理 (全画面描画等) は分割すること。tick を取りこぼして uptime が遅れる。
- SMP (AP 起動) を導入するときは、パニック時に他 CPU を止めてから `force_lock` する必要がある。

## 6. unsafe の方針と所在 (§32)

`unsafe` は低レイヤーに限定し、**すべての unsafe ブロックに `// SAFETY:`** を付ける
(`clippy::undocumented_unsafe_blocks = deny`, `multiple_unsafe_ops_per_block = warn` で 1 ブロック 1 操作、
`unsafe_op_in_unsafe_fn = deny`)。上位層 (`graphics::{splash, font, panic_screen, color}`, `fmtbuf`,
`interrupts/vectors`, `drivers::ps2::keyboard`, `memory::{addr, layout, stack}` の公開 API) は safe な API だけを使う。

| 場所 | 内容 | 理由 |
| --- | --- | --- |
| `arch/x86_64/cpu.rs` | `cli`/`sti`/`hlt`/`pushfq`/`int3`/`int n`/`rdtsc` | CPU 命令 |
| `arch/x86_64/registers.rs` | CR0/2/3/4・MSR・`invlpg`・`wbinvd`・`stac`/`clac` (書き込み系は `unsafe fn`) | 制御レジスタ |
| `arch/x86_64/mod.rs` | `enable_protection` の CR0/CR4/EFER 書き込み | CPU 保護機能の有効化 |
| `arch/x86_64/port.rs` | `in`/`out` (8/32bit) | Port I/O |
| `arch/x86_64/gdt.rs` | GDT/TSS の静的領域への書き込み、`lgdt`/far return/`ltr`、`unsafe impl Sync` | CPU テーブル |
| `arch/x86_64/idt.rs` | IDT の書き込み、`lidt`、`unsafe impl Sync` | CPU テーブル |
| `arch/x86_64/trap.rs` | スタブ表の読み出し、`*mut TrapFrame` の参照化 | Interrupt Handler |
| `arch/x86_64/probe.rs` | 例外テーブルの読み出し、プローブ関数 (asm) の呼び出し | フォルトし得るアクセス |
| `memory/paging.rs` | ページテーブルエントリの volatile 読み書き、CR3 ロード、PAT | Page Table |
| `memory/frame.rs` | ビットマップのスライス化、フレームのゼロクリア、`unsafe fn free` | 物理メモリ |
| `memory/kernel_space.rs` | `map` 呼び出し (イメージ・HHDM・ACPI)、CR3 切り替え、rsp 読み出し | Page Table |
| `memory/vm.rs` / `stack.rs` / `heap.rs` | 確保したフレームの `map`/`unmap`、スライス化、`GlobalAlloc` | 所有権付きマッピング |
| `memory/mmio.rs` | MMIO の volatile 読み書き、`unsafe fn map` | MMIO |
| `memory/mod.rs` | `map_firmware_readonly` (`unsafe fn`) | ファームウェアのデータ |
| `drivers/{apic,pic,pit,pm_timer,ps2,serial}.rs` | ポート/MSR の所有、APIC の MMIO マップ | デバイス |
| `interrupts/mod.rs`, `time/mod.rs` | 登録済み関数ポインタの復元 (`transmute`) | ハンドラ表 |
| `interrupts/exceptions.rs` | 実行プローブの戻りアドレス読み出し | Interrupt Handler |
| `sync/irq_lock.rs`, `sync/once.rs` | `UnsafeCell`、`unsafe impl Sync`、`force_lock` | 同期 |
| `log.rs`, `graphics/display.rs`, `panic.rs` | パニック経路の `force_lock`、rbp 読み出し | Kernel Panic |
| `graphics/framebuffer.rs` | volatile 書き込み、`unsafe impl Send` | framebuffer |
| `boot/protocol.rs` | Limine リクエストの配置、framebuffer の書き込み権 | Bootloader interface |
| `main.rs` | `#[unsafe(no_mangle)]`、`switch_stack` (rsp の付け替え)、Handoff の読み出し、回収 | 起動処理 |
| `selftest/*.rs` | フレームの HHDM 経由の読み書き、プローブ、テスト用の map/unmap | テスト (低レイヤーの検証) |

## 7. ログ (§38)

書式: `[LEVEL] module: message`。既定の最小レベル: debug ビルド = TRACE、release = DEBUG
(`cmdline: loglevel=info` 等で変更)。シリアルに加えて 16KiB のリングバッファに記録し、
Kernel Panic 画面に直近のログとして表示する (シリアルの無い実機でも見える)。

## 8. テスト (scripts/test-boot.sh)

| ケース | 起動 | 検証 |
| --- | --- | --- |
| `cdrom` / `usb` | 通常の ISO / USB ストレージ | 起動完了マーカー、スプラッシュの画像検査、ERROR/例外ログが無い |
| `panic` | `selftest=panic` | 場所付きのパニック出力、パニック画面 (画像検査)、マーカーが出ない |
| `memory` | `selftest=memory`, `-m 3G`, `qemu64,+smep,+smap,+pdpe1gb` | 20 項目 (フレーム 6・ページング 7・ヒープ 7) が PASS、1GiB ページの使用 |
| `exceptions` | `selftest=exceptions`, `qemu64,+smep,+smap,+pdpe1gb` | 16 項目 (#BP 復帰・#UD・#DE・未登録ベクタ・#PF 読/書 (CR2)・#GP・W^X ×3・RX 実行・NX ×2・SMEP・SMAP・継続) が PASS、SKIP なし |
| `timer` | `selftest=timer` | tick の進行、uptime と PM timer (2% 以内)、単調性、ホストの実時間との比較 (15% + 250ms 以内) |
| `keyboard` | 通常 | `sendkey a b ret right` → `1e 9e 30 b0 1c 9c e0 4d e0 cd` (set 1) |
| `stackoverflow` | `selftest=stackoverflow` | #DF でのスタックオーバーフロー検出ログ、パニック画面 (画像検査) |
| `idle` | 通常 | 30 秒アイドルで ERROR/FATAL/例外/未登録割り込みが無く、ハートビートが 3 回 |

## 9. Phase 3 (Graphics / Font / Keyboard / Mouse) への引き継ぎ

- **framebuffer の所有**: `graphics::display::with(|fb: &mut Framebuffer| ...)` で借りる。Phase 3 の描画基盤
  (バックバッファ・フォントレンダラ) はこの中で framebuffer へ転送する形にする。ロック中は割り込み禁止なので、
  全画面の転送は行単位などに分割して tick の取りこぼしを避ける。framebuffer は WC でマップ済み
  (書き込みはまとめて行うと速い。読み出しは遅いので避ける)。パニック時は `panic_screen` が上書きする。
- **ヒープ**: `alloc` が使える (Box/Vec/String/BTreeMap 等)。バックバッファ (1280x800x4 = 4MiB) も確保可能
  (ヒープは自動拡張)。割り込みハンドラ内では確保しない。
- **キーボード**: `drivers::ps2::keyboard::pop_scancode()` が **set 1** のスキャンコードを 1 バイトずつ返す
  (割り込み安全なリングバッファ, 256 バイト)。Phase 3 はこの上に `0xE0`/`0xE1` プレフィックスと make/break (bit7)
  を解釈するデコーダ、キーマップ (US/JIS)、修飾キー状態、キーイベント API を作る。IRQ1 ハンドラは変更不要の想定。
  現在アイドルループがスキャンコードをログに出しているので、イベント API を作ったら置き換える。
- **マウス (IRQ12)**: `ps2::info()` の `dual_channel` / `port2_ok` でポート 2 の有無が分かる。有効化の手順:
  `ps2::send_to_port2(0xF6/0xF4 等)` でマウスを設定 → 設定バイトでポート 2 のクロックと IRQ12 を有効化
  (`ps2` に関数を追加) → `interrupts::register(vectors::MOUSE, handler)` →
  `apic::route_isa_irq(&acpi::info().unwrap().madt, 12, vectors::MOUSE, apic::local_apic().unwrap().id())`。
  IRQ1 ハンドラは AUX (マウス) のバイトを捨てて数えている (`kbd_aux`) ので、ポート 2 を有効化した後に
  マウスのバイトが IRQ1 側で読まれないか確認すること。
- **タイマ API**: `time::uptime()` / `uptime_ms()` / `ticks()` (1000Hz) / `sleep()` / `Deadline`。
  カーソル点滅・アニメーション・キーリピートに使える。
- **ログ**: `log::recent(&mut buf)` で直近のログを取り出せる (画面上のログビューア等に)。
- 8x8 フォント (`graphics::font`) は Phase 1 のまま。パニック画面もこれを使っている。

## 10. Phase 5 以降への注意

- **プリエンプション**: `time::set_tick_hook` からスケジューラを呼ぶ。EOI は既に送られている。
  コンテキスト切り替えで戻らない場合もディスパッチャの後処理 (`NESTING` の減算) が抜けるので、
  切り替え実装時に `interrupts::dispatch` の構造を合わせて見直すこと。
- **タスクのスタック**: `memory::stack::allocate` を使う (ガード付き)。解放 API の追加が必要。
- **ユーザー空間 (Phase 7)**: `kernel_space::new_user_space()` で上位半分を共有したアドレス空間が作れる
  (セルフテストで CR3 切り替えまで検証済み)。TSS.RSP0、`swapgs`、syscall/sysret (GDT の配置は対応済み)、
  FPU/SSE 状態の保存が必要。
- **SMAP と copy_from_user**: SMAP が有効な CPU では、カーネルが user ページに触れるのは
  `stac` 〜 `clac` の間だけにする。方針: ユーザーポインタへのアクセスは `probe.rs` と同様に
  「アセンブリで `stac; 1 命令ずつのコピー; clac`、フォルト時は例外テーブルの fixup で `clac` して -EFAULT」
  という専用関数 (`copy_from_user` / `copy_to_user`) に閉じ込め、それ以外のコードでは AC を立てない。
  なお割り込み・例外の配送では RFLAGS.AC はクリアされないため、`stac` 中に割り込まれるとハンドラは
  AC=1 (SMAP 無効) のまま動く。Phase 7 では割り込み入口 (`yk_interrupt_common`) で SMAP 対応 CPU に限り
  `clac` する (iretq で元の AC は復元される)。Phase 2 で `stac` を使うのはセルフテストの 1 命令だけ。
- **SMP**: TLB shootdown (IPI)、パニック時の他 CPU 停止、per-CPU の GDT/TSS/IST、`IrqSpinLock` は
  そのままスピンロックとして機能する (割り込み無効化は自 CPU のみ)。
- **時刻源**: uptime は tick の数え上げ。長い割り込み禁止区間で遅れる ([interrupts.md](interrupts.md) §6)。
