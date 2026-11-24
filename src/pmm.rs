//! Физический аллокатор страниц 4 КиБ: bump-указатель по свободному диапазону RAM
//! + односвязный список освобождённых страниц. Пропускает зарезервированные диапазоны
//! (образ ядра, DTB, фреймбуфер, no-map).
//!
//! Защищён межъядерным `SpinLock`: после старта вторичных CPU аллокатор можно
//! вызывать с любого ядра.
use crate::sync::SpinLock;
use core::ptr::{read_volatile, write_bytes, write_volatile};

pub const PAGE_SIZE: usize = 4096;
/// Вместимость списка исключений: ядро + DTB + no-map + вся reserved-memory
/// (на SM6375 ~20+ регионов).
const MAX_RES: usize = 256;

struct Pmm {
    next: usize,
    end: usize,
    free_head: usize,
    free_count: usize,
    allocated: usize,
    res: [(usize, usize); MAX_RES],
    res_n: usize,
}

static PMM: SpinLock<Pmm> = SpinLock::new(Pmm {
    next: 0,
    end: 0,
    free_head: 0,
    free_count: 0,
    allocated: 0,
    res: [(0, 0); MAX_RES],
    res_n: 0,
});

fn align_up(x: usize) -> usize {
    (x + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Отдаёт аллокатору [start, end). `reserved` — байтовые диапазоны [s, e), которые нельзя выдавать.
pub fn init(start: usize, end: usize, reserved: &[(usize, usize)]) {
    let mut p = PMM.lock();
    p.next = align_up(start);
    p.end = end & !(PAGE_SIZE - 1);
    p.free_head = 0;
    p.free_count = 0;
    p.allocated = 0;
    p.res_n = 0;
    for &r in reserved.iter().take(MAX_RES) {
        let i = p.res_n;
        p.res[i] = r;
        p.res_n += 1;
    }
}

/// Возвращает физический адрес обнулённой страницы.
pub fn alloc_page() -> Option<usize> {
    let mut p = PMM.lock();
    if p.free_head != 0 {
        let page = p.free_head;
        p.free_head = unsafe { read_volatile(page as *const usize) };
        p.free_count -= 1;
        p.allocated += 1;
        unsafe { write_bytes(page as *mut u8, 0, PAGE_SIZE) };
        return Some(page);
    }
    loop {
        if p.next + PAGE_SIZE > p.end {
            crate::kprintln!("[pmm] OOM next={:#x} end={:#x} free={} alloc={}", p.next, p.end, p.free_count, p.allocated);
            return None;
        }
        let a = p.next;
        let mut skipped = false;
        for i in 0..p.res_n {
            let (s, e) = p.res[i];
            if a < e && a + PAGE_SIZE > s {
                p.next = align_up(e);
                skipped = true;
                break;
            }
        }
        if skipped {
            continue;
        }
        p.next = a + PAGE_SIZE;
        p.allocated += 1;
        unsafe { write_bytes(a as *mut u8, 0, PAGE_SIZE) };
        return Some(a);
    }
}

/// Возвращает страницу, полученную из `alloc_page`.
pub fn free_page(pa: usize) {
    if pa == 0 || pa % PAGE_SIZE != 0 {
        unsafe {
            let uart = 0x0900_0000usize as *mut u32;
            for b in b"BAD_FREE\n" { core::ptr::write_volatile(uart, *b as u32); }
        }
        crate::kprintln!("[pmm] BAD FREE pa={:#x}", pa);
        loop { unsafe { core::arch::asm!("wfe", options(nomem, nostack)) }; }
    }
    let mut p = PMM.lock();
    unsafe { write_volatile(pa as *mut usize, p.free_head) };
    p.free_head = pa;
    p.free_count += 1;
    p.allocated -= 1;
}

/// Приблизительное число свободных страниц (дыры из `reserved` не вычитаются).
pub fn free_pages_estimate() -> usize {
    let p = PMM.lock();
    (p.end.saturating_sub(p.next)) / PAGE_SIZE + p.free_count
}

pub fn allocated_pages() -> usize {
    PMM.lock().allocated
}

/// Выделяет физически непрерывный буфер `size` байт (выровненный на страницу).
/// Используется для initramfs и других больших буферов.
/// Сначала пробует bump pointer, затем ищет в free list.
pub fn alloc_contiguous(size: usize) -> Option<usize> {
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut p = PMM.lock();

    // 1. Пробуем через bump pointer (самый быстрый путь).
    let aligned_next = (p.next + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end = aligned_next + pages * PAGE_SIZE;
    if end <= p.end {
        let mut ok = true;
        for &(rs, re) in &p.res[..p.res_n] {
            if aligned_next < re && end > rs {
                ok = false;
                break;
            }
        }
        if ok {
            p.next = end;
            p.allocated += pages;
            for k in 0..pages {
                unsafe { write_bytes((aligned_next + k * PAGE_SIZE) as *mut u8, 0, PAGE_SIZE) };
            }
            return Some(aligned_next);
        }
    }

    // 2. Ищем в free list: проход по связанному списку.
    // Для простоты ищем блок, начиная с каждой свободной страницы.
    if p.free_head != 0 && pages <= 256 {
        // Ограничиваем поиск разумным максимумом.
        let mut cur = p.free_head;
        while cur != 0 {
            // Проверяем, есть ли `pages` непрерывных страниц начиная с `cur`.
            let mut ok = true;
            let mut next = cur;
            for _ in 1..pages {
                next = unsafe { read_volatile(next as *const usize) };
                if next == 0 || next != cur + PAGE_SIZE {
                    ok = false;
                    break;
                }
            }
            if ok {
                // Нашли! Удаляем эти страницы из free list.
                // Для простоты перестраиваем весь free list без найденного блока.
                let block_end = cur + pages * PAGE_SIZE;
                let mut new_head = 0;
                let mut new_tail = 0;
                let mut prev = 0;
                let mut scan = p.free_head;
                while scan != 0 {
                    let next = unsafe { read_volatile(scan as *const usize) };
                    if scan < cur || scan >= block_end {
                        if new_head == 0 {
                            new_head = scan;
                        } else {
                            unsafe { write_volatile(new_tail as *mut usize, scan) };
                        }
                        new_tail = scan;
                    }
                    scan = next;
                }
                if new_tail != 0 {
                    unsafe { write_volatile(new_tail as *mut usize, 0) };
                }
                p.free_head = new_head;
                p.free_count -= pages;
                p.allocated += pages;
                for k in 0..pages {
                    unsafe { write_bytes((cur + k * PAGE_SIZE) as *mut u8, 0, PAGE_SIZE) };
                }
                return Some(cur);
            }
            cur = unsafe { read_volatile(cur as *const usize) };
        }
    }

    None
}
