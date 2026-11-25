//! Консоль: вывод в UART (PL011 на QEMU virt, GENI SE UART на телефоне SM6375).
use crate::{drivers::geni_uart, sync::SpinLock, uart::Pl011};
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// База PL011 для emergency_write (0 — PL011 нет).
static EMERG_PL011: AtomicUsize = AtomicUsize::new(0);
static EMERG_GENI: AtomicBool = AtomicBool::new(false);

pub struct Console {
    uart: Option<Pl011>,
    /// Базовый адрес GENI UART (когда PL011 отсутствует — реальное железо).
    geni: bool,
}

impl Console {
    const fn new() -> Self {
        Self { uart: None, geni: false }
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            let nl = b == b'\n';
            // PL011 (QEMU virt)
            if let Some(u) = &self.uart {
                if nl {
                    u.putc(b'\r');
                }
                u.putc(b);
            }
            // GENI SE UART (телефон). Когда доступны оба — пишем в оба:
            // на QEMU это тренирует GENI-путь, на телефоне PL011 нет.
            if self.geni {
                if nl {
                    geni_uart::putc(b'\r');
                }
                geni_uart::putc(b);
            }
        }
        Ok(())
    }
}

static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new());

/// Инициализация консоли: PL011 (QEMU) и/или GENI UART (телефон).
pub fn init(uart: Option<Pl011>, geni: Option<usize>) {
    EMERG_PL011.store(uart.as_ref().map(|u| u.base()).unwrap_or(0), Ordering::Release);
    EMERG_GENI.store(geni.is_some(), Ordering::Release);
    let mut c = CONSOLE.lock();
    c.uart = uart;
    c.geni = geni.is_some();
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
    // Адрес берём из инициализированной консоли: на телефоне 0x0900_0000 не замаплен.
    let pl011 = EMERG_PL011.load(Ordering::Acquire);
    let geni = EMERG_GENI.load(Ordering::Acquire);
    for &b in bytes {
        if pl011 != 0 {
            unsafe { core::ptr::write_volatile(pl011 as *mut u32, b as u32); }
        }
        if geni {
            if b == b'\n' {
                geni_uart::putc(b'\r');
            }
            geni_uart::putc(b);
        }
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
