fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // Экранируем пробелы в пути для линкера (путь проекта может содержать пробелы).
    let escaped = dir.replace(' ', "\\ ");
    println!("cargo:rustc-link-arg=-T{}/link.ld", escaped);
    println!("cargo:rerun-if-changed=link.ld");
}
