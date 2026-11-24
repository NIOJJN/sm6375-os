//! SMP-каркас: запуск вторичных CPU через PSCI, per-CPU данные, IPI и
//! межъядерный спинлок.
//!
//! Что уже работает:
//! * boot CPU инициализирует себя, парсит список CPU из DTB и поднимает
//!   остальные ядра через PSCI `CPU_ON`;
//! * каждое ядро получает свою структуру `PerCpu` (указатель в `TPIDR_EL1`),
//!   собственный стек, включает MMU с корневой таблицей ядра, поднимает свой
//!   redistributor GICv3 и разрешает SGI `IPI_RESCHED`;
//! * CPU0 может отправить IPI любому ядру и дождаться его;
//! * самотест: все вторичные CPU параллельно увеличивают счётчик под
//!   `SpinLock`, CPU0 проверяет итог.
//!
//! Чего пока нет (следующий этап): планировщик по-прежнему работает только на
//! CPU0, а структуры `proc`/`ipc`/`shm` защищены только `IrqGuard` и не
//! рассчитаны на одновременный доступ с нескольких ядер.
use crate::{
    fdt::BootInfo,
    gic, mmu, sched, timer,
    sync::{IrqGuard, SpinLock},
};
use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Максимум ядер, которые поднимает этот каркас. Совпадает с `fdt::MAX_CPUS`.
pub const MAX_CPUS: usize = 4;
/// SGI, которым CPU0 будит вторичные ядра.
pub const IPI_RESCHED: u32 = 2;
/// Сколько операций под спинлоком делает каждое вторичное ядро в самотесте.
const LOCK_ITERS: u64 = 20_000;

/// Данные, свои у каждого CPU. Поля, помеченные `Atomic*`, читаются с других
/// ядер; остальные пишет только владелец до публикации `online = 1`.
#[repr(C, align(64))]
struct PerCpu {
    cpu: AtomicUsize,
    mpidr: AtomicU64,
    gic_rd: AtomicUsize,
    online: AtomicU32,
    ipis: AtomicU32,
    /// Текущий поток на этом CPU (индекс в таблице планировщика).
    current: AtomicUsize,
    /// pid текущего потока (0 — поток ядра / idle).
    current_pid: AtomicUsize,
    /// Нужно перепланирование на этом CPU.
    need_resched: AtomicU32,
}

impl PerCpu {
    const fn new() -> Self {
        Self {
            cpu: AtomicUsize::new(0),
            mpidr: AtomicU64::new(0),
            gic_rd: AtomicUsize::new(0),
            online: AtomicU32::new(0),
            ipis: AtomicU32::new(0),
            current: AtomicUsize::new(0),
            current_pid: AtomicUsize::new(0),
            need_resched: AtomicU32::new(0),
        }
    }
}

static PER_CPU: [PerCpu; MAX_CPUS] = [const { PerCpu::new() }; MAX_CPUS];

/// Счётчик самотеста спинлока: его увеличивают все вторичные ядра.
static TEST_COUNTER: SpinLock<u64> = SpinLock::new(0);
static TEST_DONE: AtomicU32 = AtomicU32::new(0);

extern "C" {
    /// Точка входа вторичного ядра из `boot.S`.
    fn secondary_entry();
}

#[inline]
fn mpidr() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

#[inline]
fn park() -> ! {
    loop {
        unsafe { asm!("wfe", options(nomem, nostack)) };
    }
}

/// Указатель на `PerCpu` текущего ядра (через `TPIDR_EL1`).
#[inline]
fn current() -> &'static PerCpu {
    let p: u64;
    unsafe { asm!("mrs {}, tpidr_el1", out(reg) p, options(nomem, nostack, preserves_flags)) };
    if p == 0 {
        &PER_CPU[0]
    } else {
        unsafe { &*(p as *const PerCpu) }
    }
}

/// Индекс текущего ядра (0 — boot CPU).
#[inline]
pub fn current_cpu_index() -> usize {
    current().cpu.load(Ordering::Relaxed)
}

/// Поток, который сейчас выполняется на этом CPU.
#[inline]
pub fn current_tid() -> usize {
    current().current.load(Ordering::Relaxed)
}

#[inline]
pub fn set_current_tid(tid: usize) {
    current().current.store(tid, Ordering::Relaxed);
}

/// pid текущего потока (0 — ядро/idle).
#[inline]
pub fn current_pid() -> usize {
    current().current_pid.load(Ordering::Relaxed)
}

#[inline]
pub fn set_current_pid(pid: usize) {
    current().current_pid.store(pid, Ordering::Relaxed);
}

/// Флаг «нужно перепланирование» на этом CPU.
#[inline]
pub fn need_resched() -> bool {
    current().need_resched.load(Ordering::Relaxed) != 0
}

#[inline]
pub fn set_need_resched() {
    current().need_resched.store(1, Ordering::Relaxed);
}

#[inline]
pub fn clear_need_resched() {
    current().need_resched.store(0, Ordering::Relaxed);
}

/// Просит все онлайн-ядра перепланировать. Текущему CPU IPI не нужен:
/// он увидит флаг при ближайшей проверке (idle-цикл или возврат из IRQ).
pub fn broadcast_resched() {
    let me = current_cpu_index();
    for cpu in 0..MAX_CPUS {
        if !is_online(cpu) {
            continue;
        }
        PER_CPU[cpu].need_resched.store(1, Ordering::Relaxed);
        if cpu != me {
            send_sgi(cpu, IPI_RESCHED);
        }
    }
}

/// Redistributor GICv3 текущего ядра (заполняется `gic::init_cpu`).
#[inline]
pub fn gic_rd() -> usize {
    current().gic_rd.load(Ordering::Relaxed)
}

pub fn set_gic_rd(rd: usize) {
    current().gic_rd.store(rd, Ordering::Relaxed);
}

/// Инициализирует запись boot CPU и вешает её на `TPIDR_EL1`.
/// Вызывать первым делом в `kernel_main`, до любого обращения к `smp`.
pub fn init_boot_cpu() {
    PER_CPU[0].cpu.store(0, Ordering::Relaxed);
    PER_CPU[0].mpidr.store(mpidr(), Ordering::Relaxed);
    PER_CPU[0].online.store(1, Ordering::Release);
    unsafe {
        asm!("msr tpidr_el1, {}", in(reg) &PER_CPU[0] as *const PerCpu as u64, options(nomem, nostack));
    }
}

/// Число поднятых ядер (включая boot CPU).
pub fn online_count() -> usize {
    PER_CPU.iter().filter(|p| p.online.load(Ordering::Acquire) != 0).count()
}

pub fn is_online(cpu: usize) -> bool {
    cpu < MAX_CPUS && PER_CPU[cpu].online.load(Ordering::Acquire) != 0
}

/// Поднимает вторичные CPU через PSCI. Вызывать после включения MMU и GIC,
/// но до запуска планировщика.
pub fn init(info: &BootInfo) {
    let boot = mpidr();
    let n = info.cpus_n.min(MAX_CPUS);
    if n <= 1 {
        kprintln!("SMP  only {} CPU in DTB, secondaries not started", n);
        return;
    }
    let Some(psci) = info.psci else {
        kprintln!("SMP  no PSCI node, secondaries not started");
        return;
    };
    kprintln!(
        "SMP  PSCI method={} cpu_on={:#x}, {} CPU in DTB",
        if psci.method == 1 { "hvc" } else { "smc" },
        psci.cpu_on,
        n
    );

    let mut slot = 1usize;
    for j in 0..n {
        let (cpu_mpidr, psci_enabled) = info.cpus[j];
        // DTB CPU reg обычно содержит только affinity-часть MPIDR; сравниваем по Aff0..Aff2.
        if (cpu_mpidr & 0xff_ffff) == (boot & 0xff_ffff) || !psci_enabled {
            continue;
        }
        if slot >= MAX_CPUS {
            break;
        }
        PER_CPU[slot].cpu.store(slot, Ordering::Relaxed);
        PER_CPU[slot].mpidr.store(cpu_mpidr, Ordering::Relaxed);
        let entry = secondary_entry as *const () as usize as u64;
        let ret = {
            // PSCI вызывает firmware: на время вызова IRQ лучше замаскировать.
            let _g = IrqGuard::new();
            (unsafe { psci_call(psci.method, psci.cpu_on, cpu_mpidr, entry, slot as u64) }) as i32
        };
        if ret != 0 {
            kprintln!("SMP  CPU{} PSCI CPU_ON failed: {}", slot, ret);
            slot += 1;
            continue;
        }
        // Ждём, пока ядро само сообщит о готовности (MMU + GIC + IRQ).
        let mut spins = 0u64;
        while !is_online(slot) && spins < 50_000_000 {
            spins += 1;
            core::hint::spin_loop();
        }
        if is_online(slot) {
            let mp = PER_CPU[slot].mpidr.load(Ordering::Relaxed);
            kprintln!("SMP  CPU{} online (MPIDR={:#x})", slot, mp);
        } else {
            kprintln!("SMP  CPU{} did not come online (timeout)", slot);
        }
        slot += 1;
    }
    kprintln!("SMP  {} of {} CPU online", online_count(), n);
}

/// Отправляет SGI `intid` на ядро `cpu`.
pub fn send_sgi(cpu: usize, intid: u32) {
    if cpu >= MAX_CPUS {
        return;
    }
    let mp = PER_CPU[cpu].mpidr.load(Ordering::Relaxed);
    let aff0 = mp & 0xff;
    let aff1 = (mp >> 8) & 0xff;
    let aff2 = (mp >> 16) & 0xff;
    let aff3 = (mp >> 32) & 0xff;
    if aff0 >= 16 {
        return; // этот каркас адресует только Aff0 < 16 (QEMU virt)
    }
    let v = (1u64 << aff0)
        | (aff1 << 16)
        | ((intid as u64 & 0xf) << 24)
        | (aff2 << 32)
        | (aff3 << 48);
    unsafe {
        asm!("msr S3_0_C12_C11_5, {}", in(reg) v, options(nostack));
        asm!("isb", options(nostack));
    }
}

/// Вызывается из обработчика IRQ на вторичном ядре.
pub fn on_ipi(_intid: u32) {
    let cpu = current_cpu_index();
    if cpu < MAX_CPUS {
        PER_CPU[cpu].ipis.fetch_add(1, Ordering::Relaxed);
        PER_CPU[cpu].need_resched.store(1, Ordering::Relaxed);
    }
}

pub fn ipi_count(cpu: usize) -> u32 {
    if cpu < MAX_CPUS {
        PER_CPU[cpu].ipis.load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Проверяет IPI до всех ядер и результат параллельного захвата спинлока.
pub fn selftest() {
    let me = current_cpu_index();
    let n = online_count();
    if n <= 1 {
        kprintln!("SMP  selftest skipped: no secondaries");
        return;
    }

    let mut sent = 0;
    for c in 0..MAX_CPUS {
        if c != me && is_online(c) {
            send_sgi(c, IPI_RESCHED);
            sent += 1;
        }
    }
    let mut ipi_ok = sent > 0;
    for c in 0..MAX_CPUS {
        if c == me || !is_online(c) {
            continue;
        }
        let mut spins = 0u64;
        while ipi_count(c) == 0 && spins < 10_000_000 {
            spins += 1;
            core::hint::spin_loop();
        }
        if ipi_count(c) == 0 {
            ipi_ok = false;
        }
    }

    let mut spins = 0u64;
    let expected = (n - 1) as u32;
    while TEST_DONE.load(Ordering::Acquire) < expected && spins < 200_000_000 {
        spins += 1;
        core::hint::spin_loop();
    }
    let done = TEST_DONE.load(Ordering::Acquire) == expected;
    let total = *TEST_COUNTER.lock();
    let want = LOCK_ITERS * (n - 1) as u64;
    let lock_ok = done && total == want;

    kprintln!(
        "SMP  IPI {} ({} targets), spinlock {} ({} ops, expected {})",
        if ipi_ok { "OK" } else { "FAIL" },
        sent,
        if lock_ok { "OK" } else { "FAIL" },
        total,
        want
    );
}

/// Тело вторичного ядра. Вызывается из `boot.S` с `x0 = cpu slot`.
#[no_mangle]
pub extern "C" fn secondary_main(cpu: usize) -> ! {
    if cpu == 0 || cpu >= MAX_CPUS {
        park();
    }
    unsafe {
        PER_CPU[cpu].cpu.store(cpu, Ordering::Relaxed);
        PER_CPU[cpu].mpidr.store(mpidr(), Ordering::Relaxed);
        asm!(
            "msr tpidr_el1, {}",
            in(reg) &PER_CPU[cpu] as *const PerCpu as u64,
            options(nomem, nostack)
        );
    }

    // Таблицы уже построены boot CPU: включаем MMU с тем же корнем.
    unsafe { mmu::enable_secondary(mmu::kernel_root(), 0) };

    if unsafe { gic::init_cpu() }.is_err() {
        // Без GIC ядро бесполезно, но не роняем всю систему.
        park();
    }
    gic::enable_irq(IPI_RESCHED, 0x80);
    gic::enable_irq(timer::intid(), 0x80);
    timer::start_periodic_local();

    // Регистрируем idle-поток этого CPU. После этого current_tid() корректен,
    // и можно безопасно разрешать прерывания.
    sched::init_secondary(cpu);

    // Публикуем готовность и разрешаем IRQ.
    PER_CPU[cpu].online.store(1, Ordering::Release);
    unsafe { asm!("msr daifclr, #2", options(nomem, nostack)) };

    // Параллельный захват спинлока: проверка межъядерной синхронизации.
    for _ in 0..LOCK_ITERS {
        *TEST_COUNTER.lock() += 1;
    }
    TEST_DONE.fetch_add(1, Ordering::AcqRel);

    // Дальше CPU живёт в общем планировщике: idle-цикл выбирает готовые потоки.
    sched::idle_loop(cpu);
}

/// SMCCC-вызов PSCI. `method`: 0 = SMC, 1 = HVC.
unsafe fn psci_call(method: u8, fid: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    if method == 1 {
        asm!(
            "hvc #0",
            inlateout("x0") fid => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            lateout("x4") _,
            lateout("x5") _,
            lateout("x6") _,
            lateout("x7") _,
            lateout("x8") _,
            lateout("x9") _,
            lateout("x10") _,
            lateout("x11") _,
            lateout("x12") _,
            lateout("x13") _,
            lateout("x14") _,
            lateout("x15") _,
            lateout("x16") _,
            lateout("x17") _,
            options(nostack),
        );
    } else {
        asm!(
            "smc #0",
            inlateout("x0") fid => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            lateout("x4") _,
            lateout("x5") _,
            lateout("x6") _,
            lateout("x7") _,
            lateout("x8") _,
            lateout("x9") _,
            lateout("x10") _,
            lateout("x11") _,
            lateout("x12") _,
            lateout("x13") _,
            lateout("x14") _,
            lateout("x15") _,
            lateout("x16") _,
            lateout("x17") _,
            options(nostack),
        );
    }
    ret
}
