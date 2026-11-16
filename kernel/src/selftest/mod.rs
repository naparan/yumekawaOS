//! 起動時セルフテスト (カーネルコマンドライン `selftest=...`)。
//!
//! scripts/test-boot.sh が検証用 ISO の cmdline に `selftest=<名前>` を設定して起動し、
//! シリアル出力の次の行を機械的に検査する:
//!
//! ```text
//! SELFTEST <suite>.<item>: PASS (詳細)
//! SELFTEST <suite>.<item>: FAIL (詳細)
//! SELFTEST <suite>.<item>: SKIP (理由)
//! SELFTEST_DONE suite=<suite> passed=<n> failed=<n> skipped=<n>
//! ```
//!
//! | 名前 | 内容 |
//! | --- | --- |
//! | `memory` | フレームアロケータ・ページマッパ・ヒープ ([`memory`]) |
//! | `exceptions` | #BP からの復帰・#PF・#GP・W^X・NX・SMEP/SMAP ([`exceptions`]) |
//! | `timer` | tick の進行・uptime と参照クロックの比較 ([`timer`]) |
//! | `stackoverflow` | カーネルスタックのオーバーフロー → #DF → Kernel Panic 画面 (戻らない) |
//! | `panic` | `panic!` → Kernel Panic 画面 (戻らない。main.rs で実行) |

mod exceptions;
mod memory;
mod timer;

use core::fmt;

use crate::arch::x86_64::ProtectionFeatures;
use crate::arch::x86_64::cpu::CpuFeatures;

/// 実行するセルフテストの組。
#[derive(Debug, Clone, Copy, Default)]
pub struct Selection {
    /// メモリ管理
    pub memory: bool,
    /// 例外・保護
    pub exceptions: bool,
    /// タイマ
    pub timer: bool,
    /// スタックオーバーフロー (致命的)
    pub stack_overflow: bool,
    /// パニック (致命的)
    pub panic: bool,
}

impl Selection {
    /// 名前で有効化する。未知の名前なら `false`。
    pub fn enable(&mut self, name: &str) -> bool {
        match name {
            "memory" => self.memory = true,
            "exceptions" => self.exceptions = true,
            "timer" => self.timer = true,
            "stackoverflow" => self.stack_overflow = true,
            "panic" => self.panic = true,
            _ => return false,
        }
        true
    }
}

/// 1 つのスイートの結果集計。
pub struct Suite {
    name: &'static str,
    passed: u32,
    failed: u32,
    skipped: u32,
}

impl Suite {
    fn new(name: &'static str) -> Self {
        info!("SELFTEST_BEGIN suite={name}");
        Self {
            name,
            passed: 0,
            failed: 0,
            skipped: 0,
        }
    }

    /// 結果を記録する。
    fn check(&mut self, item: &str, ok: bool, detail: fmt::Arguments<'_>) {
        if ok {
            self.passed += 1;
            info!("SELFTEST {}.{item}: PASS ({detail})", self.name);
        } else {
            self.failed += 1;
            error!("SELFTEST {}.{item}: FAIL ({detail})", self.name);
        }
    }

    /// スキップを記録する。
    fn skip(&mut self, item: &str, reason: &str) {
        self.skipped += 1;
        info!("SELFTEST {}.{item}: SKIP ({reason})", self.name);
    }

    fn finish(self) {
        info!(
            "SELFTEST_DONE suite={} passed={} failed={} skipped={}",
            self.name, self.passed, self.failed, self.skipped
        );
    }
}

/// 選択されたセルフテストを実行する (stackoverflow は戻らない)。
pub fn run(sel: Selection, features: &CpuFeatures, protection: &ProtectionFeatures) {
    if sel.memory {
        let mut s = Suite::new("memory");
        memory::run(&mut s);
        s.finish();
    }
    if sel.exceptions {
        let mut s = Suite::new("exceptions");
        exceptions::run(&mut s, features, protection);
        s.finish();
    }
    if sel.timer {
        let mut s = Suite::new("timer");
        timer::run(&mut s);
        s.finish();
    }
    if sel.stack_overflow {
        exceptions::overflow_the_stack();
    }
}
