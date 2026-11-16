//! 物理フレームアロケータ (ビットマップ方式)。
//!
//! - 管理単位は 4KiB フレーム。ビットマップの 1 ビットが 1 フレーム (1 = 使用中/対象外, 0 = 空き)。
//! - ビットマップは「確保可能な最大物理アドレス」までを覆い、その領域自体も usable メモリ
//!   から切り出して使用中にする。512MiB で 16KiB、64GiB で 2MiB。
//! - 確保可能なのは Limine のメモリマップで `usable` の領域 (と、後で回収した
//!   `bootloader-reclaimable` の領域) だけ。範囲表 ([`FrameAllocator::regions`]) を持ち、
//!   解放時に「確保可能な領域のフレームか」「二重解放でないか」を検査する。
//! - 物理 0〜1MiB は確保しない (フレーム 0 を null と区別するため、および将来の SMP で
//!   AP 起動用トランポリンを 1MiB 未満に置く必要があるため)。
//!
//! グローバルなインスタンスは [`IrqSpinLock`] で保護する (ヒープ拡張やページテーブル操作から
//! 呼ばれるため)。ロック順序: kernel heap → kernel address space → frames。

use core::fmt;

use super::addr::{PAGE_SIZE, PhysAddr, PhysFrame, align_down, align_up};
use super::hhdm;
use crate::boot::{MemoryKind, MemoryMap};
use crate::sync::IrqSpinLock;

/// 確保しない低位メモリ (1MiB 未満)。
pub const LOW_MEMORY_RESERVED: u64 = 0x10_0000;

/// 確保可能な領域の最大数。
const MAX_REGIONS: usize = 256;

/// フレーム操作のエラー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// 空きフレームが無い (または要求を満たす連続領域が無い)
    OutOfMemory,
    /// 解放しようとしたフレームが既に空き (二重解放)
    DoubleFree(PhysAddr),
    /// 確保可能な領域に属さないフレーム (予約領域・カーネル・範囲外など)
    NotAllocatable(PhysAddr),
    /// 不正な引数 (個数 0、アラインが 2 のべき乗でない等)
    InvalidArgument,
    /// 初期化前に使われた
    NotInitialized,
    /// 初期化時: usable メモリが無い/ビットマップを置けない
    NoUsableMemory,
    /// 初期化時: 確保可能な領域が多すぎる
    TooManyRegions,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfMemory => f.write_str("out of physical memory"),
            Self::DoubleFree(a) => write!(f, "double free of frame {a}"),
            Self::NotAllocatable(a) => write!(f, "frame {a} is not in an allocatable region"),
            Self::InvalidArgument => f.write_str("invalid argument"),
            Self::NotInitialized => f.write_str("frame allocator is not initialized"),
            Self::NoUsableMemory => f.write_str("no usable memory for the frame bitmap"),
            Self::TooManyRegions => write!(f, "more than {MAX_REGIONS} allocatable regions"),
        }
    }
}

/// 使用量の統計。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameStats {
    /// 確保可能なフレームの総数 (ビットマップ自身を含む)
    pub total_frames: u64,
    /// 空きフレーム数
    pub free_frames: u64,
    /// ビットマップが使うフレーム数
    pub bitmap_frames: u64,
    /// 回収した bootloader-reclaimable のフレーム数
    pub reclaimed_frames: u64,
    /// 確保可能な領域の数
    pub regions: usize,
}

impl FrameStats {
    /// 使用中のフレーム数。
    pub fn used_frames(&self) -> u64 {
        self.total_frames - self.free_frames
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct FrameRange {
    /// 先頭フレーム番号
    start: u64,
    /// 終端フレーム番号 (含まない)
    end: u64,
}

/// ビットマップ方式の物理フレームアロケータ。
pub struct FrameAllocator {
    bitmap: &'static mut [u64],
    /// ビットマップが覆うフレーム数 (フレーム番号 0..frame_limit)
    frame_limit: u64,
    regions: [FrameRange; MAX_REGIONS],
    region_count: usize,
    total: u64,
    free: u64,
    bitmap_frames: u64,
    reclaimed: u64,
    /// 次に探し始めるワード位置 (next-fit)
    hint: usize,
}

impl fmt::Debug for FrameAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameAllocator")
            .field("frame_limit", &self.frame_limit)
            .field("regions", &self.region_count)
            .field("total", &self.total)
            .field("free", &self.free)
            .finish()
    }
}

fn allocatable_range(base: u64, length: u64) -> Option<(u64, u64)> {
    let start = align_up(base.max(LOW_MEMORY_RESERVED), PAGE_SIZE) / PAGE_SIZE;
    let end = align_down(base.saturating_add(length), PAGE_SIZE) / PAGE_SIZE;
    (start < end).then_some((start, end))
}

impl FrameAllocator {
    /// メモリマップの `usable` 領域からアロケータを構築する。
    ///
    /// `reclaimable_too` が真なら bootloader-reclaimable の領域の分もビットマップの
    /// 範囲に含めておく (後で [`FrameAllocator::add_region`] で回収するため)。
    fn new(map: &MemoryMap) -> Result<Self, FrameError> {
        let mut frame_limit = 0u64;
        for r in map.iter() {
            if matches!(
                r.kind,
                MemoryKind::Usable | MemoryKind::BootloaderReclaimable
            ) && let Some((_, end)) = allocatable_range(r.base, r.length)
            {
                frame_limit = frame_limit.max(end);
            }
        }
        if frame_limit == 0 {
            return Err(FrameError::NoUsableMemory);
        }
        let words = frame_limit.div_ceil(64);
        let bitmap_bytes = words * 8;
        let bitmap_frames = bitmap_bytes.div_ceil(PAGE_SIZE);

        // ビットマップを置く usable 領域を探す (最初に十分な大きさがあるもの)
        let bitmap_start = map
            .iter()
            .filter(|r| r.kind == MemoryKind::Usable)
            .filter_map(|r| allocatable_range(r.base, r.length))
            .find(|(s, e)| e - s >= bitmap_frames)
            .map(|(s, _)| s)
            .ok_or(FrameError::NoUsableMemory)?;

        let virt = hhdm::phys_to_virt(
            PhysAddr::new(bitmap_start * PAGE_SIZE).ok_or(FrameError::NoUsableMemory)?,
        );
        let ptr = virt.as_mut_ptr::<u64>();
        // SAFETY: [bitmap_start, bitmap_start + bitmap_frames) は usable 領域内のフレームで、
        // Limine の HHDM (および以降のカーネルのページテーブルの HHDM) で書き込み可能にマップ
        // されている。このフレームはビットマップ自身により使用中に設定され、以後アロケータが
        // 他へ渡すことは無いため、この 'static な可変スライスが唯一の参照となる。
        // 4KiB 境界なので u64 の整列も満たす。
        let bitmap = unsafe { core::slice::from_raw_parts_mut(ptr, words as usize) };
        bitmap.fill(u64::MAX);

        let mut this = Self {
            bitmap,
            frame_limit,
            regions: [FrameRange::default(); MAX_REGIONS],
            region_count: 0,
            total: 0,
            free: 0,
            bitmap_frames,
            reclaimed: 0,
            hint: 0,
        };
        for r in map.iter().filter(|r| r.kind == MemoryKind::Usable) {
            if let Some((s, e)) = allocatable_range(r.base, r.length) {
                this.add_range(s, e)?;
            }
        }
        // ビットマップ自身を使用中にする
        for frame in bitmap_start..bitmap_start + bitmap_frames {
            this.set_used(frame);
        }
        this.free -= bitmap_frames;
        Ok(this)
    }

    fn add_range(&mut self, start: u64, end: u64) -> Result<(), FrameError> {
        if self.region_count >= MAX_REGIONS {
            return Err(FrameError::TooManyRegions);
        }
        let end = end.min(self.frame_limit);
        if start >= end {
            return Ok(());
        }
        // 開始位置の昇順を保って挿入する (メモリマップは昇順なので通常は末尾)
        let pos = self.regions[..self.region_count]
            .iter()
            .position(|r| r.start > start)
            .unwrap_or(self.region_count);
        self.regions.copy_within(pos..self.region_count, pos + 1);
        self.regions[pos] = FrameRange { start, end };
        self.region_count += 1;
        for frame in start..end {
            self.set_free(frame);
        }
        self.total += end - start;
        self.free += end - start;
        Ok(())
    }

    fn is_used(&self, frame: u64) -> bool {
        self.bitmap[(frame / 64) as usize] & (1 << (frame % 64)) != 0
    }

    fn set_used(&mut self, frame: u64) {
        self.bitmap[(frame / 64) as usize] |= 1 << (frame % 64);
    }

    fn set_free(&mut self, frame: u64) {
        self.bitmap[(frame / 64) as usize] &= !(1 << (frame % 64));
    }

    fn is_allocatable(&self, frame: u64) -> bool {
        let regions = &self.regions[..self.region_count];
        // regions は開始位置の昇順・重複なし
        let idx = regions.partition_point(|r| r.start <= frame);
        idx > 0 && frame < regions[idx - 1].end
    }

    /// 1 フレーム確保する。
    pub fn alloc(&mut self) -> Result<PhysFrame, FrameError> {
        if self.free == 0 {
            return Err(FrameError::OutOfMemory);
        }
        let words = self.bitmap.len();
        for i in 0..words {
            let w = (self.hint + i) % words;
            let word = self.bitmap[w];
            if word != u64::MAX {
                let bit = (!word).trailing_zeros() as u64;
                let frame = w as u64 * 64 + bit;
                if frame >= self.frame_limit {
                    continue;
                }
                self.set_used(frame);
                self.free -= 1;
                self.hint = w;
                return PhysFrame::from_number(frame).ok_or(FrameError::OutOfMemory);
            }
        }
        Err(FrameError::OutOfMemory)
    }

    /// 物理的に連続した `count` フレームを、先頭が `align_frames` フレームの倍数になるよう確保する。
    pub fn alloc_contiguous(
        &mut self,
        count: u64,
        align_frames: u64,
    ) -> Result<PhysFrame, FrameError> {
        if count == 0 || align_frames == 0 || !align_frames.is_power_of_two() {
            return Err(FrameError::InvalidArgument);
        }
        if count > self.free {
            return Err(FrameError::OutOfMemory);
        }
        for ri in 0..self.region_count {
            let r = self.regions[ri];
            let mut start = align_up(r.start, align_frames);
            while start + count <= r.end {
                // [start, start+count) がすべて空きか
                match (start..start + count).rev().find(|&f| self.is_used(f)) {
                    Some(used) => start = align_up(used + 1, align_frames),
                    None => {
                        for f in start..start + count {
                            self.set_used(f);
                        }
                        self.free -= count;
                        return PhysFrame::from_number(start).ok_or(FrameError::OutOfMemory);
                    }
                }
            }
        }
        Err(FrameError::OutOfMemory)
    }

    /// フレームを解放する。二重解放・対象外のフレームはエラー。
    pub fn free(&mut self, frame: PhysFrame) -> Result<(), FrameError> {
        let n = frame.number();
        if n >= self.frame_limit || !self.is_allocatable(n) {
            return Err(FrameError::NotAllocatable(frame.start()));
        }
        if !self.is_used(n) {
            return Err(FrameError::DoubleFree(frame.start()));
        }
        self.set_free(n);
        self.free += 1;
        Ok(())
    }

    /// フレームが確保済み (使用中) か。確保可能な領域外なら `None`。
    pub fn is_allocated(&self, frame: PhysFrame) -> Option<bool> {
        let n = frame.number();
        (n < self.frame_limit && self.is_allocatable(n)).then(|| self.is_used(n))
    }

    /// 統計。
    pub fn stats(&self) -> FrameStats {
        FrameStats {
            total_frames: self.total,
            free_frames: self.free,
            bitmap_frames: self.bitmap_frames,
            reclaimed_frames: self.reclaimed,
            regions: self.region_count,
        }
    }
}

static FRAMES: IrqSpinLock<Option<FrameAllocator>> = IrqSpinLock::new(None);

/// メモリマップからグローバルなフレームアロケータを初期化する。
pub fn init(map: &MemoryMap) -> Result<FrameStats, FrameError> {
    let allocator = FrameAllocator::new(map)?;
    let stats = allocator.stats();
    *FRAMES.lock() = Some(allocator);
    Ok(stats)
}

fn with<R>(f: impl FnOnce(&mut FrameAllocator) -> Result<R, FrameError>) -> Result<R, FrameError> {
    match FRAMES.lock().as_mut() {
        Some(a) => f(a),
        None => Err(FrameError::NotInitialized),
    }
}

/// 1 フレーム確保する (内容は不定)。
pub fn alloc() -> Result<PhysFrame, FrameError> {
    with(FrameAllocator::alloc)
}

/// 1 フレーム確保してゼロクリアする。
pub fn alloc_zeroed() -> Result<PhysFrame, FrameError> {
    let frame = alloc()?;
    zero_frames(frame, 1);
    Ok(frame)
}

/// 連続フレームを確保する。
pub fn alloc_contiguous(count: u64, align_frames: u64) -> Result<PhysFrame, FrameError> {
    with(|a| a.alloc_contiguous(count, align_frames))
}

/// フレームを解放する。
///
/// # Safety
///
/// `frame` はこのアロケータから確保したもので、どこからもマップ・参照されていないこと
/// (解放後に別の用途へ渡されるため)。二重解放や対象外のフレームはエラーとして検出するが、
/// 「まだ使われているフレームの解放」は検出できない。
pub unsafe fn free(frame: PhysFrame) -> Result<(), FrameError> {
    with(|a| a.free(frame))
}

/// 連続フレームを解放する。
///
/// # Safety
///
/// [`free`] と同じ条件を `count` 個すべてのフレームが満たすこと。
pub unsafe fn free_contiguous(start: PhysFrame, count: u64) -> Result<(), FrameError> {
    with(|a| {
        for i in 0..count {
            let f =
                PhysFrame::from_number(start.number() + i).ok_or(FrameError::InvalidArgument)?;
            a.free(f)?;
        }
        Ok(())
    })
}

/// 確保済みかどうか (テスト・診断用)。
pub fn is_allocated(frame: PhysFrame) -> Option<bool> {
    FRAMES.lock().as_ref().and_then(|a| a.is_allocated(frame))
}

/// 統計。
pub fn stats() -> Option<FrameStats> {
    FRAMES.lock().as_ref().map(FrameAllocator::stats)
}

/// パニック画面用: ロックが取れたときだけ統計を返す。
pub fn try_stats() -> Option<FrameStats> {
    FRAMES
        .try_lock()
        .and_then(|g| g.as_ref().map(FrameAllocator::stats))
}

/// bootloader-reclaimable の領域をアロケータへ追加する (回収)。
///
/// # Safety
///
/// それらの領域をもう誰も使っていないこと: Limine のスタック・ページテーブル・GDT・
/// 応答構造体 (メモリマップ・文字列等) から完全に離れていること。
pub unsafe fn reclaim(map: &MemoryMap) -> Result<u64, FrameError> {
    with(|a| {
        let before = a.total;
        for r in map
            .iter()
            .filter(|r| r.kind == MemoryKind::BootloaderReclaimable)
        {
            if let Some((s, e)) = allocatable_range(r.base, r.length) {
                a.add_range(s, e)?;
            }
        }
        let added = a.total - before;
        a.reclaimed += added;
        Ok(added)
    })
}

/// HHDM 経由でフレームをゼロクリアする。
fn zero_frames(frame: PhysFrame, count: u64) {
    let virt = hhdm::phys_to_virt(frame.start());
    // SAFETY: frame はこのアロケータが今確保したばかりの usable フレームで、
    // HHDM で書き込み可能にマップされ、まだ誰にも渡していないため排他的に書ける。
    unsafe {
        core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, (count * PAGE_SIZE) as usize);
    }
}
