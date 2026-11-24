//! Minimal virtio-blk MMIO (version 2) driver, polling mode.
//! Supports a single block device discovered from FDT virtio-mmio nodes.
use crate::{fdt::BootInfo, sync::SpinLock};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

const MMIO_MAGIC: usize = 0x000;
const MMIO_VERSION: usize = 0x004;
const MMIO_DEVICE_ID: usize = 0x008;
const MMIO_DEVICE_FEATURES: usize = 0x010;
const MMIO_DEVICE_FEATURES_SEL: usize = 0x014;
const MMIO_DRIVER_FEATURES: usize = 0x020;
const MMIO_DRIVER_FEATURES_SEL: usize = 0x024;
const MMIO_QUEUE_SEL: usize = 0x030;
const MMIO_QUEUE_NUM_MAX: usize = 0x034;
const MMIO_QUEUE_NUM: usize = 0x038;
const MMIO_QUEUE_READY: usize = 0x044;
const MMIO_QUEUE_NOTIFY: usize = 0x050;
const MMIO_INTERRUPT_STATUS: usize = 0x060;
const MMIO_INTERRUPT_ACK: usize = 0x064;
const MMIO_STATUS: usize = 0x070;
const MMIO_QUEUE_DESC_LOW: usize = 0x080;
const MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const MMIO_QUEUE_DRIVER_LOW: usize = 0x090;
const MMIO_QUEUE_DRIVER_HIGH: usize = 0x094;
const MMIO_QUEUE_DEVICE_LOW: usize = 0x0a0;
const MMIO_QUEUE_DEVICE_HIGH: usize = 0x0a4;
const MMIO_CONFIG: usize = 0x100;

// Legacy v1 регистры (окно legacy — адреса ниже общих):
const MMIO_GUEST_PAGE_SIZE: usize = 0x028;
const MMIO_QUEUE_ALIGN: usize = 0x03c;
const MMIO_QUEUE_PFN: usize = 0x040;
const PAGE_SIZE: usize = 4096;

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_FAILED: u32 = 128;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_S_OK: u8 = 0;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

const QUEUE_SIZE: usize = 8;
pub const SECTOR_SIZE: usize = 512;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(2))]
struct Avail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE],
    used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UsedElem {
    id: u32,
    len: u32,
}

#[repr(C, align(4))]
struct Used {
    flags: u16,
    idx: u16,
    ring: [UsedElem; QUEUE_SIZE],
    avail_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BlkReq {
    type_: u32,
    reserved: u32,
    sector: u64,
}

/// Смещение кольца `used` в легаси v1: QEMU вычисляет его как
/// `align_up(desc + num * sizeof(Desc) + offsetof(Avail, ring[num]), align)` —
/// для num=8 и align=4096 с page-aligned `desc` это `desc + 4096`.
const USED_OFF: usize = 4096;

/// Кольца virtio лежат в статической памяти; для legacy v1 `desc` должен начинаться
/// на границе страницы (см. `MMIO_QUEUE_PFN`), а `used` — на второй странице.
#[repr(C, align(4096))]
struct VirtioBlk {
    desc: [Desc; QUEUE_SIZE], // +0..128
    avail: Avail,             // +128..150
    pad: [u8; USED_OFF - 128 - 22], // +150..4096 (заполнение для `used`)
    used: Used,               // +4096
    base: usize,
    capacity_sectors: u64,
    ready: bool,
    req: BlkReq,
    status: u8,
    data: [u8; SECTOR_SIZE],
    last_used_idx: u16,
}

const _: () = {
    assert!(core::mem::size_of::<Desc>() == 16);
    assert!(core::mem::offset_of!(VirtioBlk, desc) == 0);
    assert!(core::mem::offset_of!(VirtioBlk, avail) == 128);
    assert!(core::mem::offset_of!(VirtioBlk, used) == USED_OFF);
};

impl VirtioBlk {
    const fn new() -> Self {
        Self {
            desc: [Desc { addr: 0, len: 0, flags: 0, next: 0 }; QUEUE_SIZE],
            avail: Avail { flags: 0, idx: 0, ring: [0; QUEUE_SIZE], used_event: 0 },
            pad: [0; USED_OFF - 128 - 22],
            used: Used { flags: 0, idx: 0, ring: [UsedElem { id: 0, len: 0 }; QUEUE_SIZE], avail_event: 0 },
            base: 0,
            capacity_sectors: 0,
            ready: false,
            req: BlkReq { type_: 0, reserved: 0, sector: 0 },
            status: 0xff,
            data: [0; SECTOR_SIZE],
            last_used_idx: 0,
        }
    }
}

static BLK: SpinLock<VirtioBlk> = SpinLock::new(VirtioBlk::new());

#[inline(always)]
unsafe fn mmio_write32(base: usize, off: usize, val: u32) {
    write_volatile((base + off) as *mut u32, val);
}
#[inline(always)]
unsafe fn mmio_read32(base: usize, off: usize) -> u32 {
    read_volatile((base + off) as *const u32)
}
#[inline(always)]
unsafe fn mmio_read64(base: usize, off: usize) -> u64 {
    let lo = read_volatile((base + off) as *const u32) as u64;
    let hi = read_volatile((base + off + 4) as *const u32) as u64;
    lo | (hi << 32)
}

fn phys_addr<T>(r: &T) -> u64 {
    r as *const T as usize as u64
}

pub fn init(info: &BootInfo) {
    crate::kprintln!("VBLK scanning {} virtio-mmio nodes", info.virtio_mmio_n);
    for i in 0..info.virtio_mmio_n {
        let (base, _size, _irq) = info.virtio_mmio[i];
        unsafe {
            let magic = mmio_read32(base, MMIO_MAGIC);
            let ver = mmio_read32(base, MMIO_VERSION);
            let dev = mmio_read32(base, MMIO_DEVICE_ID);
            crate::kprintln!("VBLK  node{} base={:#x} magic={:#x} ver={} dev={}", i, base, magic, ver, dev);
            if magic != 0x7472_6976 { continue; }
            if ver != 1 && ver != 2 { continue; }
            if dev != 2 { continue; }
            if try_init(base).is_ok() {
                let cap = BLK.lock().capacity_sectors;
                crate::kprintln!("VBLK virtio-blk at {:#x}, {} sectors", base, cap);
                return;
            }
        }
    }
    crate::kprintln!("VBLK no virtio-blk device found");
}

unsafe fn try_init(base: usize) -> Result<(), ()> {
    mmio_write32(base, MMIO_STATUS, 0);
    mmio_write32(base, MMIO_STATUS, STATUS_ACK);
    mmio_write32(base, MMIO_STATUS, STATUS_ACK | STATUS_DRIVER);

    if mmio_read32(base, MMIO_VERSION) == 1 {
        try_init_legacy(base)
    } else {
        try_init_modern(base)
    }
}

/// Legacy v1: кольца активируются записью QUEUE_PFN (см. `USED_OFF`), QUEUE_READY
/// и отдельные селекторы фич в этом режиме не используются.
unsafe fn try_init_legacy(base: usize) -> Result<(), ()> {
    mmio_write32(base, MMIO_GUEST_PAGE_SIZE, PAGE_SIZE as u32);
    mmio_write32(base, MMIO_DRIVER_FEATURES_SEL, 0);
    mmio_write32(base, MMIO_DRIVER_FEATURES, 0);
    mmio_write32(base, MMIO_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
    mmio_write32(base, MMIO_QUEUE_SEL, 0);
    let qmax = mmio_read32(base, MMIO_QUEUE_NUM_MAX) as usize;
    if qmax < QUEUE_SIZE || qmax == 0 {
        mmio_write32(base, MMIO_STATUS, STATUS_FAILED);
        return Err(());
    }
    mmio_write32(base, MMIO_QUEUE_NUM, QUEUE_SIZE as u32);
    mmio_write32(base, MMIO_QUEUE_ALIGN, PAGE_SIZE as u32);
    let mut dev = BLK.lock();
    dev.base = base;
    dev.capacity_sectors = mmio_read64(base, MMIO_CONFIG);
    dev.last_used_idx = 0;
    dev.desc = [Desc { addr: 0, len: 0, flags: 0, next: 0 }; QUEUE_SIZE];
    dev.avail.flags = 0;
    dev.avail.idx = 0;
    dev.avail.used_event = 0;
    dev.used.flags = 0;
    dev.used.idx = 0;
    dev.used.avail_event = 0;
    let pfn = (phys_addr(&dev.desc) >> 12) as u32;
    mmio_write32(base, MMIO_QUEUE_PFN, pfn);
    if mmio_read32(base, MMIO_QUEUE_PFN) != pfn {
        mmio_write32(base, MMIO_STATUS, STATUS_FAILED);
        return Err(());
    }
    mmio_write32(base, MMIO_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
    dev.ready = true;
    Ok(())
}

unsafe fn try_init_modern(base: usize) -> Result<(), ()> {
    let mut status = STATUS_ACK | STATUS_DRIVER;

    mmio_write32(base, MMIO_DEVICE_FEATURES_SEL, 0);
    let _feat0 = mmio_read32(base, MMIO_DEVICE_FEATURES);
    mmio_write32(base, MMIO_DRIVER_FEATURES_SEL, 0);
    mmio_write32(base, MMIO_DRIVER_FEATURES, 0);
    mmio_write32(base, MMIO_DEVICE_FEATURES_SEL, 1);
    let _feat1 = mmio_read32(base, MMIO_DEVICE_FEATURES);
    mmio_write32(base, MMIO_DRIVER_FEATURES_SEL, 1);
    mmio_write32(base, MMIO_DRIVER_FEATURES, 0);

    status |= STATUS_FEATURES_OK;
    mmio_write32(base, MMIO_STATUS, status);
    if mmio_read32(base, MMIO_STATUS) & STATUS_FEATURES_OK == 0 {
        mmio_write32(base, MMIO_STATUS, STATUS_FAILED);
        return Err(());
    }

    mmio_write32(base, MMIO_QUEUE_SEL, 0);
    let qmax = mmio_read32(base, MMIO_QUEUE_NUM_MAX) as usize;
    if qmax < QUEUE_SIZE || qmax == 0 {
        mmio_write32(base, MMIO_STATUS, STATUS_FAILED);
        return Err(());
    }
    mmio_write32(base, MMIO_QUEUE_NUM, QUEUE_SIZE as u32);

    let mut dev = BLK.lock();
    dev.base = base;
    dev.capacity_sectors = mmio_read64(base, MMIO_CONFIG);
    dev.last_used_idx = 0;
    dev.desc = [Desc { addr: 0, len: 0, flags: 0, next: 0 }; QUEUE_SIZE];
    dev.avail.flags = 0;
    dev.avail.idx = 0;
    dev.avail.used_event = 0;
    dev.used.flags = 0;
    dev.used.idx = 0;
    dev.used.avail_event = 0;

    let desc = phys_addr(&dev.desc);
    let avail = phys_addr(&dev.avail);
    let used = phys_addr(&dev.used);
    mmio_write32(base, MMIO_QUEUE_DESC_LOW, desc as u32);
    mmio_write32(base, MMIO_QUEUE_DESC_HIGH, (desc >> 32) as u32);
    mmio_write32(base, MMIO_QUEUE_DRIVER_LOW, avail as u32);
    mmio_write32(base, MMIO_QUEUE_DRIVER_HIGH, (avail >> 32) as u32);
    mmio_write32(base, MMIO_QUEUE_DEVICE_LOW, used as u32);
    mmio_write32(base, MMIO_QUEUE_DEVICE_HIGH, (used >> 32) as u32);
    mmio_write32(base, MMIO_QUEUE_READY, 1);

    status |= STATUS_DRIVER_OK;
    mmio_write32(base, MMIO_STATUS, status);
    dev.ready = true;
    Ok(())
}

pub fn is_ready() -> bool {
    BLK.lock().ready
}

pub fn capacity_sectors() -> u64 {
    BLK.lock().capacity_sectors
}

fn do_io(sector: u64, buf: &mut [u8; SECTOR_SIZE], write: bool) -> Result<(), ()> {
    let mut dev = BLK.lock();
    crate::kprintln!("DBG do_io: sec={} write={} ready={} cap={}", sector, write, dev.ready, dev.capacity_sectors);
    if !dev.ready || sector >= dev.capacity_sectors {
        return Err(());
    }

    dev.req.type_ = if write { VIRTIO_BLK_T_OUT } else { VIRTIO_BLK_T_IN };
    dev.req.reserved = 0;
    dev.req.sector = sector;
    dev.status = 0xff;
    if write {
        dev.data.copy_from_slice(buf);
    }

    dev.desc[0] = Desc { addr: phys_addr(&dev.req), len: core::mem::size_of::<BlkReq>() as u32, flags: DESC_F_NEXT, next: 1 };
    dev.desc[1] = Desc {
        addr: phys_addr(&dev.data),
        len: SECTOR_SIZE as u32,
        flags: DESC_F_NEXT | if write { 0 } else { DESC_F_WRITE },
        next: 2,
    };
    dev.desc[2] = Desc { addr: phys_addr(&dev.status), len: 1, flags: DESC_F_WRITE, next: 0 };

    let avail_idx = dev.avail.idx;
    dev.avail.ring[(avail_idx as usize) % QUEUE_SIZE] = 0;
    fence(Ordering::SeqCst);
    dev.avail.idx = avail_idx.wrapping_add(1);
    fence(Ordering::SeqCst);
    unsafe { mmio_write32(dev.base, MMIO_QUEUE_NOTIFY, 0); }

    for _ in 0..5_000_000 {
        fence(Ordering::SeqCst);
        let used_idx = unsafe { read_volatile(&dev.used.idx) };
        if used_idx != dev.last_used_idx {
            crate::kprintln!("DBG poll: sec={} used={} last={} status={:#x} avail={}", sector, used_idx, dev.last_used_idx, dev.status, dev.avail.idx);
            dev.last_used_idx = used_idx;
            unsafe {
                let st = mmio_read32(dev.base, MMIO_INTERRUPT_STATUS);
                if st != 0 { mmio_write32(dev.base, MMIO_INTERRUPT_ACK, st); }
            }
            if dev.status == VIRTIO_BLK_S_OK {
                if !write {
                    buf.copy_from_slice(&dev.data);
                }
                return Ok(());
            }
            crate::kprintln!("DBG do_io: bad status sec={} status={:#x} used={} last={} avail={}", sector, dev.status, used_idx, dev.last_used_idx, dev.avail.idx);
            return Err(());
        }
        core::hint::spin_loop();
    }
    crate::kprintln!("DBG do_io: timeout sec={} used={} last={} status={:#x} avail={}", sector, unsafe { read_volatile(&dev.used.idx) }, dev.last_used_idx, dev.status, dev.avail.idx);
    Err(())
}

pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    do_io(sector, buf, false)
}

pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    let mut tmp = *buf;
    do_io(sector, &mut tmp, true)
}
