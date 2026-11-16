//! タイマのセルフテスト。
//!
//! - tick が進み、uptime (ns) と tick 数が整合すること
//! - uptime が独立した参照クロック (ACPI PM timer) と大きく乖離しないこと
//! - uptime が単調非減少であること
//! - ホスト側との比較用に `TIMER_MARK` 行を出力する (scripts/test-boot.sh が QEMU の
//!   実時間と比較する)

use core::time::Duration;

use super::Suite;
use crate::drivers::pm_timer::{self, AcpiPmTimer};
use crate::time::{self, ReferenceClock};

/// 計測する長さ。
const WINDOW_MS: u64 = 3000;

pub(super) fn run(s: &mut Suite) {
    let Some(cal) = time::calibration() else {
        s.check("calibrated", false, format_args!("timer is not running"));
        return;
    };
    s.check(
        "calibrated",
        cal.lapic_hz > 0 && cal.counts_per_tick > 0,
        format_args!(
            "reference={}, lapic={} Hz, spread={} ppm, tsc={} Hz",
            cal.reference, cal.lapic_hz, cal.spread_ppm, cal.tsc_hz
        ),
    );

    let pm = match cal.reference {
        ReferenceClock::AcpiPmTimer { port, bits } => {
            Some(AcpiPmTimer::new(crate::acpi::PmTimer {
                port,
                is_32bit: bits == 32,
            }))
        }
        ReferenceClock::Pit => None,
    };

    let t0 = time::ticks();
    let u0 = time::uptime_ns();
    let pm0 = pm.map(|p| p.read());
    info!("TIMER_MARK start uptime_ms={} ticks={t0}", u0 / 1_000_000);
    time::sleep(Duration::from_millis(WINDOW_MS));
    let t1 = time::ticks();
    let u1 = time::uptime_ns();
    let pm1 = pm.map(|p| p.read());
    let elapsed_ms = (u1 - u0) / 1_000_000;
    info!(
        "TIMER_MARK end uptime_ms={} ticks={t1} elapsed_ms={elapsed_ms}",
        u1 / 1_000_000
    );

    let dticks = t1 - t0;
    let expected_ticks = WINDOW_MS * time::TIMER_HZ / 1000;
    s.check(
        "ticks_advance",
        dticks + 2 >= expected_ticks && dticks <= expected_ticks + 50,
        format_args!(
            "{dticks} ticks in {elapsed_ms} ms of uptime (expected about {expected_ticks})"
        ),
    );

    match (pm, pm0, pm1) {
        (Some(p), Some(a), Some(b)) => {
            // 24bit カウンタは約 4.7 秒で一周するので、計測窓 (3 秒) は 1 周未満
            let pm_ms = u64::from(p.delta(a, b)) * 1000 / pm_timer::FREQUENCY_HZ;
            let diff = elapsed_ms.abs_diff(pm_ms);
            s.check(
                "uptime_vs_pm_timer",
                diff * 100 <= pm_ms * 2 + 100,
                format_args!("uptime advanced {elapsed_ms} ms, ACPI PM timer {pm_ms} ms (diff {diff} ms, limit 2% + 1 ms)"),
            );
        }
        _ => s.skip("uptime_vs_pm_timer", "no ACPI PM timer"),
    }

    // 単調性
    let mut prev = time::uptime_ns();
    let mut violations = 0u32;
    for _ in 0..200_000 {
        let now = time::uptime_ns();
        if now < prev {
            violations += 1;
        }
        prev = now;
    }
    s.check(
        "uptime_monotonic",
        violations == 0,
        format_args!("200000 consecutive reads, {violations} went backwards"),
    );
}
