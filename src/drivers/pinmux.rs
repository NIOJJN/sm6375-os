//! Qualcomm TLMM (Top Level Mode Multiplexer) Pinmux driver
//!
//! Позволяет настраивать пины GPIO через pinctrl (TLMM) для SM6375.
//! Базовый адрес: 0x400000 (pinctrl@400000 из DTB)
//!
//! Регистры (offset от TLMM_BASE):
//!   GPIO_CFG: 0x0000 + gpio * 0x10000 (конфигурация пинов)
//!   GPIO_IN:  0x0004 + gpio * 0x10000
//!   GPIO_OUT: 0x0008 + gpio * 0x10000
//!   GPIO_INT: 0x000C + gpio * 0x10000
//!   GPIO_EN:  0x0010 + gpio * 0x10000
//!
//! GPIO_CFG биты:
//!   [2:0]   func:   0=GPIO, 1=func1, 2=func2, ... 7=func7
//!   [3]     dir:    0=input, 1=output
//!   [5:4]   pull:   0=no pull, 1=pull up, 2=pull down, 3=keeper
//!   [7:6]   drive:  0=2mA, 1=4mA, 2=6mA, 3=8mA
//!   [8]     oe:     output enable

use crate::platform::{DriverError, PlatformDevice};
use crate::sync::SpinLock;

/// TLMM базовый адрес (0x400000 для SM6375/Blair)
static TLMM_BASE: SpinLock<Option<usize>> = SpinLock::new(None);

/// Настройка пина через TLMM
/// gpio: номер пина (0..N)
/// func: 0=GPIO, 1..7=функции
/// output: true=output, false=input
/// pull: 0=no pull, 1=pull up, 2=pull down, 3=keeper
/// drive: 0=2mA, 1=4mA, 2=6mA, 3=8mA
pub fn tlmm_config_gpio(gpio: u32, func: u32, output: bool, pull: u32, drive: u32) -> Result<(), DriverError> {
    let base = *TLMM_BASE.lock();
    let Some(base) = base else { return Err(DriverError::NotReady); };
    
    let offset = gpio as usize * 0x10000;
    let mut cfg: u32 = 0;
    
    cfg |= func & 0x7;           // func[2:0]
    cfg |= (if output { 1 } else { 0 }) << 3;  // dir[3]
    cfg |= (pull & 0x3) << 4;    // pull[5:4]
    cfg |= (drive & 0x3) << 6;   // drive[7:6]
    cfg |= 1 << 8;               // oe enable
    
    unsafe {
        core::ptr::write_volatile((base + offset) as *mut u32, cfg);
    }
    
    Ok(())
}

/// Установить значение пина (output)
pub fn tlmm_gpio_write(gpio: u32, high: bool) -> Result<(), DriverError> {
    let base = *TLMM_BASE.lock();
    let Some(base) = base else { return Err(DriverError::NotReady); };
    
    let offset = gpio as usize * 0x10000 + 0x0008; // GPIO_OUT
    unsafe {
        core::ptr::write_volatile((base + offset) as *mut u32, if high { 1 } else { 0 });
    }
    Ok(())
}

/// Прочитать значение пина (input)
pub fn tlmm_gpio_read(gpio: u32) -> Result<bool, DriverError> {
    let base = *TLMM_BASE.lock();
    let Some(base) = base else { return Err(DriverError::NotReady); };
    
    let offset = gpio as usize * 0x10000 + 0x0004; // GPIO_IN
    let val = unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
    Ok(val & 1 != 0)
}

/// Инициализация TLMM (базовый адрес из DTB pinctrl@400000)
pub fn tlmm_init(base: usize) {
    *TLMM_BASE.lock() = Some(base);
    kprintln!("tlmm: initialized at {:#x}", base);
}

/// Настройка UART пинов для консоли (qcom,msm-geni-console @ 0x4c8c000)
/// На SM6375/Blair консольный UART обычно на GPIO 130 (TX) и 131 (RX)
/// Функция: 4 = UART (GENI SE)
pub fn tlmm_setup_console_uart() -> Result<(), DriverError> {
    // GPIO для консольного UART (подстроить под board!)
    // Обычно: TX=130 (func4), RX=131 (func4)
    tlmm_config_gpio(130, 4, true, 0, 3)?;  // TX: func4, output, no pull, 8mA
    tlmm_config_gpio(131, 4, false, 1, 0)?; // RX: func4, input, pull up
    kprintln!("tlmm: console UART pins configured (TX=130, RX=131)");
    Ok(())
}