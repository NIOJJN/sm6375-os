//! GENI SE UART (Qualcomm Generic Interface Serial Engine) — ранний консольный вывод
//! для SM6375 (Blair). Устаревший протокол UARTDM (uart.rs / msm_uart.rs) на этом SoC
//! не используется: все UART — GENI SE внутри QUPv3.
//!
//! Консольный узел DTB Poco X4 Pro:
//!   /soc/qcom,qup_uart@4c8c000 compatible="qcom,msm-geni-console" status="ok"
//!
//! Предполагается, что XBL/ABL уже настроил SE в UART-режиме (firmware загружен,
//! тактовые частоты включены) и использовал его для своих логов. Мы только
//! выставляем FIFO-режим и поллим TX.
//!
//! Источники: Linux drivers/tty/serial/qcom_geni_serial.c,
//! include/linux/soc/qcom/geni-se.h

use crate::sync::SpinLock;

// ---- Смещения регистров Serial Engine ----
mod reg {
    /// GENI_STATUS: бит 0 = M_GENI_CMD_ACTIVE (выполняется команда)
    pub const GENI_STATUS: usize = 0x040;
    /// Режим DMA: бит 0 = GENI_DMA_MODE_EN (нам нужен FIFO-режим)
    pub const SE_GENI_DMA_MODE_EN: usize = 0x258;
    /// Команда мастера: opcode [30:27], параметр (длина, байты) [15:0]
    pub const SE_GENI_M_CMD0: usize = 0x600;
    /// Статус прерываний мастера
    pub const SE_GENI_M_IRQ_STATUS: usize = 0x610;
    /// Очистка прерываний мастера
    pub const SE_GENI_M_IRQ_CLEAR: usize = 0x618;
    /// Статус прерываний подчинённого
    pub const SE_GENI_S_IRQ_CLEAR: usize = 0x648;
    /// TX FIFO (слово = 4 байта, упаковка little-endian)
    pub const SE_GENI_TX_FIFOn: usize = 0x700;
    /// Watermark TX
    pub const SE_GENI_TX_WATERMARK_REG: usize = 0x80C;
}

// UART TX opcode GENI M_CMD
const UART_TX_OP: u32 = 1;

// Таймауты (число итераций спин-ожидания)
const SPIN_TIMEOUT: u32 = 2_000_000;

static GENI_BASE: SpinLock<Option<usize>> = SpinLock::new(None);

#[inline]
fn rd(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}

#[inline]
fn wr(base: usize, off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, v) }
}

/// Инициализация ранней консоли на базе из DTB.
/// Безопасно вызывать до включения MMU (работаем с физическими адресами),
/// но после MMU база должна быть в identity-map (main.rs добавляет её в mmio-список).
pub fn early_init(base: usize) {
    // FIFO-режим, не DMA (ABL мог оставить включённым)
    wr(base, reg::SE_GENI_DMA_MODE_EN, 0);
    // TX watermark: не критично для поллинга
    wr(base, reg::SE_GENI_TX_WATERMARK_REG, 0);
    // Сбросить висящие прерывания от ABL
    wr(base, reg::SE_GENI_M_IRQ_CLEAR, 0xFFFF_FFFF);
    wr(base, reg::SE_GENI_S_IRQ_CLEAR, 0xFFFF_FFFF);

    *GENI_BASE.lock() = Some(base);
}

/// Ждёт завершения активной GENI-команды.
fn wait_idle(base: usize) -> bool {
    for _ in 0..SPIN_TIMEOUT {
        if rd(base, reg::GENI_STATUS) & 1 == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Вывод одного байта в GENI UART (блокирующий, с таймаутом).
pub fn putc(c: u8) {
    let base = *GENI_BASE.lock();
    let Some(base) = base else { return };

    // Ждём завершения предыдущей команды TX
    if !wait_idle(base) {
        return;
    }

    // Байт в FIFO: слово 4 байта, наш байт — младший (pack_bytes little-endian)
    wr(base, reg::SE_GENI_TX_FIFOn, c as u32);

    // M_CMD: UART_TX, параметр = 1 байт
    wr(base, reg::SE_GENI_M_CMD0, (UART_TX_OP << 27) | 1);

    // Ждём завершения, затем чистим статус прерываний
    let _ = wait_idle(base);
    wr(base, reg::SE_GENI_M_IRQ_CLEAR, 0xFFFF_FFFF);
}

/// Вывод строки (\n автоматически разворачивается в \r\n, как принято в терминалах).
pub fn write_str(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            putc(b'\r');
        }
        putc(b);
    }
}

/// Базовый адрес активной консоли (для отладки).
pub fn base() -> Option<usize> {
    *GENI_BASE.lock()
}

/// Приём пока не реализован (ранняя консоль только на вывод).
pub fn getc() -> Option<u8> {
    None
}
