//! Этап 2: таблицы страниц AArch64 (гранула 4 КиБ, 48-бит VA, TTBR0 + ASID), идентичное
//! отображение VA == PA, включение MMU и кэшей.
//!
//! Таблицы берутся из PMM (а не из статического пула в .bss).
//! Каждому адресному пространству выделяется свой ASID, чтобы при переключении
//! процессов не делать полный flush TLB (tlbi vmalle1) — достаточно сменить TTBR0.
use crate::{pmm, smp};
use core::arch::asm;
use core::ptr::addr_of;
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE: usize = 4096;
const BLOCK: usize = 2 * 1024 * 1024;

const TABLE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;
const BLOCK_ADDR_MASK: u64 = 0x0000_FFFF_FFE0_0000;
const ATTR_MASK: u64 = 0xFFF0_0000_0000_0FFC; // биты 63:52 и 11:2

// Индексы MAIR_EL1 (см. MAIR значение в enable()).
const IDX_DEV: u64 = 0; // Device-nGnRnE
const IDX_WB: u64 = 1; // Normal, Write-Back

const AF: u64 = 1 << 10;
const SH_OUTER: u64 = 0b10 << 8;
const SH_INNER: u64 = 0b11 << 8;
const AP_RO: u64 = 1 << 7; // EL1 read-only
const AP_USER: u64 = 1 << 6; // доступ из EL0
const NG: u64 = 1 << 11; // not-global
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

const ASID_SHIFT: usize = 48;
const ASID_MASK: u64 = 0xff << ASID_SHIFT;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mem {
    Device,   // MMIO
    Normal,   // RAM, RW, не исполняемая
    Text,     // RX
    Rodata,   // RO, не исполняемая
    Unmapped, // убрать отображение
    UserText, // EL0: RX (EL1 не исполняет)
    UserData, // EL0: RW, не исполняемая
    UserRo,   // EL0: только чтение
}

fn attrs(m: Mem) -> u64 {
    match m {
        Mem::Device => (IDX_DEV << 2) | SH_OUTER | AF | PXN | UXN,
        Mem::Normal => (IDX_WB << 2) | SH_INNER | AF | PXN | UXN,
        Mem::Text => (IDX_WB << 2) | SH_INNER | AF | AP_RO | UXN,
        Mem::Rodata => (IDX_WB << 2) | SH_INNER | AF | AP_RO | PXN | UXN,
        Mem::Unmapped => 0,
        Mem::UserText => (IDX_WB << 2) | SH_INNER | AF | NG | AP_USER | AP_RO | PXN,
        Mem::UserRo => (IDX_WB << 2) | SH_INNER | AF | NG | AP_USER | AP_RO | PXN | UXN,
        Mem::UserData => (IDX_WB << 2) | SH_INNER | AF | NG | AP_USER | PXN | UXN,
    }
}

extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __bss_start: u8;
}

struct Layout {
    kernel: (usize, usize),
    text: (usize, usize),
    rodata: (usize, usize),
    data: (usize, usize),
    tail: (usize, usize), // .bss + стек
}

fn layout() -> Layout {
    unsafe {
        let a = |p: *const u8| p as usize;
        Layout {
            kernel: (a(addr_of!(__kernel_start)), a(addr_of!(__kernel_end))),
            text: (a(addr_of!(__text_start)), a(addr_of!(__text_end))),
            rodata: (a(addr_of!(__rodata_start)), a(addr_of!(__rodata_end))),
            data: (a(addr_of!(__data_start)), a(addr_of!(__bss_start))),
            tail: (a(addr_of!(__bss_start)), a(addr_of!(__kernel_end))),
        }
    }
}

/// Глобальный аллокатор ASID. ASID 0 зарезервирован для ядра.
static NEXT_ASID: AtomicUsize = AtomicUsize::new(1);

/// Выделяет новый ASID (1..=255). TCR_EL1.AS=0, поэтому ASID 8-битный:
/// без маски он вылез бы в биты физического адреса TTBR0.
pub fn alloc_asid() -> usize {
    loop {
        let a = NEXT_ASID.fetch_add(1, Ordering::Relaxed) & 0xff;
        if a != 0 {
            return a;
        }
    }
}

/// Текущий корень таблиц страниц и ASID на каждом CPU (TTBR0 совмещает оба).
/// Глобальные переменные здесь недопустимы: у каждого ядра свой TTBR0_EL1.
static CUR_ROOT: [AtomicUsize; smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; smp::MAX_CPUS];
static CUR_ASID: [AtomicUsize; smp::MAX_CPUS] =
    [const { AtomicUsize::new(0) }; smp::MAX_CPUS];

/// Физический адрес корневой таблицы ядра (TTBR0 по умолчанию, ASID=0).
pub fn kernel_root() -> usize {
    KROOT.load(Ordering::Relaxed)
}

/// Текущий ASID на этом CPU.
pub fn current_asid() -> usize {
    CUR_ASID[smp::current_cpu_index()].load(Ordering::Relaxed)
}

/// Дескриптор страницы L3 для `pa` с атрибутами `mem`.
pub fn page_descriptor(pa: usize, mem: Mem) -> u64 {
    pa as u64 | 0b11 | attrs(mem)
}

/// Переключает TTBR0_EL1, если корень или ASID изменились.
/// При смене ASID его старые TLB-записи сбрасываются на всех ядрах: ASID
/// переиспользуются после 255 процессов, и без сброса новое адресное
/// пространство могло бы увидеть чужие отображения.
/// Вызывать при замаскированных IRQ.
pub fn switch_ttbr0(root: usize, asid: usize) {
    let cpu = smp::current_cpu_index();
    let cur_root = CUR_ROOT[cpu].load(Ordering::Relaxed);
    let cur_asid = CUR_ASID[cpu].load(Ordering::Relaxed);
    if cur_root == root && cur_asid == asid {
        return;
    }
    unsafe {
        if cur_asid != asid {
            asm!(
                "dsb ishst",
                "tlbi aside1is, {a}",
                "dsb ish",
                "isb",
                a = in(reg) asid as u64,
                options(nostack),
            );
        }
        asm!(
            "msr ttbr0_el1, {r}",
            "isb",
            r = in(reg) (root as u64 | ((asid as u64) << ASID_SHIFT)),
            options(nostack),
        );
    }
    CUR_ROOT[cpu].store(root, Ordering::Relaxed);
    CUR_ASID[cpu].store(asid, Ordering::Relaxed);
}

/// [начало, конец) образа ядра в физической памяти.
pub fn kernel_range() -> (usize, usize) {
    layout().kernel
}

/// Выделяет одну страницу таблиц из PMM. Возвращает указатель на массив из 512 u64.
fn alloc_table() -> Option<*mut u64> {
    pmm::alloc_page().map(|pa| pa as *mut u64)
}

/// Освобождает страницу таблиц в PMM.
fn free_table(pa: usize) {
    pmm::free_page(pa);
}

#[repr(C, align(4096))]
struct Table([u64; 512]);

struct Mapper {
    root: *mut u64,
}

impl Mapper {
    fn new() -> Option<Self> {
        let root = alloc_table()?;
        unsafe { core::ptr::write_bytes(root, 0, PAGE) };
        Some(Self { root })
    }

    unsafe fn next_table(&mut self, parent: *mut u64, i: usize) -> Option<*mut u64> {
        let e = parent.add(i);
        let v = e.read();
        if v & 0b11 == 0b11 {
            return Some((v & TABLE_ADDR_MASK) as *mut u64);
        }
        if v != 0 {
            return None;
        }
        let t = alloc_table()?;
        core::ptr::write_bytes(t, 0, PAGE);
        e.write(t as u64 | 0b11);
        Some(t as *mut u64)
    }

    unsafe fn l2(&mut self, va: usize) -> Option<*mut u64> {
        let l1 = self.next_table(self.root, idx(va, 39))?;
        self.next_table(l1, idx(va, 30))
    }

    /// Таблица L3 для `va`; 2-МиБ блок при необходимости разбивается на 512 страниц.
    unsafe fn l3(&mut self, va: usize) -> Option<*mut u64> {
        let l2 = self.l2(va)?;
        let e = l2.add(idx(va, 21));
        let v = e.read();
        if v & 0b11 == 0b11 {
            return Some((v & TABLE_ADDR_MASK) as *mut u64);
        }
        let t = alloc_table()?;
        if v & 0b11 == 0b01 {
            let base = v & BLOCK_ADDR_MASK;
            let attr = v & ATTR_MASK;
            for i in 0..512usize {
                t.add(i).write((base + (i * PAGE) as u64) | 0b11 | attr);
            }
        }
        e.write(t as u64 | 0b11);
        Some(t as *mut u64)
    }

    /// Идентичное отображение [start, end): блоками по 2 МиБ где возможно, иначе страницами.
    unsafe fn map(&mut self, start: usize, end: usize, mem: Mem) {
        let a = attrs(mem);
        let mut addr = start & !(PAGE - 1);
        let end = (end + PAGE - 1) & !(PAGE - 1);
        while addr < end {
            if addr % BLOCK == 0 && end - addr >= BLOCK {
                let Some(l2) = self.l2(addr) else { return };
                let d = if mem == Mem::Unmapped { 0 } else { addr as u64 | 0b01 | a };
                l2.add(idx(addr, 21)).write(d);
                addr += BLOCK;
            } else {
                let Some(l3) = self.l3(addr) else { return };
                let d = if mem == Mem::Unmapped { 0 } else { addr as u64 | 0b11 | a };
                l3.add(idx(addr, 12)).write(d);
                addr += PAGE;
            }
        }
    }
}

fn idx(va: usize, shift: usize) -> usize {
    (va >> shift) & 0x1ff
}

unsafe fn dcache_inval(start: usize, end: usize) {
    let ctr: u64;
    asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags));
    let line = 4usize << ((ctr >> 16) & 0xf); // DminLine
    let mut a = start & !(line - 1);
    while a < end {
        asm!("dc ivac, {}", in(reg) a, options(nostack));
        a += line;
    }
    asm!("dsb sy", options(nostack));
}

unsafe fn enable(root: usize, asid: usize) {
    let mmfr0: u64;
    asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack, preserves_flags));
    let ips = core::cmp::min(mmfr0 & 0xf, 5); // не больше 48 бит PA
    // T0SZ=16, IRGN0/ORGN0=WB, SH0=inner, TG0=4K, T1SZ=16, EPD1=1, IPS
    let tcr: u64 = 16 | (1 << 8) | (1 << 10) | (3 << 12) | (16 << 16) | (1 << 23) | (ips << 32);
    // attr0=0x00 Device-nGnRnE, attr1=0xFF Normal WB
    let mair: u64 = 0x0000_FF00;
    // SCTLR_EL1: M | C | SA | SA0 | I
    let bits: u64 = (1 << 0) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 12);
    asm!(
        "dsb sy",
        "msr mair_el1, {mair}",
        "msr tcr_el1, {tcr}",
        "msr ttbr0_el1, {r}",
        "isb",
        "tlbi vmalle1",
        "dsb nsh",
        "isb",
        "ic iallu",
        "dsb nsh",
        "isb",
        "mrs {t}, sctlr_el1",
        "orr {t}, {t}, {bits}",
        "msr sctlr_el1, {t}",
        "isb",
        mair = in(reg) mair,
        tcr = in(reg) tcr,
        r = in(reg) (root as u64 | ((asid as u64) << ASID_SHIFT)),
        bits = in(reg) bits,
        t = out(reg) _,
        options(nostack),
    );
}

/// Строит таблицы и включает MMU + кэши на boot CPU. Возвращает (root_pa, asid).
///
/// * `ram` — все диапазоны RAM из DTB (Normal, RW, XN);
/// * `no_map` — области reserved-memory с `no-map` (отображение убирается);
/// * `mmio` — диапазоны (база, размер) устройств, которые использует ядро (Device).
pub unsafe fn enable_kernel_mmu(
    ram: &[(u64, u64)],
    no_map: &[(u64, u64)],
    mmio: &[(usize, usize)],
) -> (usize, usize) {
    let l = layout();
    let mut m = Mapper::new().expect("mmu: cannot allocate root table");

    if let Some(lo) = ram.iter().map(|r| r.0 as usize).min() {
        let top = core::cmp::min(lo & !(BLOCK - 1), 4usize << 30);
        m.map(0, top, Mem::Device);
    }
    for &(b, s) in ram {
        m.map(b as usize, (b + s) as usize, Mem::Normal);
    }

    // Образ ядра: W^X.
    m.map(l.text.0, l.text.1, Mem::Text);
    m.map(l.rodata.0, l.rodata.1, Mem::Rodata);
    m.map(l.data.0, l.data.1, Mem::Normal);
    m.map(l.tail.0, l.tail.1, Mem::Normal);

    // no-map: не отображаем (кроме пересечений с образом ядра — их игнорируем).
    for &(b, s) in no_map {
        let (nb, ne) = (b as usize, (b + s) as usize);
        if nb < l.kernel.1 && ne > l.kernel.0 {
            continue;
        }
        m.map(nb, ne, Mem::Unmapped);
    }

    // Устройства — после no-map, чтобы они имели приоритет.
    for &(b, sz) in mmio {
        if sz != 0 {
            m.map(b, b + sz, Mem::Device);
        }
    }

    let root = m.root as usize;
    let asid = 0; // ядро использует ASID 0
    let cpu = smp::current_cpu_index();
    KROOT.store(root, Ordering::Relaxed);
    CUR_ROOT[cpu].store(root, Ordering::Relaxed);
    CUR_ASID[cpu].store(asid, Ordering::Relaxed);
    dcache_inval(l.kernel.0, l.kernel.1);
    enable(root, asid);
    (root, asid)
}

/// Включает MMU и кэши на вторичном CPU с уже построенной корневой таблицей.
pub unsafe fn enable_secondary(root: usize, asid: usize) {
    enable(root, asid);
    let cpu = smp::current_cpu_index();
    CUR_ROOT[cpu].store(root, Ordering::Relaxed);
    CUR_ASID[cpu].store(asid, Ordering::Relaxed);
}

static KROOT: AtomicUsize = AtomicUsize::new(0);
