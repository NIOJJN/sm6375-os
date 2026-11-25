# vv1raos

Автономное (standalone) bare-metal ядро для AArch64 на Rust. Не зависит от Android:
Ядро собирается стабильным Rust и запускается под QEMU (машина `virt`).

## Сборка

Требуется:
- Rust (стабильный канал, цель `aarch64-unknown-none-softfloat`, компонент `llvm-tools`)
- `qemu-system-aarch64`
- `make`

```sh
make build      # собрать userspace + ядро + ядро в чистый бинарный образ kernel.bin
make run        # запустить под QEMU (EL1)
make run-el2    # запустить под QEMU в EL2 (проверка перехода EL2 -> EL1)
```

## Структура

- `Cargo.toml` — ядро, crate `vv1raos` (`#![no_std]`, `#![no_main]`)
- `src/main.rs` — точка входа ядра, инициализация UART, MMU, PMM, GICv3, таймер, планировщик, IPC
- `src/boot.S` — точка входа AArch64, заголовок arm64-образа, таблица векторов
- `linker.ld` — компоновщик, адрес загрузки по умолчанию `0x40080000` (QEMU virt), переопределяется через `LOAD_BASE`
- `user/` — независимое пользовательское пространство, crate `vv1raos-user` (selftest, shell)
- `Makefile` — только `build` / `run` / `run-el2` (без целей, связанных с Android)

## Загрузка на реальном устройстве

Адрес загрузки задаётся символом `__load_base`. Для телефона:

```sh
LOAD_BASE=0x... make build   # должен быть кратен 2 MiB и равен 0x80000 mod 2MiB
```

Результат: `kernel.bin` — можно прошить как ядро, без образа загрузки Android.

# UFS драйвер для SM6375

## Статус: ✅ Базовая инфраструктура готова

### Компоненты

| Компонент | Статус | Описание |
|-----------|--------|----------|
| **UFS HC init** | ✅ | Инициализация Host Controller @ 0x4804000 |
| **UFS PHY init** | ✅ | Настройка QMP v4 PHY @ 0x4807000 |
| **Link startup** | ✅ | DME_LINKSTARTUP, ожидание ULSS |
| **Block device API** | ✅ | `ufs_read_sectors(lba, count, buf)` |
| **SCSI READ(10)** | ⏳ | Базовая реализация (не тестирована) |
| **Интеграция initramfs** | ⏳ | Загрузка cpio с UFS вместо RAM |

---

## API

### `ufs_read_sectors(lba: u64, count: u16, buf: &mut [u8])`

Читает `count` секторов (512 байт каждый) начиная с LBA в `buf`.

**Пример:**
```rust
let mut buf = [0u8; 4096];
// Читаем 8 секторов (4KB) начиная с LBA 0
ufs_read_sectors(0, 8, &mut buf)?;
```

### `ufs_get_capacity() -> Result<u64, DriverError>`

Возвращает размер накопителя в секторах.

---

## Интеграция с initramfs

### Текущая схема (RAM):
1. Загрузчик загружает `initramfs.cpio` в RAM @ `info.initrd_start`
2. Ядро парсит cpio из RAM в `fdt::load_initramfs()`
3. Файлы загружаются через `ElfLoader::load()`

### Новая схема (UFS):
1. Загрузчик передаёт **offset initramfs на UFS** (например, partition @ LBA 2048)
2. Ядро читает cpio заголовок с UFS через `ufs_read_sectors()`
3. Файлы загружаются on-demand из UFS

**План изменений:**

```rust
// src/fdt.rs
pub fn load_initramfs_from_ufs(start_lba: u64) -> Result<(), &'static str> {
    let mut buf = [0u8; 512];
    
    // Читаем первый сектор (cpio magic)
    crate::drivers::ufs::ufs_read_sectors(start_lba, 1, &mut buf)?;
    
    // Проверяем magic "070701" (ASCII newc format)
    if &buf[0..6] != b"070701" {
        return Err("not a valid cpio archive");
    }
    
    // Парсим cpio заголовки, читаем файлы по мере надобности
    // ...
}
```

---

## Device Tree требования

### UFS Host Controller узел:
```dts
ufshc@4804000 {
    compatible = "qcom,ufshc";
    reg = <0x4804000 0x3000>;
    interrupts = <GIC_SPI 356 IRQ_TYPE_LEVEL_HIGH>;
    clocks = <&gcc GCC_UFS_CORE_CLK>,
             <&gcc GCC_UFS_AXI_CLK>,
             <&gcc GCC_UFS_PHY_CLK>;
    clock-names = "core", "bus", "phy";
    status = "okay";  // <-- должен быть enabled загрузчиком!
};
```

### UFS PHY узел:
```dts
ufsphy_mem@4807000 {
    compatible = "qcom,ufs-phy-qmp-v4";
    reg = <0x4807000 0xddc>;
    clocks = <&gcc GCC_UFS_PHY_CLK>;
    status = "okay";
};
```

**⚠️ В текущем DTB `status = "disabled"`** — загрузчик должен patching или передать корректный DTB.

---

## Тестирование

### На QEMU (virt):
- UFS не пробируется (нет узла в DTB) ✅
- `stage 8 OK` работает с RAM initramfs ✅

### На железе:
1. Загрузчик включает UFS в DTB (`status = "okay"`)
2. Передаёт корректный DTB с узлами `qcom,ufshc` и `qcom,ufs-phy-qmp-v4`
3. Ядро пробирует UFS драйвер → `ufs: probe ok`
4. Инициализирует PHY → `ufs: PHY init done`
5. Запускает link → `ufs: link startup complete`
6. Читает initramfs с partition через `ufs_read_sectors()`

---

## Следующие шаги

1. **Тест на железе**: проверить `ufs: probe ok` и `link startup complete`
2. **READ CAPACITY**: реализовать SCSI READ CAPACITY(10) для `ufs_get_capacity()`
3. **Partition table**: добавить парсинг GPT для поиска initramfs partition
4. **On-demand loading**: загружать файлы из cpio по мере обращения (не весь initramfs в RAM)
5. **Write support**: SCSI WRITE(10) для persistence (логи, конфиги)

---

## Debug

### Проверка probe:
```
kprintln!("ufs: HC mapped at {:#x}, size={:#x}", hc_base, hc_size);
kprintln!("ufs: PHY init done");
kprintln!("ufs: HC enabled");
kprintln!("ufs: link startup complete");
kprintln!("ufs: probe ok");
```

### Регистры для debug:
- `HCE` (0x34): должен быть `1` после включения
- `HCS` (0x30): Host Controller Status, бит `UCRDY` = UIC command ready
- `IS` (0x20): Interrupt Status, `ULSS` = link startup, `UTRCS` = transfer complete
- `UTRLDBR` (0x58): Doorbell register, бит 0 = slot 0 active

### Earlycon для UFS debug:
```rust
// В kernel_main перед platform_drivers_init():
earlycon_init(0x4c8c000); // MSM UART @ 0x4c8c000
kprintln!("UFS: starting probe...");
```

---

## Ссылки

- JEDEC UFS v3.1 Specification: https://www.jedec.org/standards-documents/docs/jesd220d
- Linux UFS Qualcomm: `drivers/scsi/ufs/ufs-qcom.c`
- UFSHCI v3.0: Universal Flash Storage Host Controller Interface
- QMP PHY v4: Qualcomm Multi-Purpose PHY для UFS HS-G3
