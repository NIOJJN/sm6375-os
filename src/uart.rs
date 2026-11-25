//! UART PL011 (QEMU virt). На телефоне PL011 нет — используется только если найден в DTB.
use core::ptr::{read_volatile, write_volatile};

const DR: usize = 0x00;
const FR: usize = 0x18;
const LCRH: usize = 0x2C;
const CR: usize = 0x30;
const IMSC: usize = 0x38;
const ICR: usize = 0x44;

#[derive(Clone, Copy)]
pub struct Pl011 {
    base: usize,
}

impl Pl011 {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    pub fn base(&self) -> usize {
        self.base
    }

    unsafe fn w(&self, off: usize, v: u32) {
        write_volatile((self.base + off) as *mut u32, v);
    }

    unsafe fn r(&self, off: usize) -> u32 {
        read_volatile((self.base + off) as *const u32)
    }

    /// Скорость не настраиваем: QEMU её игнорирует, а реальный PL011 уже настроен загрузчиком.
    pub unsafe fn init(&self) {
        self.w(CR, 0);
        self.w(ICR, 0x7ff);
        self.w(LCRH, (3 << 5) | (1 << 4)); // 8N1, FIFO
        self.w(IMSC, 0);
        self.w(CR, (1 << 0) | (1 << 8) | (1 << 9)); // UARTEN | TXE | RXE
    }

    pub fn putc(&self, b: u8) {
        unsafe {
            while self.r(FR) & (1 << 5) != 0 {} // TXFF
            self.w(DR, b as u32);
        }
    }

    pub fn getc(&self) -> Option<u8> {
        unsafe {
            if self.r(FR) & (1 << 4) != 0 {
                None // RXFE
            } else {
                Some((self.r(DR) & 0xff) as u8)
            }
        }
    }
}
