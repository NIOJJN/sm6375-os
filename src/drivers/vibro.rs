//! PMIC Vibration Motor Indicator — «Морзе» на вибромоторе для диагностики без UART
//!
//! На Xiaomi Poco X4 Pro (SM6375) вибромотор обычно подключен к PMIC GPIO.
//! Типичные GPIO: 13 (PMI8998) или специальный vibrator driver (DRV2605).
//! Если загрузчик не инициализировал vibrator driver, можно дергать GPIO напрямую.
//!
//! Паттерны (короткий=·, длинный=−, пауза=пробел):
//!   WDT_OFF      = ·       (1 short)
//!   GCC_PROBE    = ··      (2 short)
//!   SPMI_PROBE   = ···     (3 short)
//!   UFS_INIT     = ····    (4 short)
//!   INITRAMFS_OK = −       (1 long = success)
//!   ERROR        = ·−·−    (SOS)

use crate::drivers::pmic_sm6375::{pmic_gpio_config, pmic_gpio_write};
use core::sync::atomic::{AtomicU8, Ordering};

/// GPIO для вибромотора на PMI8998 (подстроить под board)
/// На Xiaomi часто: 13 = VIBR_EN, иногда 14/15
static VIBRO_GPIO: AtomicU8 = AtomicU8::new(13);
static VIBRO_INIT: AtomicU8 = AtomicU8::new(0);

/// Настроить GPIO вибромотора (вызовется после PMIC probe)
pub fn vibro_init(gpio: u8) {
    VIBRO_GPIO.store(gpio, Ordering::Relaxed);
    let _ = pmic_gpio_config(gpio, true, false, 4); // output, no pull, drive strength 4
    VIBRO_INIT.store(1, Ordering::Relaxed);
}

/// Короткий вибробурст (~50мс)
fn vibro_short() {
    let gpio = VIBRO_GPIO.load(Ordering::Relaxed);
    if VIBRO_INIT.load(Ordering::Relaxed) == 0 { return; }
    let _ = pmic_gpio_write(gpio, true);
    udelay(50_000);
    let _ = pmic_gpio_write(gpio, false);
    udelay(50_000);
}

/// Длинный вибробурст (~200мс)
fn vibro_long() {
    let gpio = VIBRO_GPIO.load(Ordering::Relaxed);
    if VIBRO_INIT.load(Ordering::Relaxed) == 0 { return; }
    let _ = pmic_gpio_write(gpio, true);
    udelay(200_000);
    let _ = pmic_gpio_write(gpio, false);
    udelay(100_000);
}

/// Пауза между символами
fn vibro_gap() {
    udelay(100_000);
}

/// Задержка в микросекундах
fn udelay(us: u32) {
    for _ in 0..us / 10 {
        core::hint::spin_loop();
    }
}

/// Этапы загрузки для индикации
#[derive(Clone, Copy)]
pub enum BootStage {
    WdtOff,       // WDT отключён
    GccProbe,     // GCC пробирован
    SpmiProbe,    // SPMI пробирован
    UfsInit,      // UFS инициализация началась
    InitramfsOk,  // initramfs загружен успешно
    Error,        // Ошибка (SOS)
}

impl BootStage {
    /// Выполнить паттерн для этапа
    pub fn indicate(self) {
        match self {
            BootStage::WdtOff => {
                vibro_short();
            }
            BootStage::GccProbe => {
                vibro_short(); vibro_gap(); vibro_short();
            }
            BootStage::SpmiProbe => {
                vibro_short(); vibro_gap(); vibro_short(); vibro_gap(); vibro_short();
            }
            BootStage::UfsInit => {
                for _ in 0..4 { vibro_short(); vibro_gap(); }
            }
            BootStage::InitramfsOk => {
                vibro_long(); // успех = длинный
            }
            BootStage::Error => {
                // SOS: ·−·−
                vibro_short(); vibro_gap();
                vibro_long(); vibro_gap();
                vibro_short(); vibro_gap();
                vibro_long();
            }
        }
    }
}

/// Быстрый вызов из кода загрузки
pub fn indicate(stage: BootStage) {
    stage.indicate();
}