//! Пользовательское пространство (EL0). Одна программа, роль выбирает загрузчик:
//! 3 — самотест ядра, 4 — «leaker» (проверка освобождения памяти), 5 — shell,
//! 6 — init (PID 1, запускает остальную ОС).
#![no_std]
#![no_main]

#[macro_use]
mod sys;
mod init;
mod mem;
mod selftest;
mod shell;

use core::panic::PanicInfo;

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(role: u64, _arg1: u64) -> ! {
    match role {
        3 => selftest::run(),
        4 => selftest::leaker(),
        5 => shell::run(),
        6 => init::run(),
        _ => sys::exit(77),
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    uprintln!("user panic: {}", info);
    sys::exit(100)
}
