//! ヒープ無しで使える固定長の文字列バッファ。
//!
//! Phase 1 ではヒープが無いため、画面に表示する文字列の整形に使う。

use core::fmt;

/// 容量 `N` バイトの UTF-8 文字列バッファ。溢れた分は文字境界で切り捨てる。
pub struct StackString<const N: usize> {
    buf: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> StackString<N> {
    /// 空のバッファを作る。
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
            truncated: false,
        }
    }

    /// 格納済みの文字列。
    pub fn as_str(&self) -> &str {
        // 文字境界でしか切らないので常に妥当な UTF-8 だが、unsafe を避けて検証する。
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    /// 容量不足で切り捨てが発生したか。
    #[allow(dead_code)] // Phase 1 では未使用だが API として提供
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }
}

impl<const N: usize> Default for StackString<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> fmt::Write for StackString<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let space = N - self.len;
        let take = if s.len() <= space {
            s.len()
        } else {
            self.truncated = true;
            // space 以下で最大の文字境界
            (0..=space)
                .rev()
                .find(|&i| s.is_char_boundary(i))
                .unwrap_or(0)
        };
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

impl<const N: usize> fmt::Debug for StackString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<const N: usize> fmt::Display for StackString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
