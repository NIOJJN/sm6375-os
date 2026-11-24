fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // Экранируем пробелы в пути для линкера.
    let escaped = dir.replace(' ', "\\ ");
    println!("cargo:rustc-link-arg=-T{}/linker.ld", escaped);
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/boot.S");
    println!("cargo:rerun-if-env-changed=LOAD_BASE");
    println!("cargo:rerun-if-changed=user/target/aarch64-unknown-none-softfloat/release/user");
    // Физический адрес загрузки. QEMU virt: 0x40080000 (по умолчанию).
    // Для телефона: LOAD_BASE=0x... cargo build --release (должен быть = 0x80000 mod 2MiB).
    if let Ok(b) = std::env::var("LOAD_BASE") {
        println!("cargo:rustc-link-arg=--defsym=__load_base={}", b);
    }
}
