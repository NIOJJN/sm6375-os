//! UFS (Universal Flash Storage) Host Controller для Qualcomm SM6375 (Blair)
//!
//! Базовый адрес: берётся из DT (обычно 0x4804000)
//! PHY адрес: 0x4807000
//!
//! Источник: Linux drivers/scsi/ufs/ufs-qcom.c + JEDEC UFS Specification v3.1

use crate::platform::{self, Driver, DriverError, PlatformDevice, Resource};
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

/// UFS Host Controller базовый адрес (будет взят из DT)
static UFS_BASE: SpinLock<Option<usize>> = SpinLock::new(None);
static UFS_PHY_BASE: SpinLock<Option<usize>> = SpinLock::new(None);
static UFS_INIT: AtomicBool = AtomicBool::new(false);

// UFS HC регистры (offset от UFS_BASE)
mod reg {
    // Host Controller Standard Registers (UFSHCI v3.0)
    pub const HCE: usize = 0x34;           // Host Controller Enable
    pub const UTRIACR: usize = 0x48;       // UTP Transfer Request Interrupt Aggregation Control
    pub const UTRLBA: usize = 0x50;        // UTP Transfer Request List Base Address L
    pub const UTRLBAU: usize = 0x54;       // UTP Transfer Request List Base Address U
    pub const UTRLDBR: usize = 0x58;       // UTP Transfer Request List Doorbell
    pub const UTRLCLR: usize = 0x5C;       // UTP Transfer Request List Clear
    pub const UTRLRSR: usize = 0x60;       // UTP Transfer Request List Run Stop
    pub const UTMRLBA: usize = 0x70;       // UTP Task Management Request List Base Address L
    pub const UTMRLBAU: usize = 0x74;      // UTP Task Management Request List Base Address U
    pub const UTMRLDBR: usize = 0x78;      // UTP Task Management Request List Doorbell
    pub const UTMRLCLR: usize = 0x7C;      // UTP Task Management Request List Clear
    pub const UTMRLRSR: usize = 0x80;      // UTP Task Management Request List Run Stop
    pub const UICCMD: usize = 0x90;        // UIC Command
    pub const UCMDARG1: usize = 0x94;      // UIC Command Argument 1
    pub const UCMDARG2: usize = 0x98;      // UIC Command Argument 2
    pub const UCMDARG3: usize = 0x9C;      // UIC Command Argument 3
    pub const HCS: usize = 0x30;           // Host Controller Status
    pub const IS: usize = 0x20;            // Interrupt Status
    pub const IE: usize = 0x24;            // Interrupt Enable
    
    // Vendor-specific (Qualcomm)
    pub const CFG1: usize = 0x104;         // Configuration 1
    pub const HCLKDIV: usize = 0x108;      // Host Clock Divider
}

// UFS PHY регистры (offset от UFS_PHY_BASE)
mod phy_reg {
    pub const PHY_START: usize = 0x000;
    pub const SW_RESET: usize = 0x008;
    pub const POWER_DOWN_CONTROL: usize = 0x004;
    pub const PHY_MODE: usize = 0x010;
    pub const TX_LANE_ENABLE: usize = 0x044;
    pub const RX_LANE_ENABLE: usize = 0x084;
}

// UIC Commands
const UIC_CMD_DME_GET: u32 = 0x01;
const UIC_CMD_DME_SET: u32 = 0x02;
const UIC_CMD_DME_PEER_GET: u32 = 0x03;
const UIC_CMD_DME_PEER_SET: u32 = 0x04;
const UIC_CMD_DME_POWERON: u32 = 0x10;
const UIC_CMD_DME_POWEROFF: u32 = 0x11;
const UIC_CMD_DME_ENABLE: u32 = 0x12;
const UIC_CMD_DME_RESET: u32 = 0x14;
const UIC_CMD_DME_LINKSTARTUP: u32 = 0x16;

/// Чтение 32-битного регистра UFS HC
fn ufs_read(offset: usize) -> u32 {
    if let Some(base) = *UFS_BASE.lock() {
        unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
    } else {
        0
    }
}

/// Запись 32-битного регистра UFS HC
fn ufs_write(offset: usize, val: u32) {
    if let Some(base) = *UFS_BASE.lock() {
        unsafe { core::ptr::write_volatile((base + offset) as *mut u32, val) }
    }
}

/// Чтение 32-битного регистра UFS PHY
fn phy_read(offset: usize) -> u32 {
    if let Some(base) = *UFS_PHY_BASE.lock() {
        unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
    } else {
        0
    }
}

/// Запись 32-битного регистра UFS PHY
fn phy_write(offset: usize, val: u32) {
    if let Some(base) = *UFS_PHY_BASE.lock() {
        unsafe { core::ptr::write_volatile((base + offset) as *mut u32, val) }
    }
}

/// Задержка в микросекундах (примерная)
fn udelay(us: u32) {
    for _ in 0..us * 100 {
        core::hint::spin_loop();
    }
}

/// Инициализация UFS PHY (QMP v4)
fn ufs_phy_init() -> Result<(), DriverError> {
    // 1. Сброс PHY
    phy_write(phy_reg::SW_RESET, 1);
    udelay(10);
    phy_write(phy_reg::SW_RESET, 0);
    udelay(10);
    
    // 2. Power up PHY
    phy_write(phy_reg::POWER_DOWN_CONTROL, 0x0);
    udelay(10);
    
    // 3. Enable TX/RX lanes (обычно lane 0 и 1 для UFS)
    phy_write(phy_reg::TX_LANE_ENABLE, 0x3); // lane 0,1
    phy_write(phy_reg::RX_LANE_ENABLE, 0x3);
    
    // 4. Set PHY mode to UFS HS-G3 (0x03 = HS-G3, 0x02 = HS-G2, 0x01 = PWM)
    phy_write(phy_reg::PHY_MODE, 0x02); // HS-G2 для начала
    
    // 5. Start PHY
    phy_write(phy_reg::PHY_START, 0x1);
    udelay(100);
    
    kprintln!("ufs: PHY init done");
    Ok(())
}

/// Отправка UIC команды
fn ufs_uic_cmd(cmd: u32, arg1: u32, arg2: u32, arg3: u32) -> Result<(), DriverError> {
    // Ждём пока предыдущая команда завершится
    for _ in 0..1000 {
        let hcs = ufs_read(reg::HCS);
        if (hcs & (1 << 3)) == 0 { // UCRDY bit
            break;
        }
        udelay(10);
    }
    
    // Пишем аргументы
    ufs_write(reg::UCMDARG1, arg1);
    ufs_write(reg::UCMDARG2, arg2);
    ufs_write(reg::UCMDARG3, arg3);
    
    // Отправляем команду
    ufs_write(reg::UICCMD, cmd);
    
    // Ждём завершения (UCCS bit в IS)
    for _ in 0..1000 {
        let is = ufs_read(reg::IS);
        if (is & (1 << 10)) != 0 { // UCCS bit
            ufs_write(reg::IS, 1 << 10); // clear
            return Ok(());
        }
        udelay(10);
    }
    
    Err(DriverError::Timeout)
}

/// Инициализация UFS Host Controller
fn ufs_hc_init() -> Result<(), DriverError> {
    // 1. Disable Host Controller
    ufs_write(reg::HCE, 0);
    udelay(10);
    
    // 2. Enable Host Controller
    ufs_write(reg::HCE, 1);
    udelay(100);
    
    // Ждём готовности HCE
    for _ in 0..1000 {
        let hce = ufs_read(reg::HCE);
        if (hce & 1) != 0 {
            break;
        }
        udelay(10);
    }
    
    kprintln!("ufs: HC enabled");
    
    // 3. Configure interrupt aggregation (disable для простоты)
    ufs_write(reg::UTRIACR, 0);
    
    // 4. Enable interrupts
    ufs_write(reg::IE, 0xFFFF_FFFF); // enable all
    
    // 5. DME_RESET
    ufs_uic_cmd(UIC_CMD_DME_RESET, 0, 0, 0)?;
    udelay(100);
    
    // 6. DME_ENABLE
    ufs_uic_cmd(UIC_CMD_DME_ENABLE, 0, 0, 0)?;
    udelay(100);
    
    // 7. DME_LINKSTARTUP
    kprintln!("ufs: starting link...");
    ufs_uic_cmd(UIC_CMD_DME_LINKSTARTUP, 0, 0, 0)?;
    
    // Ждём ULSS (UIC Link Startup Status) в IS
    for _ in 0..5000 {
        let is = ufs_read(reg::IS);
        if (is & (1 << 8)) != 0 { // ULSS bit
            ufs_write(reg::IS, 1 << 8); // clear
            kprintln!("ufs: link startup complete");
            return Ok(());
        }
        udelay(100);
    }
    
    Err(DriverError::Timeout)
}

/// Probe UFS драйвера
fn ufs_probe(dev: &PlatformDevice) -> Result<(), DriverError> {
    if UFS_INIT.swap(true, Ordering::AcqRel) {
        return Err(DriverError::AlreadyExists);
    }
    
    // Получаем адреса из DT
    let (hc_base, hc_size) = dev.get_resource_mem(0).ok_or(DriverError::NoMemory)?;
    *UFS_BASE.lock() = Some(hc_base);
    
    kprintln!("ufs: HC mapped at {:#x}, size={:#x}", hc_base, hc_size);
    
    // PHY адрес обычно идёт вторым resource или отдельным узлом
    // Для SM6375: PHY @ 0x4807000
    // TODO: получить из отдельного узла "ufsphy_mem"
    *UFS_PHY_BASE.lock() = Some(0x4807000);
    kprintln!("ufs: PHY at {:#x}", 0x4807000);
    
    // Включаем clocks через GCC
    if let Some(clk_id) = dev.get_resource_clock("core") {
        crate::drivers::gcc_sm6375::clk_enable_by_id(clk_id as u32)?;
    }
    if let Some(clk_id) = dev.get_resource_clock("bus") {
        crate::drivers::gcc_sm6375::clk_enable_by_id(clk_id as u32)?;
    }
    if let Some(clk_id) = dev.get_resource_clock("phy") {
        crate::drivers::gcc_sm6375::clk_enable_by_id(clk_id as u32)?;
    }
    
    // Инициализация PHY
    crate::drivers::vibro::indicate(crate::drivers::vibro::BootStage::UfsInit);
    ufs_phy_init()?;
    
    // Запрашиваем SMMU bypass для нашего SID (0x60 = 96)
    // Это должно произойти ДО HC init, иначе DMA упадёт
    if let Some(sid) = dev.get_resource_sid() {
        crate::drivers::smmu::smmu_request_bypass(sid);
    }
    
    // Инициализация Host Controller
    ufs_hc_init()?;
    
    kprintln!("ufs: probe ok");
    
    // Инициализируем GPT parser для поиска ESP
    let _ = crate::drivers::gpt::gpt_init();
    
    Ok(())
}

/// UFS драйвер
static UFS_DRIVER: Driver = Driver {
    name: "ufs-qcom",
    compatible: ["qcom,ufshc", "qcom,ufs-qcom", "", "", "", "", "", ""],
    compatible_n: 2,
    probe: ufs_probe,
    remove: None,
};

#[used]
#[link_section = ".init_array"]
static UFS_DRIVER_REG: fn() = || {
    let _ = platform::driver_register(&UFS_DRIVER);
};

// ========== Block Device API ==========

/// UTP Transfer Request Descriptor (UTRD)
#[repr(C, align(1024))]
struct Utrd {
    // DW0-3: Command Descriptor Base Address
    ucdba: u64,         // UCD base address (lower 7 bits = 0)
    ucdba_upper: u32,   // upper 32 bits (reserved in 32-bit)
    _rsv0: u32,
    // DW4-5: Response UPIU offset/length
    response_upiu_offset: u16,
    response_upiu_length: u16,
    prdt_offset: u16,
    prdt_length: u16,
    // DW6-7: Status
    overall_cmd_status: u8,
    _rsv1: [u8; 7],
}

/// UTP Command Descriptor (UCD)
#[repr(C, align(128))]
struct Ucd {
    // Command UPIU
    cmd_upiu: [u8; 32],
    // Response UPIU
    response_upiu: [u8; 32],
    // PRDT (Physical Region Description Table)
    prdt: [Prdt; 1],
}

/// Physical Region Description Table Entry
#[repr(C)]
struct Prdt {
    base_addr: u64,
    _rsv: u32,
    data_byte_count: u32, // bits [0:17] = byte count - 1
}

/// SCSI READ(10) Command Descriptor Block
#[repr(C)]
struct ScsiRead10Cdb {
    opcode: u8,          // 0x28
    flags: u8,
    lba: [u8; 4],        // big-endian LBA
    group: u8,
    transfer_len: [u8; 2], // big-endian sector count
    control: u8,
}

/// Direction для SCSI команды
#[derive(PartialEq)]
enum ScsiDir {
    None,
    In,
    Out,
}

/// Общий SCSI команда через UFS
/// cdb: Command Descriptor Block (до 16 байт)
/// dir: направление данных
/// buf: буфер для данных (In/Out)
/// expected_len: ожидаемая длина данных
fn ufs_scsi_cmd_raw(cdb: &[u8], dir: ScsiDir, buf: &mut [u8], expected_len: usize) -> Result<usize, DriverError> {
    if !UFS_INIT.load(Ordering::Acquire) {
        return Err(DriverError::NotReady);
    }
    if cdb.len() > 16 {
        return Err(DriverError::InvalidArg);
    }
    if (dir == ScsiDir::In || dir == ScsiDir::Out) && buf.len() < expected_len {
        return Err(DriverError::InvalidArg);
    }

    let mut utrd = UFS_UTRD.lock();
    let mut ucd = UFS_UCD.lock();
    let mut read_buf = UFS_READ_BUF.lock();

    // UTRD: указываем на UCD
    let ucd_addr = &*ucd as *const Ucd as u64;
    utrd.ucdba = ucd_addr;
    utrd.ucdba_upper = 0;
    utrd.response_upiu_offset = 32;
    utrd.response_upiu_length = 32;
    utrd.prdt_offset = 64;
    utrd.prdt_length = if dir == ScsiDir::None { 0 } else { 1 };
    utrd.overall_cmd_status = 0;

    // PRDT для data-in/out
    if dir != ScsiDir::None {
        let buf_addr = read_buf.as_ptr() as u64;
        ucd.prdt[0].base_addr = buf_addr;
        ucd.prdt[0].data_byte_count = ((expected_len as u32) - 1) & 0x3FFFF;
    }

    // Command UPIU
    ucd.cmd_upiu[0] = 0x01; // Transaction Type = Command
    ucd.cmd_upiu[1] = 0x00; // Flags
    ucd.cmd_upiu[2] = 0x00; // LUN
    ucd.cmd_upiu[3] = 0x00; // Task Tag
    ucd.cmd_upiu[4] = 0x00; // Command Set Type = SCSI
    ucd.cmd_upiu[5] = 0x00; // Reserved
    ucd.cmd_upiu[6] = 0x00; // Reserved
    ucd.cmd_upiu[7] = 0x00; // Reserved
    ucd.cmd_upiu[8] = 0x00; // EHS Length
    ucd.cmd_upiu[9] = 0x00; // Reserved
    // Data Segment Length (big-endian, 2 байта)
    let data_len = expected_len as u16;
    ucd.cmd_upiu[10] = (data_len >> 8) as u8;
    ucd.cmd_upiu[11] = data_len as u8;
    // CDB (bytes 12..)
    let cdb_len = cdb.len().min(16);
    ucd.cmd_upiu[12..12+cdb_len].copy_from_slice(&cdb[..cdb_len]);

    // Настраиваем Transfer Request List
    let utrd_addr = &*utrd as *const Utrd as u64;
    ufs_write(reg::UTRLBA, (utrd_addr & 0xFFFFFFFF) as u32);
    ufs_write(reg::UTRLBAU, (utrd_addr >> 32) as u32);

    // Start Transfer Request List
    ufs_write(reg::UTRLRSR, 1);

    // Ring doorbell для slot 0
    ufs_write(reg::UTRLDBR, 1 << 0);

    // Ждём завершения (UTRCS bit в IS)
    for _ in 0..10000 {
        let is = ufs_read(reg::IS);
        if (is & (1 << 0)) != 0 { // UTRCS bit
            ufs_write(reg::IS, 1 << 0); // clear
            
            // Проверяем статус
            if utrd.overall_cmd_status == 0 {
                // Success: копируем данные для In
                if dir == ScsiDir::In {
                    buf[..expected_len].copy_from_slice(&read_buf[..expected_len]);
                }
                return Ok(expected_len);
            } else {
                return Err(DriverError::IoError);
            }
        }
        udelay(10);
    }

    Err(DriverError::Timeout)
}

/// Обёртка для простых команд без данных (TEST UNIT READY, etc.)
fn ufs_scsi_simple(opcode: u8) -> Result<(), DriverError> {
    ufs_scsi_cmd_raw(&[opcode], ScsiDir::None, &mut [], 0).map(|_| ())
}

/// Buffer для чтения секторов (4KB aligned)
static UFS_READ_BUF: SpinLock<[u8; 8192]> = SpinLock::new([0u8; 8192]);
static UFS_UTRD: SpinLock<Utrd> = SpinLock::new(Utrd {
    ucdba: 0,
    ucdba_upper: 0,
    _rsv0: 0,
    response_upiu_offset: 32,
    response_upiu_length: 32,
    prdt_offset: 64,
    prdt_length: 1,
    overall_cmd_status: 0,
    _rsv1: [0; 7],
});
static UFS_UCD: SpinLock<Ucd> = SpinLock::new(Ucd {
    cmd_upiu: [0; 32],
    response_upiu: [0; 32],
    prdt: [Prdt {
        base_addr: 0,
        _rsv: 0,
        data_byte_count: 0,
    }],
});

/// Чтение секторов с UFS (LBA, sector_count)
pub fn ufs_read_sectors(lba: u64, count: u16, buf: &mut [u8]) -> Result<usize, DriverError> {
    if !UFS_INIT.load(Ordering::Acquire) {
        return Err(DriverError::NotReady);
    }
    
    if buf.len() < (count as usize * 512) {
        return Err(DriverError::InvalidArg);
    }
    
    // Настраиваем UTRD
    let mut utrd = UFS_UTRD.lock();
    let mut ucd = UFS_UCD.lock();
    let mut read_buf = UFS_READ_BUF.lock();
    
    // UTRD: указываем на UCD
    let ucd_addr = &*ucd as *const Ucd as u64;
    utrd.ucdba = ucd_addr;
    utrd.ucdba_upper = 0;
    utrd.response_upiu_offset = 32;
    utrd.response_upiu_length = 32;
    utrd.prdt_offset = 64;
    utrd.prdt_length = 1;
    utrd.overall_cmd_status = 0;
    
    // PRDT: физический адрес буфера
    let buf_addr = read_buf.as_ptr() as u64;
    ucd.prdt[0].base_addr = buf_addr;
    ucd.prdt[0].data_byte_count = ((count as u32 * 512) - 1) & 0x3FFFF;
    
    // Строим SCSI READ(10) CDB
    let cdb = ScsiRead10Cdb {
        opcode: 0x28,
        flags: 0,
        lba: [
            (lba >> 24) as u8,
            (lba >> 16) as u8,
            (lba >> 8) as u8,
            lba as u8,
        ],
        group: 0,
        transfer_len: [(count >> 8) as u8, count as u8],
        control: 0,
    };
    
    // Command UPIU: заголовок + CDB
    ucd.cmd_upiu[0] = 0x01; // Transaction Type = Command
    ucd.cmd_upiu[1] = 0x00; // Flags
    ucd.cmd_upiu[2] = 0x00; // LUN
    ucd.cmd_upiu[3] = 0x00; // Task Tag
    ucd.cmd_upiu[4] = 0x00; // Command Set Type = SCSI
    ucd.cmd_upiu[5] = 0x00; // Reserved
    ucd.cmd_upiu[6] = 0x00; // Reserved
    ucd.cmd_upiu[7] = 0x00; // Reserved
    ucd.cmd_upiu[8] = 0x00; // EHS Length
    ucd.cmd_upiu[9] = 0x00; // Reserved
    ucd.cmd_upiu[10] = 0x00; // Data Segment Length (MSB)
    ucd.cmd_upiu[11] = 0x00; // Data Segment Length (LSB)
    ucd.cmd_upiu[12..22].copy_from_slice(unsafe {
        core::slice::from_raw_parts(&cdb as *const ScsiRead10Cdb as *const u8, 10)
    });
    
    // Настраиваем Transfer Request List
    let utrd_addr = &*utrd as *const Utrd as u64;
    ufs_write(reg::UTRLBA, (utrd_addr & 0xFFFFFFFF) as u32);
    ufs_write(reg::UTRLBAU, (utrd_addr >> 32) as u32);
    
    // Start Transfer Request List
    ufs_write(reg::UTRLRSR, 1);
    
    // Ring doorbell для slot 0
    ufs_write(reg::UTRLDBR, 1 << 0);
    
    // Ждём завершения (UTRCS bit в IS)
    for _ in 0..10000 {
        let is = ufs_read(reg::IS);
        if (is & (1 << 0)) != 0 { // UTRCS bit
            ufs_write(reg::IS, 1 << 0); // clear
            
            // Проверяем статус
            if utrd.overall_cmd_status == 0 {
                // Success: копируем данные
                buf[0..(count as usize * 512)].copy_from_slice(&read_buf[0..(count as usize * 512)]);
                return Ok(count as usize * 512);
            } else {
                return Err(DriverError::IoError);
            }
        }
        udelay(10);
    }
    
    Err(DriverError::Timeout)
}

/// Получить размер UFS устройства (в секторах)
pub fn ufs_get_capacity() -> Result<u64, DriverError> {
    if !UFS_INIT.load(Ordering::Acquire) {
        return Err(DriverError::NotReady);
    }
    
    // SCSI READ CAPACITY(10) - opcode 0x25
    // CDB: [0x25, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    // PMI=0 (byte 1 bit 0) = return last LBA
    // LBA=0 (bytes 2-5)
    let mut resp = [0u8; 8];
    ufs_scsi_cmd_raw(&[0x25, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], 
                     ScsiDir::In, &mut resp, 8)?;
    
    let last_lba = u32::from_be_bytes([resp[0], resp[1], resp[2], resp[3]]);
    let block_size = u32::from_be_bytes([resp[4], resp[5], resp[6], resp[7]]);
    
    if block_size != 512 {
        kprintln!("ufs: unexpected block size {}", block_size);
    }
    
    // Capacity in sectors = last LBA + 1
    Ok((last_lba as u64) + 1)
}
