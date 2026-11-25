.RECIPEPREFIX = >
# WSL: гарантируем, что cargo/rustc — shims rustup из ~/.cargo/bin (по умолчанию
# PATH содержит системный cargo из /usr/bin, который не видит цель
# aarch64-unknown-none-softfloat и ломает сборку).
PATH ?= /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
PATH := $(HOME)/.cargo/bin:$(PATH)
export PATH
TARGET  := aarch64-unknown-none-softfloat
ELF     := target/$(TARGET)/release/mk
BIN     := kernel.bin
OBJCOPY ?= $(shell find $(shell rustc --print sysroot) -name llvm-objcopy | head -n1)
INITRD  := initramfs.cpio
QEMU    := qemu-system-aarch64 -machine virt,gic-version=3 -cpu max -m 512M -smp 4 -serial stdio -initrd $(INITRD) -kernel $(BIN)

build:
> cd user && cargo build --release
> cargo build --release
> $(OBJCOPY) -O binary $(ELF) $(BIN)

initramfs:
> rustc --edition 2021 make_initramfs.rs -o make_initramfs_host 2>/dev/null
> ./make_initramfs_host

run: build initramfs
> $(QEMU)

# Старт в EL2: проверяет путь EL2 -> EL1
run-el2: build initramfs
> $(QEMU),virtualization=on

# Репетиция телефонного пути: fake DTB в форме SM6375 (geni-console, WDT,
# GCC/SPMI/UFS с runtime-мэппингом MMIO, no-map резерв). См. gen_fake_dtb.py.
run-dtb: build initramfs
> qemu-system-aarch64 -machine virt,gic-version=3 -cpu max -m 512M -smp 4 -serial stdio -initrd $(INITRD) -kernel $(BIN) -dtb fake_sm6375.dtb

# boot.img v4 для `fastboot boot boot.img` на Poco X4 Pro (SM6375/Blair).
# DDR base = 0x80000000, kernel LOAD_BASE = 0x80080000 (text_offset=0x80000).
# DTB берётся из vendor_boot_patched.img (там консольный GENI UART status="ok").
bootimg: build initramfs
> mkbootimg --header_version 4 --kernel $(BIN) --ramdisk $(INITRD) --base 0x80000000 --pagesize 4096 --kernel_offset 0x80000 --tags_offset 0x01E00000 --os_version 14.0.0 --os_patch_level 2026-11 -o boot.img

# boot.img для QEMU virt (тестирование)
bootimg-qemu: build initramfs
> mkbootimg --header_version 4 --kernel $(BIN) --ramdisk $(INITRD) --base 0x40000000 --pagesize 4096 --kernel_offset 0x80000 --tags_offset 0x01E00000 --os_version 14.0.0 --os_patch_level 2026-11 -o boot-qemu.img

.PHONY: build initramfs run run-el2 bootimg