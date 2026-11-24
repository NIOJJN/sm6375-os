//! Консоль: вывод в UART (PL011). Графическая консоль удалена — ядро использует
//! только последовательный порт, как и положено bare-metal ядру.
use crate::{sync::SpinLock, uart::Pl011};
use core::fmt::{self, Write};

pub struct Console {
    uart: Option<Pl011>,
}

impl Console {
    const fn new() -> Self {
        Self { uart: None }
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if let Some(u) = &self.uart {
                if b == b'\n' {
                    u.putc(b'\r');
                }
                u.putc(b);
            }
        }
        Ok(())
    }
}

static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new());

pub fn init(uart: Option<Pl011>) {
    let mut c = CONSOLE.lock();
    c.uart = uart;
}

pub fn _print(args: fmt::Arguments) {
    let mut c = CONSOLE.lock();
    let _ = c.write_fmt(args);
}

/// Считывает байт с UART, если доступен.
pub fn read_byte() -> Option<u8> {
    let c = CONSOLE.lock();
    c.uart.as_ref().and_then(|u| u.getc())
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => { $crate::console::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($($arg:tt)*) => { $crate::kprint!("{}\n", format_args!($($arg)*)) };
}

/// Write directly to UART without taking console lock. For emergency/debug use only.
pub fn emergency_write(bytes: &[u8]) {
    let uart = 0x0900_0000usize as *mut u32;
    for &b in bytes {
        unsafe { core::ptr::write_volatile(uart, b as u32); }
    }
}

/// Try to print; if console lock is busy, drop.
pub fn try_print(args: fmt::Arguments) {
    if let Some(mut c) = CONSOLE.try_lock() {
        let _ = c.write_fmt(args);
    }
}

pub fn console_free() -> bool {
    CONSOLE.try_lock().is_some()
}
