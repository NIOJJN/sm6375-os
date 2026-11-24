//! Qualcomm APSS watchdog (compatible "qcom,msm-watchdog").
//!
//! На SM6375 (Blair): /soc/qcom,wdt@f410000, reg = 0xf410000 + 0x1000.
//! Если сторожевой таймер оставлен включённым загрузчиком, зависшее ядро
//! приведёт к перезагрузке через ~30 с (bark/bite прерывания GIC 32/33).
//! На QEMU virt такого узла нет — отключение не выполняется.
//!
//! Смещения регистров: drivers/watchdog/qcom-wdt.c (reg_offset_data_apcs_tmr).

const WDT_EN: usize = 0x08; // 1 = включён, 0 = выключен
const WDT_RST: usize = 0x04; // запись сбрасывает счётчик («погладить»)

/// Полностью отключает watchdog по базовому адресу из DTB.
pub fn disable(base: usize) {
    unsafe {
        core::ptr::write_volatile((base + WDT_EN) as *mut u32, 0);
    }
}

/// Погладить watchdog (если вдруг включён намеренно).
pub fn kick(base: usize) {
    unsafe {
        core::ptr::write_volatile((base + WDT_RST) as *mut u32, 1);
    }
}
