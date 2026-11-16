# 割り込み・例外・タイマ・キーボード (Phase 2)

要件: [requirements.md](requirements.md) §9, §27, §30.2。実装: `kernel/src/arch/x86_64/{gdt,idt,trap,probe}.rs`,
`kernel/src/interrupts/`, `kernel/src/drivers/{apic,pic,pit,pm_timer,ps2}`, `kernel/src/time/`, `kernel/src/acpi/`。

## 1. GDT / TSS / IST

| セレクタ | 内容 | 備考 |
| --- | --- | --- |
| `0x00` | null | |
| `0x08` | kernel code (64bit, DPL0) | `STAR[47:32]=0x08` → syscall で CS=0x08, SS=0x10 |
| `0x10` | kernel data | |
| `0x18` | user code 32bit (DPL3) | `STAR[63:48]=0x18` の基準 (未使用) |
| `0x20` | user data (DPL3) | sysret で SS=`0x23` |
| `0x28` | user code 64bit (DPL3) | sysret で CS=`0x2B` |
| `0x30` | TSS (16 バイト) | IST1=#DF, IST2=NMI, IST3=#MC。RSP0 は Phase 7 で設定 |

## 2. 割り込みベクタの割り当て

| ベクタ | 用途 | ハンドラ |
| --- | --- | --- |
| `0x00`〜`0x1F` | CPU 例外 | `interrupts::exceptions` |
| `0x20`〜`0x2F` | 8259 PIC (リマップ後・全マスク) | スプリアス (IRQ7/15) を識別して無視。それ以外は警告 + EOI |
| `0x30`〜`0x3F` | ISA IRQ 0〜15 (I/O APIC 経由, `0x30 + IRQ`) | `0x31` = キーボード (IRQ1)、`0x3C` = マウス (IRQ12, Phase 3) |
| `0x40`〜`0xEF` | 予約 (PCI / MSI) | 未登録 → 警告 |
| `0xF0`〜`0xFC` | 予約 (IPI: スケジューラ・TLB shootdown) | 未登録 → 警告 |
| `0xFD` | Local APIC エラー | ESR をログ (最初の 8 回) |
| `0xFE` | Local APIC タイマ | `time` (tick + フック) |
| `0xFF` | Local APIC スプリアス | 数えるだけ (EOI しない) |

IDT は 256 ベクタすべてに `build.rs` が生成したスタブを登録する (Interrupt Gate, DPL0)。
スタブ → `yk_interrupt_common` (汎用レジスタ 15 個を保存) → `interrupts::dispatch(&mut TrapFrame)`。
カーネルは soft-float (SSE 無効) なので SSE/x87 状態は保存しない (Phase 7 でユーザーに FPU を使わせるなら XSAVE が必要)。

### ディスパッチと EOI

- Local APIC 経由の割り込みは、**ハンドラを呼ぶ前に** EOI を送る。送るのは Local APIC の ISR にそのベクタが
  立っているときだけ (ソフトウェア `int n` やスプリアスには送らない)。ハンドラは IF=0 で動くので
  先に EOI しても再入しない。ハンドラが EOI を忘れることも、Phase 5 でタイマから別タスクへ切り替えて
  戻らない場合に EOI が漏れることも無い。
- 未登録ベクタ: 回数を数え、最初の 16 回だけ警告ログ。ISR に立っていれば EOI。
- ハンドラ登録: `interrupts::register(vector, fn(&mut TrapFrame))` (0x30〜0xFE。二重登録はエラー)。

## 3. 例外

| 例外 | 処理 |
| --- | --- |
| #BP | `#BP breakpoint at rip=...` + レジスタを出して**復帰** (int3 の次の命令から継続) |
| NMI (IST2) | ロックを使わずシリアルへ出力して復帰 |
| #PF | 例外テーブル該当 → 回復。ガードページなら「スタックオーバーフロー」で Kernel Panic。それ以外は Kernel Panic (CR2・エラーコードの解読付き) |
| #GP | 例外テーブル該当 → 回復。それ以外は Kernel Panic (セレクタの解読付き) |
| #UD / #DE | 例外テーブル該当 → 回復。それ以外は Kernel Panic |
| #DF (IST1) | Kernel Panic。CR2 / RSP がスタックのガード内なら「kernel stack overflow detected」 |
| #MC (IST3) / その他 | Kernel Panic |

Phase 2 ではユーザー空間が無いので、すべてカーネルの例外として扱う。§9.2 の「該当タスクのみ停止」は、
Phase 5/7 で `frame.privilege_level() == 3` の例外をタスク終了に振り分ける形で追加する。

### 例外テーブル (プローブ)

`arch/x86_64/probe.rs`。Linux の exception table と同じく、フォルトし得る命令とその復帰先を
`.yk_extable` (rodata 内) に登録し、#PF/#GP/#UD/#DE ハンドラは RIP が表にあれば復帰先へ戻す。

- `probe::read_u8` / `read_u64` / `write_u8` (unsafe) / `read_u8_user` (stac/clac 付き, unsafe) /
  `exec` (unsafe, 命令フェッチのフォルトを捕捉) / `invalid_opcode` / `divide`
- 用途: セルフテスト (CR2・エラーコードの検証)、パニック時のバックトレース (壊れた rbp を辿っても落ちない)、
  Phase 7 の `copy_from_user` / `copy_to_user`。

## 4. Kernel Panic (§30.2)

`kernel/src/panic.rs` + `kernel/src/graphics/panic_screen.rs`。

1. 割り込みを無効化。`PANICKING` を立てる (2 回目以降は短いメッセージをシリアルに出して即停止 = 再パニックで無限再帰しない)
2. シリアルへ**ロックを使わず**出力: 種別・メッセージ・場所 (Rust のパニック) / エラーコード・RIP・CR2・全レジスタ (例外)・CPU 情報・uptime・メモリ統計 (`try_lock` で取れたときだけ)・バックトレース
3. 画面へ Kernel Panic 画面: エラー種別・メッセージ・エラーコード・RIP・CR2・バックトレース・CPU (ベンダー/ブランド/APIC ID)・状態・レジスタ・直近のカーネルログ (リングバッファ)
4. `system halted.` を出して `cli; hlt` ループ

framebuffer の所有: Phase 1 では kmain のローカル変数だった `Framebuffer` を、Phase 2 からは
`graphics::display` が `IrqSpinLock<Option<Framebuffer>>` で所有する。パニック時は `force_for_panic()`
で**保持者を無視して奪う** (保持者は割り込まれたコードか自分自身で、二度と再開しないため安全。描画は範囲チェック付き)。
ロガー (直近ログのリングバッファ) も同様に `force_lock` で読む。ヒープは使わない (固定長バッファのみ)。

バックトレースはフレームポインタ (`-C force-frame-pointers=yes`) を辿ったリターンアドレス。
シンボルは `llvm-addr2line -f -C -i -e build/kernel <アドレス>` で引ける (release は行情報のみ)。

## 5. ACPI と割り込みコントローラ

- **ACPI** (`acpi/mod.rs`): RSDP (シグネチャ・チェックサム、rev2 は拡張チェックサムも) → XSDT (無ければ RSDT) →
  各テーブルの長さ・チェックサムを検証。MADT (LAPIC アドレス、I/O APIC、Interrupt Source Override、LAPIC NMI、
  PCAT_COMPAT、CPU 数)、FADT (PM timer、IAPC_BOOT_ARCH の 8042 / legacy devices)、HPET (アドレスのみ) を読む。
  テーブルは HHDM 上で読み取り専用 (reserved 領域にあれば追加マップ)。AML は解釈しない。
  解析結果は `acpi::info()` で後から参照できる。
- **8259 PIC**: 0x20/0x28 にリマップして全 IRQ をマスク (PCAT_COMPAT が無くてもポート書き込みは無害なので常に実行)。
- **Local APIC**: `IA32_APIC_BASE` を正とし (MADT と違えば警告)、MMIO 領域に UC でマップ。ファームウェアが
  x2APIC モードにしていれば MSR でアクセスする (x2APIC への切り替え自体はしない)。
  TPR=0、LINT0 マスク、LINT1 は MADT の NMI 設定 (極性/トリガ) に従う、エラーベクタ 0xFD、SVR で有効化 (スプリアス 0xFF)。
- **I/O APIC**: MADT の全 I/O APIC をマップし、全エントリをマスク。`apic::route_isa_irq(madt, irq, vector, dest)` が
  Interrupt Source Override に従って GSI・極性・トリガを決め、物理宛先・固定配送で有効化する
  (書き換え中はマスクしてから上位→下位の順に書く)。

## 6. タイマ (`time/`)

- **校正**: Local APIC タイマ (16 分周, one-shot, マスク) を、参照クロックで 25ms 計る × 3 回の中央値。
  参照クロックは **ACPI PM timer** (3.579545MHz, FADT から。動作確認してから使う) → 無ければ **PIT チャネル 2**
  (ポーリング、IRQ 不要)。同時に TSC も校正する。すべてのポーリングに上限があり、参照クロックが無い実機でもハングしない。
  ログ例 (QEMU TCG):
  `calibrated with ACPI PM timer (3.579545 MHz, 24-bit, port 0x608): local APIC timer 62511917 Hz (bus / 16), samples [62558477, 62502397, 62511917] Hz, spread 897 ppm; TSC 2100 MHz; 1000 Hz tick = 62511 counts`
  (QEMU の APIC タイマは 1GHz/16 = 62.5MHz なので誤差は 0.02%)
- **周期割り込み**: 1000Hz (`TIMER_HZ`)、ベクタ 0xFE。
- **uptime**: `ticks() × counts_per_tick + 現 tick 内の経過カウント` を校正周波数で ns 換算。
  前回返した値を下回らない (単調非減少)。API: `time::ticks()`, `uptime_ns()`, `uptime_ms()`, `uptime() -> Duration`,
  `sleep(Duration)` (割り込み有効なら hlt で待つ), `Deadline::after(Duration)` (TSC ベース。割り込み無効でも使える)。
- **スケジューラのフック**: `time::set_tick_hook(Some(fn(&mut TrapFrame, ticks)))`。タイマ割り込みごとに
  (EOI 済み・IF=0 で) 呼ばれる。Phase 5 のプリエンプションはここから行う。Phase 2 ではダミーのスケジューラは置かない。
- **精度の注意**: uptime は tick の数え上げに基づくため、割り込み禁止区間が 1 tick (1ms) を超えると tick を取りこぼして
  遅れる。QEMU では検証済み (3 秒で PM timer との差 23ms 以内、ホスト時刻との差 20ms 程度) だが、実機で COM1 に大量の
  ログを出す (115200bps で 1 行 ≒ 数 ms, ロガーは IF=0 で出力) と遅れ得る。Phase 5 以降で TSC (invariant) /
  HPET を時刻源にする改善を検討する。

## 7. PS/2 キーボード (`drivers/ps2/`)

- i8042 の初期化: FADT の 8042 ビットが 0 または状態ポートが 0xFF なら「無し」。両ポート無効化 → バッファ空 →
  設定バイト (IRQ・変換オフ) → 自己診断 (0xAA→0x55, 設定を書き直す) → 2 ポート目の判定 (Phase 3 用に記録) →
  インターフェーステスト (0xAB/0xA9) → ポート 1 有効化 → キーボードリセット (0xFF → ACK, BAT 0xAA) →
  スキャンコードセット 2 を指定 (0xF0 0x02) → スキャン有効化 (0xF4) → 設定バイトで IRQ1 と**変換**を有効化。
  すべての待ちに TSC のタイムアウト (コマンド 50ms, リセット 1s)。失敗しても起動は継続 (警告)。
- **受け取るのはスキャンコードセット 1** (キーボードの set 2 をコントローラが変換したもの)。
- IRQ1 ハンドラは 1 回の割り込みで最大 16 バイト読み、ロックフリーの `SpscRing<256>` に積む
  (溢れたら捨てて数える。AUX (マウス) のバイトは捨てて数える)。
- Phase 2 では、アイドルループが `pop_scancode()` で取り出してログに出す:
  `keyboard: scancode 0x1e (make) [set 1]` / `0xe0 (prefix)` / `0xcd (break, extended)`。
