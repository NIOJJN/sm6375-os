//! Clock Tree Verification для Qualcomm SM6375 (Blair)
//!
//! Проверяет что все необходимые такты для UFS/SPMI/UART/GCC включены.
//! Читает регистры GCC (clock controller) и проверяет статус ENABLE битов.
//!
//! GCC base: 0x1400000 (blair-gccsyscon)
//! Регистры:
//!   CLK_ENA[n]: 0x0000 + n*4  - Enable регистры (по 32 такта на регистр)
//!   CLK_STS[n]: 0x0008 + n*4  - Status регистры

use crate::drivers::gcc_sm6375::{clk_enable, clk_enable_by_id};
use crate::platform::DriverError;
use crate::sync::SpinLock;

/// GCC базовый адрес (уже инициализирован в gcc_sm6375)
static GCC_BASE: SpinLock<Option<usize>> = SpinLock::new(None);

/// Clock IDs для SM6375 (Blair) - из DTB clock-names
/// Источник: Linux drivers/clk/qcom/gcc-sm6375.c
pub mod clk_id {
    // UFS clocks
    pub const UFS_CORE: u32 = 0x9E;      // core_clk
    pub const UFS_AGGR: u32 = 0x9A;      // bus_aggr_clk
    pub const UFS_IFACE: u32 = 0x9D;     // iface_clk
    pub const UFS_UNIPRO: u32 = 0xA6;    // core_clk_unipro
    pub const UFS_CORE_ICE: u32 = 0xA0;  // core_clk_ice
    pub const UFS_REF: u32 = 0x00;       // ref_clk
    pub const UFS_TX_L0: u32 = 0xA5;     // tx_lane0_sync_clk
    pub const UFS_RX_L0: u32 = 0xA4;     // rx_lane0_sync_clk
    
    // SPMI clocks
    pub const SPMI_BUS: u32 = 0x86;      // se_clk
    pub const SPMI_IFACE: u32 = 0x8E;    // ahbs
    pub const SPMI_AHB: u32 = 0x8F;      // ahb
    
    // UART clocks (GENI console)
    pub const UART_SE: u32 = 0x86;       // se_clk
    pub const UART_AHBS: u32 = 0x8E;     // ahbs
    pub const UART_AHB: u32 = 0x8F;      // ahb
    
    // GCC clocks
    pub const GCC_TCXO: u32 = 0x00;      // bi_tcxo
    pub const GCC_TCXO_AO: u32 = 0x01;   // bi_tcxo_ao
    pub const GCC_SLEEP: u32 = 0x61;     // sleep_clk
}

/// Установить базовый адрес GCC (вызывается из gcc_sm6375 probe)
pub fn clock_set_gcc_base(base: usize) {
    *GCC_BASE.lock() = Some(base);
}

/// Проверить, включен ли такт (читаем CLK_STS)
fn is_clk_enabled(clk_id: u32) -> bool {
    let base = *GCC_BASE.lock();
    let Some(base) = base else { return false; };
    
    let reg_idx = clk_id / 32;
    let bit = clk_id % 32;
    let sts_reg = base + 0x0008 + reg_idx as usize * 4;
    
    let sts = unsafe { core::ptr::read_volatile(sts_reg as *const u32) };
    (sts & (1 << bit)) != 0
}

/// Включить такт если выключен
pub fn ensure_clk_enabled(clk_id: u32) -> Result<(), DriverError> {
    if is_clk_enabled(clk_id) {
        return Ok(());
    }
    kprintln!("clock: enabling clk {:#x}", clk_id);
    clk_enable_by_id(clk_id)
}

/// Проверить и включить все критические такты
pub fn verify_clock_tree() -> Result<(), DriverError> {
    kprintln!("clock: verifying clock tree...");
    
    // UFS clocks
    for &id in &[
        clk_id::UFS_CORE, clk_id::UFS_AGGR, clk_id::UFS_IFACE,
        clk_id::UFS_UNIPRO, clk_id::UFS_CORE_ICE,
        clk_id::UFS_REF, clk_id::UFS_TX_L0, clk_id::UFS_RX_L0,
    ] {
        ensure_clk_enabled(id)?;
    }
    
    // SPMI clocks
    for &id in &[
        clk_id::SPMI_BUS, clk_id::SPMI_IFACE, clk_id::SPMI_AHB,
    ] {
        ensure_clk_enabled(id)?;
    }
    
    // UART clocks
    for &id in &[
        clk_id::UART_SE, clk_id::UART_AHBS, clk_id::UART_AHB,
    ] {
        ensure_clk_enabled(id)?;
    }
    
    // GCC base clocks
    ensure_clk_enabled(clk_id::GCC_TCXO)?;
    ensure_clk_enabled(clk_id::GCC_TCXO_AO)?;
    ensure_clk_enabled(clk_id::GCC_SLEEP)?;
    
    kprintln!("clock: tree verified, all critical clocks enabled");
    Ok(())
}

/// Отладочный вывод статуса всех тактов
pub fn dump_clock_status() {
    let base = *GCC_BASE.lock();
    let Some(base) = base else { return; };
    
    kprintln!("clock: GCC status dump:");
    for reg_idx in 0..16 {
        let sts = unsafe { core::ptr::read_volatile((base + 0x0008 + reg_idx * 4) as *const u32) };
        let ena = unsafe { core::ptr::read_volatile((base + 0x0000 + reg_idx * 4) as *const u32) };
        if sts != 0 || ena != 0 {
            kprintln!("  CLK_STS[{}]={:#010x} CLK_ENA[{}]={:#010x}", reg_idx, sts, reg_idx, ena);
        }
    }
}