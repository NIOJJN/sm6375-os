# vv1raos

Автономное (standalone) bare-metal ядро для AArch64 на Rust. Не зависит от Android:
нет Binder, нет `mkbootimg`, нет `boot.img`, нет пользовательского пространства Android.
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
