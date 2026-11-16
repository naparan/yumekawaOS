//! 時間管理: Local APIC タイマの校正・周期割り込み・uptime (§9.1 Timer Interrupt, §27)。
//!
//! - 校正: Local APIC タイマ (16 分周) を one-shot で走らせ、参照クロック
//!   (第 1 候補: ACPI PM timer、無ければ PIT チャネル 2) で約 [`CALIBRATION_MS`] ms 計り、
//!   これを [`CALIBRATION_ROUNDS`] 回繰り返した中央値を採用する。同時に TSC も校正する。
//! - 周期割り込み: [`TIMER_HZ`] Hz で `vectors::LAPIC_TIMER` を発生させる。
//! - uptime: tick 数 × 1 tick のカウント数 + 現在の tick 内の経過カウントを、校正した
//!   周波数で ns に換算する。値は単調非減少 (前回返した値を下回らない) を保証する。
//! - スケジューラのフック: [`set_tick_hook`] で、タイマ割り込みごとに呼ぶ関数を登録できる
//!   (Phase 5 のプリエンプション用。EOI はディスパッチャがフック呼び出し前に送っている)。
//!
//! 壁時計 (RTC による日付・時刻) は Phase 2 の範囲外 (§27 の GUI 時刻表示は後続フェーズ)。

use core::fmt;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use core::time::Duration;

use crate::acpi::AcpiInfo;
use crate::arch::x86_64::cpu;
use crate::arch::x86_64::trap::TrapFrame;
use crate::drivers::apic::{self, LocalApic};
use crate::drivers::pit;
use crate::drivers::pm_timer::{self, AcpiPmTimer};
use crate::interrupts::{self, vectors};
use crate::sync::Once;

/// タイマ割り込みの周波数。
pub const TIMER_HZ: u64 = 1000;
/// 1 回の校正の計測時間。
pub const CALIBRATION_MS: u64 = 25;
/// 校正の繰り返し回数。
pub const CALIBRATION_ROUNDS: usize = 3;

const NS_PER_SEC: u128 = 1_000_000_000;

/// 校正に使った参照クロック。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceClock {
    /// ACPI PM timer (3.579545 MHz)
    AcpiPmTimer {
        /// I/O ポート
        port: u16,
        /// カウンタのビット幅
        bits: u32,
    },
    /// PIT チャネル 2 (1.193182 MHz)
    Pit,
}

impl fmt::Display for ReferenceClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AcpiPmTimer { port, bits } => {
                write!(
                    f,
                    "ACPI PM timer (3.579545 MHz, {bits}-bit, port {port:#x})"
                )
            }
            Self::Pit => f.write_str("PIT channel 2 (1.193182 MHz)"),
        }
    }
}

/// 校正結果。
#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    /// 参照クロック
    pub reference: ReferenceClock,
    /// Local APIC タイマの周波数 (分周後, Hz)
    pub lapic_hz: u64,
    /// 各回の計測値 (Hz)
    pub samples: [u64; CALIBRATION_ROUNDS],
    /// 計測値のばらつき (最大 - 最小) の中央値に対する比 (ppm)
    pub spread_ppm: u64,
    /// TSC の周波数 (Hz, 0 = 未校正)
    pub tsc_hz: u64,
    /// 1 tick あたりの Local APIC タイマのカウント
    pub counts_per_tick: u32,
}

/// 校正・開始のエラー。
#[derive(Debug, Clone, Copy)]
pub enum TimerError {
    /// Local APIC が初期化されていない
    NoLocalApic,
    /// 参照クロックが使えない
    NoReferenceClock,
    /// 計測値が異常 (0 やオーバーフロー)
    BadMeasurement(u64),
}

impl fmt::Display for TimerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLocalApic => f.write_str("local APIC is not initialized"),
            Self::NoReferenceClock => {
                f.write_str("no usable reference clock (ACPI PM timer / PIT both failed)")
            }
            Self::BadMeasurement(hz) => write!(f, "implausible APIC timer frequency {hz} Hz"),
        }
    }
}

struct TimerState {
    calibration: Calibration,
    lapic: &'static LocalApic,
}

static STATE: Once<TimerState> = Once::new();
static TICKS: AtomicU64 = AtomicU64::new(0);
static LAST_NS: AtomicU64 = AtomicU64::new(0);
static TICK_HOOK: AtomicUsize = AtomicUsize::new(0);
static TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// 1 回分の計測: (APIC カウント, 参照の経過 ns, TSC の経過)。
fn measure(
    lapic: &LocalApic,
    reference: ReferenceClock,
    pm: Option<AcpiPmTimer>,
) -> Option<(u64, u64, u64)> {
    match (reference, pm) {
        (ReferenceClock::AcpiPmTimer { .. }, Some(pm)) => {
            let target = pm_timer::FREQUENCY_HZ * CALIBRATION_MS / 1000;
            let start = pm.read();
            lapic.timer_start_measurement(u32::MAX);
            let tsc0 = cpu::rdtsc();
            let mut polls = 0u64;
            let mut now;
            loop {
                now = pm.read();
                if u64::from(pm.delta(start, now)) >= target {
                    break;
                }
                polls += 1;
                if polls > 500_000_000 {
                    return None;
                }
                core::hint::spin_loop();
            }
            let apic_now = lapic.timer_current();
            let tsc1 = cpu::rdtsc();
            let pm_elapsed = u64::from(pm.delta(start, now));
            let ns =
                (u128::from(pm_elapsed) * NS_PER_SEC / u128::from(pm_timer::FREQUENCY_HZ)) as u64;
            Some((u64::from(u32::MAX - apic_now), ns, tsc1.wrapping_sub(tsc0)))
        }
        (ReferenceClock::Pit, _) => {
            let count = (pit::FREQUENCY_HZ * CALIBRATION_MS / 1000) as u16;
            let mut tsc0 = 0;
            let mut tsc1 = 0;
            let mut apic_now = 0;
            let ok = pit::wait_ticks(
                count,
                || {
                    lapic.timer_start_measurement(u32::MAX);
                    tsc0 = cpu::rdtsc();
                },
                || {
                    apic_now = lapic.timer_current();
                    tsc1 = cpu::rdtsc();
                },
            );
            if !ok {
                return None;
            }
            let ns = (u128::from(count) * NS_PER_SEC / u128::from(pit::FREQUENCY_HZ)) as u64;
            Some((u64::from(u32::MAX - apic_now), ns, tsc1.wrapping_sub(tsc0)))
        }
        (ReferenceClock::AcpiPmTimer { .. }, None) => None,
    }
}

fn calibrate_with(
    lapic: &LocalApic,
    reference: ReferenceClock,
    pm: Option<AcpiPmTimer>,
    tsc: bool,
) -> Option<Calibration> {
    let mut samples = [0u64; CALIBRATION_ROUNDS];
    let mut tsc_samples = [0u64; CALIBRATION_ROUNDS];
    for i in 0..CALIBRATION_ROUNDS {
        let (counts, ns, tsc_delta) = measure(lapic, reference, pm)?;
        if ns == 0 {
            return None;
        }
        samples[i] = (u128::from(counts) * NS_PER_SEC / u128::from(ns)) as u64;
        tsc_samples[i] = (u128::from(tsc_delta) * NS_PER_SEC / u128::from(ns)) as u64;
    }
    lapic.timer_stop();
    let mut sorted = samples;
    sorted.sort_unstable();
    let median = sorted[CALIBRATION_ROUNDS / 2];
    tsc_samples.sort_unstable();
    let spread_ppm = ((sorted[CALIBRATION_ROUNDS - 1] - sorted[0]) * 1_000_000)
        .checked_div(median)
        .unwrap_or(0);
    Some(Calibration {
        reference,
        lapic_hz: median,
        samples,
        spread_ppm,
        tsc_hz: if tsc {
            tsc_samples[CALIBRATION_ROUNDS / 2]
        } else {
            0
        },
        counts_per_tick: 0,
    })
}

/// Local APIC タイマを校正する。
pub fn calibrate(acpi: Option<&AcpiInfo>, has_tsc: bool) -> Result<Calibration, TimerError> {
    let lapic = apic::local_apic().ok_or(TimerError::NoLocalApic)?;
    let pm = acpi
        .and_then(|a| a.fadt)
        .and_then(|f| f.pm_timer)
        .map(AcpiPmTimer::new)
        .filter(|pm| {
            let running = pm.is_running();
            if !running {
                warn!(
                    "time: ACPI PM timer at port {:#x} is not counting; ignoring it",
                    pm.port()
                );
            }
            running
        });
    let mut result = None;
    if let Some(pm) = pm {
        let reference = ReferenceClock::AcpiPmTimer {
            port: pm.port(),
            bits: pm.bits(),
        };
        result = calibrate_with(lapic, reference, Some(pm), has_tsc);
        if result.is_none() {
            warn!("time: calibration with the ACPI PM timer failed; trying the PIT");
        }
    } else {
        info!("time: no ACPI PM timer; calibrating with the PIT");
    }
    if result.is_none() {
        result = calibrate_with(lapic, ReferenceClock::Pit, None, has_tsc);
    }
    let mut c = result.ok_or(TimerError::NoReferenceClock)?;
    // 1kHz で 1 tick あたり 100 カウント未満だと分解能が粗すぎる。u32 に収まる必要もある
    let per_tick = c.lapic_hz / TIMER_HZ;
    if per_tick < 100 || per_tick > u64::from(u32::MAX) {
        return Err(TimerError::BadMeasurement(c.lapic_hz));
    }
    c.counts_per_tick = per_tick as u32;
    Ok(c)
}

/// 周期割り込みを開始する (割り込みはまだ無効のままでよい。`sti` 後に tick が進む)。
pub fn start(calibration: Calibration) -> Result<(), TimerError> {
    let lapic = apic::local_apic().ok_or(TimerError::NoLocalApic)?;
    TSC_HZ.store(calibration.tsc_hz, Ordering::Relaxed);
    if STATE.set(TimerState { calibration, lapic }).is_err() {
        warn!("time: timer already started");
        return Ok(());
    }
    if let Err(e) = interrupts::register(vectors::LAPIC_TIMER, timer_interrupt) {
        warn!("time: cannot register the timer handler: {e}");
    }
    lapic.timer_start_periodic(calibration.counts_per_tick);
    Ok(())
}

/// TSC の周波数を設定する (校正済みの値。busy-wait 用)。
pub fn set_tsc_hz(hz: u64) {
    TSC_HZ.store(hz, Ordering::Relaxed);
}

fn timer_interrupt(frame: &mut TrapFrame) {
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    let hook = TICK_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        // SAFETY: TICK_HOOK に格納するのは set_tick_hook が TickHook 型の関数ポインタを
        // usize にした値だけ。
        let hook = unsafe { core::mem::transmute::<usize, TickHook>(hook) };
        hook(frame, ticks);
    }
}

/// タイマ割り込みごとに呼ばれるフックの型 (割り込みコンテキスト, IF=0)。
/// 引数は割り込まれたコンテキストのレジスタと、現在の tick 数。
pub type TickHook = fn(&mut TrapFrame, u64);

/// タイマ割り込みのフックを設定する (Phase 5 のスケジューラ用)。`None` で解除。
#[allow(dead_code)] // Phase 5 で使用
pub fn set_tick_hook(hook: Option<TickHook>) {
    TICK_HOOK.store(hook.map_or(0, |h| h as usize), Ordering::Release);
}

/// 起動 (タイマ開始) からの tick 数。
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// 校正結果 (タイマ開始後)。
pub fn calibration() -> Option<Calibration> {
    STATE.get().map(|s| s.calibration)
}

/// タイマ開始からの経過時間 (ns)。タイマ開始前は 0。単調非減少。
pub fn uptime_ns() -> u64 {
    let Some(state) = STATE.get() else {
        return 0;
    };
    let per_tick = u64::from(state.calibration.counts_per_tick);
    let (ticks, current) = loop {
        let t1 = TICKS.load(Ordering::Acquire);
        let cur = u64::from(state.lapic.timer_current());
        let t2 = TICKS.load(Ordering::Acquire);
        if t1 == t2 {
            break (t1, cur);
        }
    };
    let in_tick = per_tick.saturating_sub(current).min(per_tick);
    let counts = u128::from(ticks) * u128::from(per_tick) + u128::from(in_tick);
    let ns = (counts * NS_PER_SEC / u128::from(state.calibration.lapic_hz)) as u64;
    // 割り込みが保留中 (IF=0 等) でカウンタだけ再装填された場合に戻らないようにする
    let prev = LAST_NS.fetch_max(ns, Ordering::AcqRel);
    ns.max(prev)
}

/// タイマ開始からの経過時間。
pub fn uptime() -> Duration {
    Duration::from_nanos(uptime_ns())
}

/// タイマ開始からの経過時間 (ms)。
pub fn uptime_ms() -> u64 {
    uptime_ns() / 1_000_000
}

/// 割り込みに依存しない期限 (TSC を使う)。割り込み有効化前のデバイス初期化のタイムアウト用。
///
/// TSC が校正されていない場合は、ポーリング回数 (1 回 ≒ 1µs と仮定) で代用する。
#[derive(Debug)]
pub struct Deadline {
    tsc_end: Option<u64>,
    polls_left: u64,
}

impl Deadline {
    /// 今から `timeout` 後に期限切れになる。
    pub fn after(timeout: Duration) -> Self {
        let hz = TSC_HZ.load(Ordering::Relaxed);
        if hz != 0 {
            let cycles = (timeout.as_nanos() * u128::from(hz) / NS_PER_SEC) as u64;
            Self {
                tsc_end: Some(cpu::rdtsc().saturating_add(cycles)),
                polls_left: 0,
            }
        } else {
            Self {
                tsc_end: None,
                polls_left: timeout.as_micros() as u64,
            }
        }
    }

    /// 期限切れか (TSC が無い場合は呼び出し 1 回を 1µs として数える)。
    pub fn expired(&mut self) -> bool {
        match self.tsc_end {
            Some(end) => cpu::rdtsc() >= end,
            None => {
                if self.polls_left == 0 {
                    return true;
                }
                self.polls_left -= 1;
                for _ in 0..100 {
                    core::hint::spin_loop();
                }
                false
            }
        }
    }
}

/// `d` だけ待つ (割り込みが有効なら `hlt` で待つ)。
pub fn sleep(d: Duration) {
    let end = uptime_ns().saturating_add(d.as_nanos() as u64);
    if STATE.get().is_none() || !cpu::interrupts_enabled() {
        let mut deadline = Deadline::after(d);
        while !deadline.expired() {
            core::hint::spin_loop();
        }
        return;
    }
    while uptime_ns() < end {
        cpu::enable_interrupts_and_halt();
    }
}
