//! Адресное пространство процесса: свой корень L0 + копия L1 ядра (общие L2 ядра, доступны только из EL1)
//! + пользовательское окно USER_BASE..USER_END, которое собирается из страниц PMM.
//!
//! Владение страницами: в дескрипторе страницы бит 55 (software) = OWNED — страница принадлежит процессу
//! и освобождается при разрушении адресного пространства. Страницы shm и MMIO не помечаются (у них свои счётчики).
use crate::{
    mmu::{self, Mem},
    pmm,
    sync::SpinLock,
};

/// Блокировка на каждое адресное пространство: таблицы страниц процесса
/// могут менять несколько его потоков одновременно на разных CPU.
static PT_LOCKS: [SpinLock<()>; 256] = [const { SpinLock::new(()) }; 256];

fn pt_lock(asid: usize) -> crate::sync::SpinGuard<'static, ()> {
    PT_LOCKS[asid & 0xff].lock()
}

pub const USER_BASE: usize = 0x40_0000_0000; // 256 ГиБ: выше любой RAM/MMIO, что ядро отображает 1:1
pub const USER_END: usize = USER_BASE + 0x4000_0000; // окно 1 ГиБ (ровно одна запись L1)
/// Окно анонимной памяти (куча, стеки потоков): между окном shm и окном устройств.
pub const HEAP_BASE: usize = USER_BASE + 0x20_0000;
pub const HEAP_END: usize = USER_BASE + 0x1000_0000;

const ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;
const OWNED: u64 = 1 << 55;
const AP_RO: u64 = 1 << 7;

fn idx(va: usize, shift: usize) -> usize {
    (va >> shift) & 0x1ff
}

pub struct AddressSpace {
    pub root: usize,
    pub asid: usize,
}

impl AddressSpace {
    pub fn new() -> Option<Self> {
        let kl0 = mmu::kernel_root() as *const u64;
        let kl1 = unsafe { (kl0.read() & ADDR_MASK) as *const u64 };
        // Пользовательская запись L1 в таблице ядра обязана быть пустой, иначе L2 были бы общими.
        if unsafe { kl1.add(idx(USER_BASE, 30)).read() } != 0 {
            return None;
        }
        let l0 = pmm::alloc_page()?;
        let Some(l1) = pmm::alloc_page() else {
            pmm::free_page(l0);
            return None;
        };
        unsafe {
            for i in 0..512 {
                (l1 as *mut u64).add(i).write(kl1.add(i).read());
            }
            (l0 as *mut u64).write(l1 as u64 | 0b11);
        }
        let asid = mmu::alloc_asid();
        Some(Self { root: l0, asid })
    }

    unsafe fn table(parent: *mut u64, i: usize) -> Option<*mut u64> {
        let e = parent.add(i);
        let v = e.read();
        if v & 0b11 == 0b11 {
            return Some((v & ADDR_MASK) as *mut u64);
        }
        if v != 0 {
            return None;
        }
        let t = pmm::alloc_page()?;
        e.write(t as u64 | 0b11);
        Some(t as *mut u64)
    }

    /// Отображает страницу `pa` по пользовательскому адресу `va`. `owned` — страница освободится вместе с процессом.
    pub fn map_page(&mut self, va: usize, pa: usize, mem: Mem, owned: bool) -> bool {
        if va < USER_BASE || va >= USER_END || va % 4096 != 0 {
            return false;
        }
        let _pt = pt_lock(self.asid);
        unsafe {
            let Some(l1) = Self::table(self.root as *mut u64, idx(va, 39)) else { return false };
            let Some(l2) = Self::table(l1, idx(va, 30)) else { return false };
            let Some(l3) = Self::table(l2, idx(va, 21)) else { return false };
            let d = mmu::page_descriptor(pa, mem) | if owned { OWNED } else { 0 };
            l3.add(idx(va, 12)).write(d);
        }
        true
    }

    /// Указатель на запись L3 для `va` (None — промежуточных таблиц нет).
    fn l3_entry(&self, va: usize) -> Option<*mut u64> {
        if va < USER_BASE || va >= USER_END {
            return None;
        }
        unsafe {
            let mut t = self.root as *const u64;
            for shift in [39usize, 30, 21] {
                let v = t.add(idx(va, shift)).read();
                if v & 0b11 != 0b11 {
                    return None;
                }
                t = (v & ADDR_MASK) as *const u64;
            }
            Some((t as *mut u64).add(idx(va, 12)))
        }
    }

    /// Снимает отображение страницы. Возвращает (физический адрес, owned). Вызывающий сбрасывает TLB.
    pub fn unmap_page(&mut self, va: usize) -> Option<(usize, bool)> {
        let _pt = pt_lock(self.asid);
        let e = self.l3_entry(va)?;
        let v = unsafe { e.read() };
        if v & 0b11 != 0b11 {
            return None;
        }
        unsafe { e.write(0) };
        Some(((v & ADDR_MASK) as usize, v & OWNED != 0))
    }

    fn leaf(&self, va: usize) -> Option<u64> {
        let v = unsafe { self.l3_entry(va)?.read() };
        if v & 0b11 == 0b11 {
            Some(v)
        } else {
            None
        }
    }

    /// Физический адрес для пользовательского `va` (None — не отображён).
    pub fn translate(&self, va: usize) -> Option<usize> {
        let v = self.leaf(va)?;
        Some((v & ADDR_MASK) as usize | (va & 0xfff))
    }

    /// То же, но только если страница доступна процессу на запись.
    pub fn translate_write(&self, va: usize) -> Option<usize> {
        let v = self.leaf(va)?;
        if v & AP_RO != 0 {
            return None;
        }
        Some((v & ADDR_MASK) as usize | (va & 0xfff))
    }

    /// Безопасное копирование из памяти процесса: проходим по его таблицам, а не обращаемся по VA.
    pub fn copy_from_user(&self, va: usize, dst: &mut [u8]) -> bool {
        match va.checked_add(dst.len()) {
            Some(end) if va >= USER_BASE && end <= USER_END => {}
            _ => return false,
        }
        let mut off = 0;
        while off < dst.len() {
            let a = va + off;
            let Some(pa) = self.translate(a) else { return false };
            let n = core::cmp::min(4096 - (a & 0xfff), dst.len() - off);
            unsafe { core::ptr::copy_nonoverlapping(pa as *const u8, dst[off..].as_mut_ptr(), n) };
            off += n;
        }
        true
    }

    /// Копирование в память процесса (только в страницы, доступные ему на запись).
    pub fn copy_to_user(&self, va: usize, src: &[u8]) -> bool {
        match va.checked_add(src.len()) {
            Some(end) if va >= USER_BASE && end <= USER_END => {}
            _ => return false,
        }
        let mut off = 0;
        while off < src.len() {
            let a = va + off;
            let Some(pa) = self.translate_write(a) else { return false };
            let n = core::cmp::min(4096 - (a & 0xfff), src.len() - off);
            unsafe { core::ptr::copy_nonoverlapping(src[off..].as_ptr(), pa as *mut u8, n) };
            off += n;
        }
        true
    }

    /// Сброс TLB после изменения отображений адресного пространства.
    /// Инвалидация broadcast: отображения процесса используют все ядра (потоки
    /// мигрируют между CPU), поэтому локальный `vmalle1` оставил бы stale-записи.
    pub fn flush_tlb() {
        unsafe {
            core::arch::asm!(
                "dsb ishst",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                options(nostack)
            )
        };
    }

    /// Освобождает всё: страницы с флагом OWNED, пользовательские таблицы L2/L3, а также L1 и L0.
    /// Общие L2-таблицы ядра (скопированные записи L1) не трогаются. Адресное пространство не должно быть активным.
    pub fn destroy(self) {
        unsafe {
            let l0 = self.root as *mut u64;
            let l1v = l0.read();
            let l1 = (l1v & ADDR_MASK) as *mut u64;
            let e = l1.add(idx(USER_BASE, 30)).read();
            if e & 0b11 == 0b11 {
                let l2 = (e & ADDR_MASK) as *mut u64;
                for i in 0..512 {
                    let v = l2.add(i).read();
                    if v & 0b11 != 0b11 {
                        continue;
                    }
                    let l3 = (v & ADDR_MASK) as *mut u64;
                    for j in 0..512 {
                        let leaf = l3.add(j).read();
                        if leaf & 0b11 == 0b11 && leaf & OWNED != 0 {
                            pmm::free_page((leaf & ADDR_MASK) as usize);
                        }
                    }
                    pmm::free_page(l3 as usize);
                }
                pmm::free_page(l2 as usize);
            }
            pmm::free_page(l1 as usize);
            pmm::free_page(l0 as usize);
        }
    }
}
