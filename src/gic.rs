//! Драйвер ARM GICv3 (EL1, режим системных регистров ICC_*, affinity routing).
//!
//! Distributor инициализируется один раз на boot CPU, redistributor — на каждом
//! ядре отдельно (`init_cpu`). Redistributor текущего ядра хранится в `smp::PerCpu`.
use crate::smp;
use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};

// Distributor
const GICD_CTLR: usize = 0x0000;
const GICD_IGROUPR: usize = 0x0080;
const GICD_ISENABLER: usize = 0x0100;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_IROUTER: usize = 0x6000;
// Redistributor (RD_base)
const GICR_CTLR: usize = 0x0000;
const GICR_TYPER: usize = 0x0008;
const GICR_WAKER: usize = 0x0014;
// Redistributor (SGI_base = RD_base + 64 KiB)
const SGI_BASE: usize = 0x10000;
const GICR_IGROUPR0: usize = 0x0080;
const GICR_ISENABLER0: usize = 0x0100;
const GICR_ICENABLER0: usize = 0x0180;
const GICR_IPRIORITYR: usize = 0x0400;
const GICR_ICFGR1: usize = 0x0C04;

static GICD: AtomicUsize = AtomicUsize::new(0);
static GICR: AtomicUsize = AtomicUsize::new(0);
static GICR_SIZE: AtomicUsize = AtomicUsize::new(0);

unsafe fn r32(a: usize) -> u32 {
    read_volatile(a as *const u32)
}
unsafe fn w32(a: usize, v: u32) {
    write_volatile(a as *mut u32, v)
}

fn spin_until(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..5_000_000u32 {
        if f() {
            return true;
        }
    }
    false
}

fn mpidr() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// Полная инициализация GIC на boot CPU: distributor + redistributor этого ядра.
/// IRQ остаются замаскированными в PSTATE.
pub unsafe fn init(gicd: usize, gicr: usize, gicr_size: usize) -> Result<(), &'static str> {
    init_distributor(gicd)?;
    GICD.store(gicd, Ordering::Relaxed);
    GICR.store(gicr, Ordering::Relaxed);
    GICR_SIZE.store(gicr_size, Ordering::Relaxed);
    init_cpu()
}

/// Инициализация distributor. Выполняется ровно один раз на boot CPU.
unsafe fn init_distributor(gicd: usize) -> Result<(), &'static str> {
    // ARE + Group 1
    w32(gicd + GICD_CTLR, 0);
    if !spin_until(|| r32(gicd + GICD_CTLR) & (1 << 31) == 0) {
        return Err("GICD RWP timeout");
    }
    w32(gicd + GICD_CTLR, (1 << 4) | (1 << 1));
    if !spin_until(|| r32(gicd + GICD_CTLR) & (1 << 31) == 0) {
        return Err("GICD RWP timeout (enable)");
    }
    Ok(())
}

/// Поднимает redistributor и интерфейс CPU текущего ядра. Вызывается на каждом
/// CPU после включения MMU. Адреса GICD/GICR берутся из сохранённых значений.
pub unsafe fn init_cpu() -> Result<(), &'static str> {
    let gicr = GICR.load(Ordering::Relaxed);
    let gicr_size = GICR_SIZE.load(Ordering::Relaxed);
    if gicr == 0 {
        return Err("GIC not initialised");
    }

    // --- Redistributor этого CPU ---
    let mp = mpidr();
    let aff = (((mp >> 32) & 0xff) << 24) | (mp & 0xff_ffff);
    let span = if gicr_size == 0 { 64 * 0x20000 } else { gicr_size };
    let mut p = gicr;
    let mut rd = 0usize;
    while p + 0x20000 <= gicr + span {
        let lo = r32(p + GICR_TYPER);
        let hi = r32(p + GICR_TYPER + 4);
        if hi as u64 == aff {
            rd = p;
            break;
        }
        if lo & (1 << 4) != 0 {
            break; // Last
        }
        p += 0x20000;
        if lo & (1 << 1) != 0 {
            p += 0x20000; // VLPIS: ещё два 64-КиБ фрейма
        }
    }
    if rd == 0 {
        return Err("redistributor for this CPU not found");
    }

    // Разбудить redistributor
    let waker = rd + GICR_WAKER;
    w32(waker, r32(waker) & !(1 << 1)); // ProcessorSleep = 0
    if !spin_until(|| r32(waker) & (1 << 2) == 0) {
        return Err("redistributor wake timeout"); // ChildrenAsleep
    }

    // SGI/PPI: Group 1, всё выключено, приоритет 0xA0, PPI level-sensitive
    let sgi = rd + SGI_BASE;
    w32(sgi + GICR_IGROUPR0, 0xffff_ffff);
    w32(sgi + GICR_ICENABLER0, 0xffff_ffff);
    for i in 0..8 {
        w32(sgi + GICR_IPRIORITYR + 4 * i, 0xa0a0_a0a0);
    }
    w32(sgi + GICR_ICFGR1, 0);
    if !spin_until(|| r32(rd + GICR_CTLR) & (1 << 3) == 0) {
        return Err("GICR RWP timeout");
    }

    // --- Интерфейс CPU (системные регистры) ---
    let sre: u64;
    asm!("mrs {}, S3_0_C12_C12_5", out(reg) sre, options(nostack)); // ICC_SRE_EL1
    asm!("msr S3_0_C12_C12_5, {}", in(reg) sre | 1, options(nostack));
    asm!("isb", options(nostack));
    let sre2: u64;
    asm!("mrs {}, S3_0_C12_C12_5", out(reg) sre2, options(nostack));
    if sre2 & 1 == 0 {
        return Err("ICC_SRE_EL1.SRE cannot be set (EL2 does not allow it)"); // ток при л2 такое 
    }
    asm!("msr S3_0_C4_C6_0, {}", in(reg) 0xffu64, options(nostack)); // ICC_PMR_EL1: пропускать все приоритеты
    asm!("msr S3_0_C12_C12_3, {}", in(reg) 0u64, options(nostack)); // ICC_BPR1_EL1
    asm!("msr S3_0_C12_C12_7, {}", in(reg) 1u64, options(nostack)); // ICC_IGRPEN1_EL1
    asm!("isb", options(nostack));

    smp::set_gic_rd(rd);
    Ok(())
}

/// Включает прерывание: SGI/PPI (<32) через redistributor текущего CPU,
/// SPI — через distributor (маршрут на текущий CPU).
pub fn enable_irq(intid: u32, prio: u8) {
    let rd = smp::gic_rd();
    let gicd = GICD.load(Ordering::Relaxed);
    if rd == 0 || gicd == 0 {
        return;
    }
    let n = intid as usize;
    unsafe {
        if n < 32 {
            let sgi = rd + SGI_BASE;
            write_volatile((sgi + GICR_IPRIORITYR + n) as *mut u8, prio);
            w32(sgi + GICR_ISENABLER0, 1 << n);
        } else {
            let d = gicd;
            let w = n / 32;
            let bit = 1u32 << (n % 32);
            w32(d + GICD_IGROUPR + 4 * w, r32(d + GICD_IGROUPR + 4 * w) | bit);
            write_volatile((d + GICD_IPRIORITYR + n) as *mut u8, prio);
            let mp = mpidr();
            let route = ((mp >> 32) & 0xff) << 32 | (mp & 0xff_ffff);
            w32(d + GICD_IROUTER + 8 * n, route as u32);
            w32(d + GICD_IROUTER + 8 * n + 4, (route >> 32) as u32);
            w32(d + GICD_ISENABLER + 4 * w, bit);
        }
    }
}

/// Читает ICC_IAR1_EL1: INTID принятого прерывания (>= 1020 — ложное, EOI не нужен).
pub fn ack() -> u32 {
    let v: u64;
    unsafe { asm!("mrs {}, S3_0_C12_C12_0", out(reg) v, options(nostack)) };
    (v & 0xff_ffff) as u32
}

/// ICC_EOIR1_EL1: завершение обработки (priority drop + deactivate).
pub fn eoi(intid: u32) {
    unsafe { asm!("msr S3_0_C12_C12_1, {}", in(reg) intid as u64, options(nostack)) };
}

/// Отправляет SGI (0..15) самому себе через ICC_SGI1R_EL1.
pub fn send_sgi_self(intid: u32) {
    let mp = mpidr();
    let (a0, a1, a2, a3) = (mp & 0xff, (mp >> 8) & 0xff, (mp >> 16) & 0xff, (mp >> 32) & 0xff);
    let v = (1u64 << (a0 & 0xf)) | (a1 << 16) | (((intid & 0xf) as u64) << 24) | (a2 << 32) | ((a0 >> 4) << 44) | (a3 << 48);
    unsafe {
        asm!("msr S3_0_C12_C11_5, {}", in(reg) v, options(nostack));
        asm!("isb", options(nostack));
    }
}
