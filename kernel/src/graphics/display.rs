//! 画面 (framebuffer) の所有者。
//!
//! Phase 1 では `Framebuffer` は kmain のローカル変数だったが、Phase 2 からは
//! Kernel Panic 画面をどこからでも (例外ハンドラ・パニックハンドラから) 描けるように、
//! ここでグローバルに所有する。
//!
//! - 通常の描画: [`with`] で [`IrqSpinLock`] を取り、`&mut Framebuffer` を借りて描く。
//!   Phase 3 の描画基盤 (ダブルバッファ等) もこの関数経由で framebuffer に触れる。
//! - パニック時: [`force_for_panic`] で保持者を無視してロックを奪う (描画途中の画面の上に
//!   パニック画面を上書きする。保持者は二度と再開しないので競合しない)。

use super::Framebuffer;
use crate::sync::{IrqSpinLock, IrqSpinLockGuard};

static DISPLAY: IrqSpinLock<Option<Framebuffer>> = IrqSpinLock::new(None);

/// framebuffer を登録する (起動時に 1 度)。既に登録済みなら古いものを返す。
pub fn install(fb: Framebuffer) -> Option<Framebuffer> {
    DISPLAY.lock().replace(fb)
}

/// framebuffer を借りて `f` を実行する。未登録なら `None`。
///
/// ロック保持中は割り込みが無効になるので、長い描画は分割すること。
pub fn with<R>(f: impl FnOnce(&mut Framebuffer) -> R) -> Option<R> {
    DISPLAY.lock().as_mut().map(f)
}

/// パニック経路専用: 保持者を無視して framebuffer を借りる。
///
/// # Safety
///
/// 割り込み無効で、framebuffer のロックを保持していたコードが二度と実行されないこと
/// (パニック処理中で CPU を停止する直前)。他 CPU は存在しないこと (現状 BSP のみ)。
pub unsafe fn force_for_panic() -> IrqSpinLockGuard<'static, Option<Framebuffer>> {
    // SAFETY: 呼び出し元の契約による。Framebuffer への書き込みは範囲チェック付きなので、
    // 保持者が描画途中でもメモリ安全性は損なわれない (画面が混ざるだけ)。
    unsafe { DISPLAY.force_lock() }
}
