//! ARM Generic Timer: физический таймер EL1 (CNTP_*), периодический тик.
use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);
static PERIOD: AtomicU64 = AtomicU64::new(0);
static INTID: AtomicU32 = AtomicU32::new(30);

/// Частота счётчика, Гц (CNTFRQ_EL0).
pub fn freq() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Текущее значение системного счётчика (CNTPCT_EL0).
pub fn counter() -> u64 {
    let v: u64;
    unsafe {
        asm!("isb", options(nostack));
        asm!("mrs {}, cntpct_el0", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn intid() -> u32 {
    INTID.load(Ordering::Relaxed)
}

fn arm(period: u64) {
    unsafe { asm!("msr cntp_tval_el0, {}", in(reg) period, options(nomem, nostack)) };
}

/// Запускает периодический тик с частотой `hz`. Прерывание нужно включить в GIC отдельно.
pub fn start_periodic(intid: u32, hz: u64) {
    INTID.store(intid, Ordering::Relaxed);
    let period = freq() / hz;
    PERIOD.store(period, Ordering::Relaxed);
    start_periodic_local();
}

/// Запускает локальный таймер этого CPU с уже заданным глобальным периодом.
/// Используется вторичными ядрами после `gic::init_cpu`.
pub fn start_periodic_local() {
    let period = PERIOD.load(Ordering::Relaxed);
    if period == 0 {
        return;
    }
    arm(period);
    // CNTP_CTL_EL0: ENABLE=1, IMASK=0
    unsafe { asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack)) };
}

/// Вызывается из обработчика IRQ: считает тик и перезаряжает таймер (это же снимает запрос).
/// `is_boot` — только CPU0 двигает глобальный счётчик, иначе 4 ядра ускорили бы время в 4 раза.
pub fn on_irq(is_boot: bool) {
    if is_boot {
        TICKS.store(TICKS.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    }
    arm(PERIOD.load(Ordering::Relaxed));
}
