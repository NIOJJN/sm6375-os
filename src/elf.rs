//! Загрузчик ELF64 (AArch64, ET_EXEC): отображает PT_LOAD-сегменты в адресное пространство процесса.
use crate::{
    aspace::{AddressSpace, USER_BASE},
    mmu::Mem,
    pmm, proc,
};

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
/// Образ обязан помещаться ниже стека (см. proc.rs).
const IMAGE_LIMIT: usize = USER_BASE + 0x70000;

fn rd16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn rd32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Загружает образ, возвращает точку входа. None — образ повреждён или не помещается.
pub fn load(asp: &mut AddressSpace, img: &[u8]) -> Option<usize> {
    if img.len() < 64 || &img[0..4] != b"\x7fELF" || img[4] != 2 || img[5] != 1 {
        return None; // не ELF64 little-endian
    }
    if rd16(img, 16)? != 2 || rd16(img, 18)? != 183 {
        return None; // не ET_EXEC / не AArch64
    }
    let entry = rd64(img, 24)? as usize;
    let phoff = rd64(img, 32)? as usize;
    let phentsize = rd16(img, 54)? as usize;
    let phnum = rd16(img, 56)? as usize;
    if phentsize < 56 {
        return None;
    }
    for i in 0..phnum {
        let ph = phoff.checked_add(i.checked_mul(phentsize)?)?;
        if rd32(img, ph)? != PT_LOAD {
            continue;
        }
        let flags = rd32(img, ph + 4)?;
        let off = rd64(img, ph + 8)? as usize;
        let va = rd64(img, ph + 16)? as usize;
        let filesz = rd64(img, ph + 32)? as usize;
        let memsz = rd64(img, ph + 40)? as usize;
        if memsz == 0 {
            continue;
        }
        if filesz > memsz || off.checked_add(filesz)? > img.len() {
            return None;
        }
        let end = va.checked_add(memsz)?;
        if va < USER_BASE || end > IMAGE_LIMIT {
            return None;
        }
        let mem = if flags & PF_X != 0 {
            Mem::UserText
        } else if flags & PF_W != 0 {
            Mem::UserData
        } else {
            Mem::UserRo
        };
        let mut page = va & !0xfff;
        let last = (end + 0xfff) & !0xfff;
        while page < last {
            if asp.translate(page).is_some() {
                return None; // два сегмента на одной странице не поддерживаются
            }
            let pa = pmm::alloc_page()?; // обнулена: хвост (.bss) уже нулевой
            let lo = core::cmp::max(page, va);
            let hi = core::cmp::min(page + 4096, va + filesz);
            if lo < hi {
                unsafe {
                    core::ptr::copy_nonoverlapping(img.as_ptr().add(off + (lo - va)), (pa + (lo - page)) as *mut u8, hi - lo);
                }
            }
            if flags & PF_X != 0 {
                unsafe { proc::sync_icache(pa, 4096) };
            }
            if !asp.map_page(page, pa, mem, true) {
                pmm::free_page(pa);
                return None;
            }
            page += 4096;
        }
    }
    if entry < USER_BASE || entry >= IMAGE_LIMIT {
        return None;
    }
    Some(entry)
}
