//! Простой cpio newc архиватор (no_std не требуется, запускается на хосте).
//! Собирает initramfs из бинарников в user/target/.../release/ (без сжатия).

use std::fs;

const TARGET: &str = "aarch64-unknown-none-softfloat";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = std::env::current_dir()?.to_string_lossy().to_string();
    let user_bin = format!("{}/user/target/{}/release/user", manifest_dir, TARGET);
    let out_dir = format!("{}/initramfs_tmp", manifest_dir);
    let out_cpio = format!("{}/initramfs.cpio", manifest_dir);

    println!("manifest_dir: {}", manifest_dir);
    println!("user_bin: {}", user_bin);
    println!("out_dir: {}", out_dir);

    // Удаляем старые файлы
    let _ = fs::remove_dir_all(&out_dir);
    let _ = fs::remove_file(&out_cpio);
    fs::create_dir_all(format!("{}/bin", out_dir))?;

    // Копируем бинарник под разными именами: init — PID 1, остальные роли
    // выбираются через arg0 при spawn.
    let names = ["init", "selftest", "shell", "leaker", "monitor"];
    for name in &names {
        let dst = format!("{}/bin/{}", out_dir, name);
        println!("copying {} -> {}", user_bin, dst);
        fs::copy(&user_bin, &dst)?;
    }
    
    // Check files exist
    for name in &names {
        let dst = format!("{}/bin/{}", out_dir, name);
        let meta = fs::metadata(&dst)?;
        println!("  {} size: {}", dst, meta.len());
    }

    // Создаём cpio newc архив
    let mut archive = Vec::new();
    add_dir_entries(&mut archive, &out_dir, "")?;
    add_cpio_entry(&mut archive, "TRAILER!!!", &[])?;

    use std::io::Write;
    fs::write(&out_cpio, &archive)?;
    eprintln!("AFTER WRITE - verifying file...");

    // Verify the written file
    let written = fs::read(&out_cpio)?;
    eprintln!("VERIFY: Written file size: {} bytes", written.len());
    // Check first entry header
    if written.len() >= 110 {
        let filesize_bytes = &written[54..62];
        let namesize_bytes = &written[94..102];
        eprintln!("VERIFY: Written filesize field: {:?} = {:?}", filesize_bytes, std::str::from_utf8(filesize_bytes));
        eprintln!("VERIFY: Written namesize field: {:?} = {:?}", namesize_bytes, std::str::from_utf8(namesize_bytes));
    } else {
        eprintln!("VERIFY: File too small: {} bytes", written.len());
    }

    // Очистка
    fs::remove_dir_all(&out_dir)?;

    println!("Created {} ({} bytes)", out_cpio, archive.len());
    Ok(())
}

fn add_dir_entries(archive: &mut Vec<u8>, base: &str, prefix: &str) -> Result<(), std::io::Error> {
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let full_name = if prefix.is_empty() { name.clone() } else { format!("{}/{}", prefix, name) };

        if path.is_dir() {
            add_dir_entries(archive, &path.to_string_lossy(), &full_name)?;
        } else {
            let data = fs::read(&path)?;
            add_cpio_entry(archive, &full_name, &data)?;
        }
    }
    Ok(())
}

/// Добавляет cpio newc запись в конец архива.
/// Каждая запись должна начинаться на 4-байтовой границе.
fn add_cpio_entry(archive: &mut Vec<u8>, name: &str, data: &[u8]) -> Result<(), std::io::Error> {
    let name_bytes = name.as_bytes();
    let namesize = name_bytes.len() + 1; // с нуль-терминатором
    let filesize = data.len();

    let header_size = 110;
    let name_padded = (namesize + 3) & !3;
    let data_start = header_size + name_padded;
    let data_padded = (filesize + 3) & !3;
    let entry_size = data_start + data_padded;

    // Выравниваем начало записи на 4 байта
    let offset = (archive.len() + 3) & !3;
    archive.resize(offset + entry_size, 0);

    // Записываем заголовок по смещению offset
    write_hex_at(archive, offset + 0, 6, 0x070701);
    write_hex_at(archive, offset + 6, 8, 0);
    write_hex_at(archive, offset + 14, 8, 0o100644);
    write_hex_at(archive, offset + 22, 8, 0);
    write_hex_at(archive, offset + 30, 8, 0);
    write_hex_at(archive, offset + 38, 8, 1);
    write_hex_at(archive, offset + 46, 8, 0);
    write_hex_at(archive, offset + 54, 8, filesize);
    write_hex_at(archive, offset + 62, 8, 0);
    write_hex_at(archive, offset + 70, 8, 0);
    write_hex_at(archive, offset + 78, 8, 0);
    write_hex_at(archive, offset + 86, 8, 0);
    write_hex_at(archive, offset + 94, 8, namesize);
    write_hex_at(archive, offset + 102, 8, 0);

    // Имя + нуль-терминатор
    archive[offset + header_size..offset + header_size + name_bytes.len()].copy_from_slice(name_bytes);
    archive[offset + header_size + name_bytes.len()] = 0;

    // Данные
    archive[offset + data_start..offset + data_start + filesize].copy_from_slice(data);

    Ok(())
}

/// Записывает hex-число в буфер по указанному смещению.
fn write_hex_at(buf: &mut [u8], offset: usize, width: usize, value: usize) {
    let hex = format!("{:0width$x}", value, width = width);
    let bytes = hex.as_bytes();
    buf[offset..offset + width].copy_from_slice(bytes);
}