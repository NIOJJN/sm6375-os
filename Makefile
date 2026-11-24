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

bootimg: build
> mkbootimg --header_version 4 --kernel $(BIN) --os_version 14.0.0 --os_patch_level 2026-11 -o boot.img

.PHONY: build initramfs run run-el2 bootimg