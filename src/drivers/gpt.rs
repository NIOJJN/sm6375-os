//! GPT (GUID Partition Table) parser для поиска EFI System Partition
//!
//! Используется для нахождения initramfs на UFS без зависимости от
//! загрузчика (initrd-start/end в /chosen).
//!
//! Структура GPT:
//! - LBA 0: Protective MBR
//! - LBA 1: GPT Header
//! - LBA 2..33: Partition Entry Array (обычно 128 entries по 128 байт)
//!
//! EFI System Partition type GUID: C12A7328-F81F-11D2-BA4B-00A0C93EC93B

use crate::drivers::ufs::{ufs_read_sectors, ufs_get_capacity};
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

/// GPT Partition Type GUIDs
pub mod guid {
    /// EFI System Partition: C12A7328-F81F-11D2-BA4B-00A0C93EC93B
    pub const ESP: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1,  // time_low
        0x1F, 0xF8,              // time_mid
        0xD2, 0x11,              // time_hi_and_version
        0xBA, 0x4B,              // clock_seq_hi_res + clock_seq_low
        0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,  // node
    ];
    
    /// Linux Filesystem: 0FC63DAF-8483-4772-8E79-3D69D8477DE4
    pub const LINUX_FS: [u8; 16] = [
        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47,
        0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4,
    ];
    
    /// Microsoft Basic Data: EBD0A0A2-B9E5-4433-87C0-68B6B72699C7
    pub const MS_BASIC_DATA: [u8; 16] = [
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44,
        0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
    ];
}

/// GPT Header (LBA 1) - не packed, читаем побайтово
#[repr(C)]
struct GptHeader {
    signature: [u8; 8],        // "EFI PART"
    revision: u32,             // 0x00010000
    header_size: u32,          // 92+
    header_crc32: u32,
    reserved: u32,
    current_lba: u64,          // 1
    backup_lba: u64,           // last LBA
    first_usable_lba: u64,
    last_usable_lba: u64,
    disk_guid: [u8; 16],
    part_entry_start_lba: u64, // usually 2
    num_part_entries: u32,     // usually 128
    part_entry_size: u32,      // usually 128
    part_array_crc32: u32,
}

/// Partition Entry (128 bytes) - не packed
#[repr(C)]
struct PartEntry {
    type_guid: [u8; 16],
    unique_guid: [u8; 16],
    first_lba: u64,
    last_lba: u64,
    attributes: u64,
    name: [u16; 36],           // UTF-16LE, 72 bytes
}

/// Найденный раздел
#[derive(Clone, Copy)]
pub struct GptPartition {
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name: [u16; 36],
}

impl GptPartition {
    /// Является ли это EFI System Partition
    pub fn is_esp(&self) -> bool {
        self.type_guid == guid::ESP
    }
    
    /// Размер в секторах
    pub fn size_sectors(&self) -> u64 {
        self.last_lba.saturating_sub(self.first_lba) + 1
    }
    
    /// Размер в байтах
    pub fn size_bytes(&self) -> u64 {
        self.size_sectors() * 512
    }
}

/// Статический буфер для чтения GPT (1 сектор = 512 байт)
static GPT_BUF: SpinLock<[u8; 512]> = SpinLock::new([0u8; 512]);
static GPT_PARSED: AtomicBool = AtomicBool::new(false);
static GPT_ESP: SpinLock<Option<GptPartition>> = SpinLock::new(None);

/// Внутренняя функция чтения сектора в буфер GPT
fn gpt_read_sector(lba: u64) -> Result<(), &'static str> {
    let mut buf = GPT_BUF.lock();
    ufs_read_sectors(lba, 1, &mut *buf).map_err(|_| "UFS read failed")?;
    Ok(())
}

/// Получить указатель на буфер как байтовый слайс
fn gpt_buf() -> SpinLock<[u8; 512]> {
    // Not used, just for type reference
    SpinLock::new([0u8; 512])
}

/// CRC32 для проверки GPT (полином 0x04C11DB7, reflected)
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB88320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Сравнение GUID (16 байт)
fn guid_eq(a: &[u8], b: &[u8; 16]) -> bool {
    a.len() == 16 && a[..] == b[..]
}

/// Парсинг GPT и поиск ESP
pub fn gpt_find_esp() -> Result<GptPartition, &'static str> {
    if GPT_PARSED.load(Ordering::Acquire) {
        if let Some(esp) = *GPT_ESP.lock() {
            return Ok(esp);
        }
        return Err("ESP not found");
    }
    
    // Читаем LBA 1 (GPT Header)
    gpt_read_sector(1)?;
    let buf = GPT_BUF.lock();
    
    // Парсим GPT заголовок вручную из байтового буфера (избегаем unaligned access)
    // Signature: 8 bytes at offset 0
    if &buf[0..8] != b"EFI PART" {
        return Err("Invalid GPT signature");
    }
    // Revision: u32 at offset 8 (little-endian)
    let revision = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    // Header size: u32 at offset 12
    let header_size = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    // Header CRC32: u32 at offset 16
    let header_crc32 = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
    // Reserved: u32 at offset 20
    let _reserved = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
    // Current LBA: u64 at offset 24
    let current_lba = u64::from_le_bytes([
        buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31]
    ]);
    // Backup LBA: u64 at offset 32
    let _backup_lba = u64::from_le_bytes([
        buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39]
    ]);
    // First usable LBA: u64 at offset 40
    let _first_usable_lba = u64::from_le_bytes([
        buf[40], buf[41], buf[42], buf[43], buf[44], buf[45], buf[46], buf[47]
    ]);
    // Last usable LBA: u64 at offset 48
    let _last_usable_lba = u64::from_le_bytes([
        buf[48], buf[49], buf[50], buf[51], buf[52], buf[53], buf[54], buf[55]
    ]);
    // Disk GUID: 16 bytes at offset 56
    let _disk_guid = &buf[56..72];
    // Partition entry start LBA: u64 at offset 72
    let part_entry_start_lba = u64::from_le_bytes([
        buf[72], buf[73], buf[74], buf[75], buf[76], buf[77], buf[78], buf[79]
    ]);
    // Number of partition entries: u32 at offset 80
    let num_part_entries = u32::from_le_bytes([buf[80], buf[81], buf[82], buf[83]]);
    // Partition entry size: u32 at offset 84
    let part_entry_size = u32::from_le_bytes([buf[84], buf[85], buf[86], buf[87]]);
    // Partition array CRC32: u32 at offset 88
    let part_array_crc32 = u32::from_le_bytes([buf[88], buf[89], buf[90], buf[91]]);
    
    // Проверяем CRC32 заголовка (нулеем поле crc32 для расчёта)
    let mut hdr_bytes = [0u8; 92];
    hdr_bytes[..8].copy_from_slice(&buf[0..8]);
    hdr_bytes[8..12].copy_from_slice(&revision.to_le_bytes());
    hdr_bytes[12..16].copy_from_slice(&header_size.to_le_bytes());
    // crc32 field (4 байта) = 0
    hdr_bytes[16..20].fill(0);
    hdr_bytes[20..92].copy_from_slice(&buf[20..92]);
    
    let calc_crc = crc32(&hdr_bytes);
    if calc_crc != header_crc32 {
        kprintln!("gpt: header CRC mismatch (calc={:#x} stored={:#x})", calc_crc, header_crc32);
        // Не возвращаем ошибку — некоторые загрузчики не обновляют CRC
    }
    
    // Используем распарсенные значения
    let entry_lba = part_entry_start_lba;
    let num_entries = num_part_entries as usize;
    let entry_size = part_entry_size as usize;
    
    if entry_size != 128 {
        return Err("Unexpected partition entry size");
    }
    
    // Читаем массив записей разделов
    let entries_per_sector = 512 / entry_size; // 4 entries per sector
    let total_sectors = (num_entries + entries_per_sector - 1) / entries_per_sector;
    
    let mut found_esp = None;
    
    for sector_idx in 0..total_sectors {
        let lba = entry_lba + sector_idx as u64;
        gpt_read_sector(lba)?;
        let buf = GPT_BUF.lock();
        
        // Парсим entries в этом секторе
        for i in 0..entries_per_sector {
            let entry_idx = sector_idx * entries_per_sector + i;
            if entry_idx >= num_entries {
                break;
            }
            
            let offset = i * entry_size;
            
            // Парсим PartEntry вручную из байтов (128 байт)
            // type_guid: 16 bytes at offset 0
            let type_guid = &buf[offset..offset+16];
            // unique_guid: 16 bytes at offset 16
            let unique_guid = &buf[offset+16..offset+32];
            // first_lba: u64 at offset 32
            let first_lba = u64::from_le_bytes([
                buf[offset+32], buf[offset+33], buf[offset+34], buf[offset+35],
                buf[offset+36], buf[offset+37], buf[offset+38], buf[offset+39]
            ]);
            // last_lba: u64 at offset 40
            let last_lba = u64::from_le_bytes([
                buf[offset+40], buf[offset+41], buf[offset+42], buf[offset+43],
                buf[offset+44], buf[offset+45], buf[offset+46], buf[offset+47]
            ]);
            // attributes: u64 at offset 48
            let attributes = u64::from_le_bytes([
                buf[offset+48], buf[offset+49], buf[offset+50], buf[offset+51],
                buf[offset+52], buf[offset+53], buf[offset+54], buf[offset+55]
            ]);
            // name: 36 u16 = 72 bytes at offset 56
            let mut name = [0u16; 36];
            for j in 0..36 {
                name[j] = u16::from_le_bytes([buf[offset+56+j*2], buf[offset+57+j*2]]);
            }
            
            // Пустой type GUID = unused entry
            if type_guid.iter().all(|&b| b == 0) {
                continue;
            }
            
            if guid_eq(type_guid, &guid::ESP) {
                let mut tg = [0u8; 16];
                tg.copy_from_slice(type_guid);
                let mut ug = [0u8; 16];
                ug.copy_from_slice(unique_guid);
                let part = GptPartition {
                    type_guid: tg,
                    unique_guid: ug,
                    first_lba,
                    last_lba,
                    attributes,
                    name,
                };
                found_esp = Some(part);
                break;
            }
        }
        
        if found_esp.is_some() {
            break;
        }
    }
    
    GPT_PARSED.store(true, Ordering::Release);
    
    if let Some(esp) = found_esp {
        *GPT_ESP.lock() = Some(esp);
        Ok(esp)
    } else {
        Err("ESP partition not found")
    }
}

/// Получить LBA и размер ESP (если найден)
pub fn gpt_esp_info() -> Option<(u64, u64)> {
    gpt_find_esp().ok().map(|p| (p.first_lba, p.size_sectors()))
}

/// Прочитать raw данные из ESP (для загрузки initramfs)
pub fn gpt_read_esp(lba_offset: u64, count: u16, buf: &mut [u8]) -> Result<usize, &'static str> {
    gpt_find_esp()?;
    if let Some(esp) = *GPT_ESP.lock() {
        let lba = esp.first_lba + lba_offset;
        ufs_read_sectors(lba, count, buf).map_err(|_| "UFS read failed")
    } else {
        Err("ESP not found")
    }
}

/// Инициализация GPT (вызывается после UFS init)
pub fn gpt_init() -> Result<(), &'static str> {
    match gpt_find_esp() {
        Ok(esp) => {
            kprintln!("gpt: ESP found at LBA {}..{} ({} MiB)", 
                     esp.first_lba, esp.last_lba, esp.size_bytes() / (1024*1024));
            Ok(())
        }
        Err(e) => {
            kprintln!("gpt: ESP not found: {}", e);
            Err(e)
        }
    }
}