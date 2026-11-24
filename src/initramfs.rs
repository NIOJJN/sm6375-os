//! Initramfs (cpio newc + gzip) parser.
//! Загружается прошивкой/QEMU по физическому адресу (DTB / chosen/linux,initrd-start/end).

use crate::pmm;
use core::{slice, str};

/// Заголовок cpio newc (ASCII hex).
#[repr(C, packed)]
struct CpioHeader {
    magic: [u8; 6],      // "070701"
    ino: [u8; 8],
    mode: [u8; 8],
    uid: [u8; 8],
    gid: [u8; 8],
    nlink: [u8; 8],
    mtime: [u8; 8],
    filesize: [u8; 8],
    devmajor: [u8; 8],
    devminor: [u8; 8],
    rdevmajor: [u8; 8],
    rdevminor: [u8; 8],
    namesize: [u8; 8],
    check: [u8; 8],
}

const CPIO_MAGIC: &[u8; 6] = b"070701";
const TRAILER_NAME: &str = "TRAILER!!!";

/// Парсит 8-байтное hex-число (big-endian ASCII).
fn parse_hex8(b: &[u8]) -> usize {
    let s = str::from_utf8(b).unwrap_or("");
    usize::from_str_radix(s, 16).unwrap_or(0)
}

/// Запись файла в initramfs.
#[derive(Clone, Copy)]
pub struct InitramfsFile {
    /// Имя файла (макс. 63 байта + нуль-терминатор).
    pub name: [u8; 64],
    /// Длина имени (без нуль-терминатора).
    pub name_len: usize,
    /// Указатель на данные (внутри GUNZIP_BUF или исходного буфера).
    pub data: &'static [u8],
}

static mut FILES: [Option<InitramfsFile>; 32] = [const { None }; 32];
static mut FILES_COUNT: usize = 0;

/// Инициализация: ищет initrd в DTB (chosen/linux,initrd-start/end), распаковывает gzip + cpio.
pub fn init(fdt: &crate::fdt::BootInfo) {
    let (start, end) = match (fdt.initrd_start, fdt.initrd_end) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => {
            crate::kprintln!("initramfs: no initrd in DTB");
            return;
        }
    };
    crate::kprintln!("initramfs: found at {:#x}..{:#x} ({} KiB)", start, end, (end - start) / 1024);

    // Копируем в память ядра (чтобы не зависеть от карты памяти загрузчика).
    let size = end - start;
    let dst = match pmm::alloc_contiguous(size) {
        Some(p) => p,
        None => {
            crate::kprintln!("initramfs: OOM allocating {} bytes", size);
            return;
        }
    };
    unsafe { core::ptr::copy_nonoverlapping(start as *const u8, dst as *mut u8, size) };

    // Распаковываем gzip, если нужно. Всегда используем статический буфер,
    // чтобы данные файлов жили там и ссылки на них были 'static.
    let data = unsafe { slice::from_raw_parts(dst as *const u8, size) };
    static mut GUNZIP_BUF: [u8; MAX_UNCOMPRESSED] = [0; MAX_UNCOMPRESSED];
    let uncompressed = if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        let out_len = match gunzip(data, unsafe { &mut GUNZIP_BUF }) {
            Some(len) => len,
            None => {
                crate::kprintln!("initramfs: gzip decompress failed");
                return;
            }
        };
        unsafe { &GUNZIP_BUF[..out_len] }
    } else {
        // Копируем несжатые данные в статический буфер.
        if size > MAX_UNCOMPRESSED {
            crate::kprintln!("initramfs: uncompressed data too large ({} > {})", size, MAX_UNCOMPRESSED);
            return;
        }
        unsafe {
            GUNZIP_BUF[..size].copy_from_slice(data);
            &GUNZIP_BUF[..size]
        }
    };

    // SAFETY: uncompressed указывает на статический GUNZIP_BUF.
    parse_cpio(unsafe { core::mem::transmute::<&[u8], &'static [u8]>(uncompressed) });
    crate::kprintln!("initramfs: loaded {} files", unsafe { FILES_COUNT });
}

/// Поиск файла по имени (без ведущего '/').
pub fn find(name: &str) -> Option<&'static [u8]> {
    let name_bytes = name.as_bytes();
    unsafe {
        for f in FILES.iter().take(FILES_COUNT) {
            if let Some(file) = f {
                if file.name_len == name_bytes.len() && file.name[..file.name_len] == *name_bytes {
                    return Some(file.data);
                }
            }
        }
    }
    None
}

/// Список всех файлов (для отладки).
pub fn list() {
    unsafe {
        for f in FILES.iter().take(FILES_COUNT) {
            if let Some(file) = f {
                let name_str = core::str::from_utf8(&file.name[..file.name_len]).unwrap_or("<bad>");
                crate::kprintln!("  /{} ({} bytes)", name_str, file.data.len());
            }
        }
    }
}

// --- gzip (RFC 1952) minimal decoder ---

/// Максимальный размер распакованного initramfs (64 MiB).
const MAX_UNCOMPRESSED: usize = 64 * 1024 * 1024;

/// Распаковывает gzip (только stored blocks) в предвыделенный буфер.
/// Возвращает длину распакованных данных или None при ошибке.
fn gunzip(data: &[u8], out_buf: &mut [u8]) -> Option<usize> {
    if data.len() < 10 {
        return None;
    }
    let mut i = 10;
    let flg = data[3];
    if flg & 0x04 != 0 {
        if i + 2 > data.len() { return None; }
        let xlen = u16::from_le_bytes([data[i], data[i+1]]) as usize;
        i += 2 + xlen;
    }
    if flg & 0x08 != 0 { while i < data.len() && data[i] != 0 { i += 1; } i += 1; }
    if flg & 0x10 != 0 { while i < data.len() && data[i] != 0 { i += 1; } i += 1; }
    if flg & 0x02 != 0 { i += 2; }

    let mut out_len = 0;
    while i < data.len() {
        if i + 1 >= data.len() { break; }
        let bfinal = data[i] & 1;
        let btype = (data[i] >> 1) & 3;
        i += 1;
        if btype == 0 {
            if i + 4 > data.len() { break; }
            let len = u16::from_le_bytes([data[i], data[i+1]]) as usize;
            let nlen = u16::from_le_bytes([data[i+2], data[i+3]]) as usize;
            i += 4;
            if len != (!nlen & 0xffff) { break; }
            if i + len > data.len() { break; }
            if out_len + len > out_buf.len() { return None; }
            out_buf[out_len..out_len+len].copy_from_slice(&data[i..i+len]);
            out_len += len;
            i += len;
            if bfinal != 0 { break; }
        } else {
            crate::kprintln!("initramfs: deflate block not supported, need stored blocks");
            return None;
        }
    }
    Some(out_len)
}

// --- cpio newc parser ---

/// data должен указывать на статический буфер (GUNZIP_BUF), поэтому безопасно
/// привести к 'static.
fn parse_cpio(data: &'static [u8]) {
    let mut offset = 0;
    while offset < data.len() {
        crate::kprintln!("initramfs: loop offset={}", offset);
        if offset + core::mem::size_of::<CpioHeader>() > data.len() {
            break;
        }
        let hdr = unsafe { &*(data[offset..].as_ptr() as *const CpioHeader) };
        if &hdr.magic != CPIO_MAGIC {
            crate::kprintln!("initramfs: bad magic at offset {}", offset);
            break;
        }
        let namesize = parse_hex8(&hdr.namesize);
        let filesize = parse_hex8(&hdr.filesize);
        let hdr_size = core::mem::size_of::<CpioHeader>();
        let name_start = offset + hdr_size;
        let name_end = name_start + namesize;
        if name_end > data.len() { break; }
        let name_bytes = &data[name_start..name_end - 1];
        let name = str::from_utf8(name_bytes).unwrap_or("");
        // Имя в cpio включает null-терминатор и выравнивается на 4 байта.
        // data_start = name_start + align_up(namesize, 4)
        let name_padded = (namesize + 3) & !3;
        let data_start = name_start + name_padded;
        let data_end = data_start + filesize;
        if data_end > data.len() { break; }
        let file_data = &data[data_start..data_end];

        if name == TRAILER_NAME {
            break;
        }
        if !name.is_empty() && filesize > 0 {
            // Копируем имя в фиксированный буфер.
            let mut name_buf = [0u8; 64];
            let name_bytes = name.as_bytes();
            let nlen = name_bytes.len().min(63);
            name_buf[..nlen].copy_from_slice(&name_bytes[..nlen]);
            unsafe {
                if FILES_COUNT < FILES.len() {
                    FILES[FILES_COUNT] = Some(InitramfsFile {
                        name: name_buf,
                        name_len: nlen,
                        data: file_data,
                    });
                    FILES_COUNT += 1;
                }
            }
        }
        // Следующий заголовок выровнен на 4 байта
        offset = (data_end + 3) & !3;
    }
}