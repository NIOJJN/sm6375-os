# vv1raos
**English** | [Русский](README.ru.md)



A standalone, bare-metal kernel for AArch64, written in Rust.

vv1raos does not depend on Android or Linux. It is a `#![no_std]`, `#![no_main]` kernel with no external crate dependencies. It builds with stable Rust and runs under QEMU (`virt` machine), and it is being brought up on real hardware: the Qualcomm SM6375 (Snapdragon 695, "Blair") in the Poco X4 Pro.

## Status

| Area                                  | Status                                                      |
| ------------------------------------- | ----------------------------------------------------------- |
| QEMU `virt` (EL1 and EL2)             | Working                                                     |
| QEMU with a fake SM6375-shaped DTB    | Used to rehearse the phone boot path (`make run-dtb`)       |
| Real hardware (Poco X4 Pro, SM6375)   | Bring-up in progress, not verified end to end               |
| UFS driver                            | Takeover path implemented, **untested on hardware**         |

## Features

- AArch64 boot: arm64 image header, exception vector table, EL2 → EL1 transition, secondary-CPU entry for PSCI `CPU_ON`
- Device tree parsing: RAM, reserved and no-map regions, GIC, timer, PSCI, CPUs, UARTs, watchdog, TLMM, `/chosen` initrd
- Two consoles: PL011 (QEMU `virt`) and Qualcomm GENI SE UART (SM6375), with early output before the MMU is up
- APSS watchdog is disabled at boot when the DTB has a watchdog node
- Identity-mapped kernel with W^X page tables, a page-granular physical memory manager, and per-process address spaces with 8-bit ASIDs
- Userspace in EL0: ELF loader, system calls via `svc #0`, demand paging in the heap window, processes with several threads
- Capability-based access to kernel objects (endpoints, shared memory, ports) with reference counting and revocation when the owner dies
- Synchronous IPC (call/recv/reply, as in L4/seL4) with timeouts, cancellation and priority inheritance; asynchronous ports with notifications; futexes that work across processes through shared memory
- GICv3 with an SGI self-test, Generic Timer at 100 Hz
- SMP: up to 4 cores started via PSCI `CPU_ON`, per-CPU state, IPIs, a priority scheduler with preemption
- cpio `initramfs`; the kernel hands off to `init` (PID 1) in EL0, which starts services, tests and the shell
- Platform device model driven by the DTB, with drivers registered through `.init_array` (see [Platform driver model](#platform-driver-model))
- SM6375 drivers: GCC clocks, SPMI, PMIC, GENI UART, TLMM pinmux, Apps SMMU (SID bypass), UFS, GPT, clock-tree verification, and a PMIC vibration-motor indicator for headless boot diagnostics
- Exception handler that dumps registers, a frame-pointer backtrace, stack words and the faulting process's page-table walk

## Requirements

- Rust (stable) with:
  - target `aarch64-unknown-none-softfloat`
  - component `llvm-tools` (the Makefile finds `llvm-objcopy` in the Rust sysroot)
- `make`
- `qemu-system-aarch64` (for `run`, `run-el2`, `run-dtb`)
- `mkbootimg` (for `bootimg` and `bootimg-qemu`)

```sh
rustup target add aarch64-unknown-none-softfloat
rustup component add llvm-tools
```

The Makefile puts `~/.cargo/bin` first in `PATH`, so rustup's `cargo` is used instead of a system one that lacks the bare-metal target (this matters on WSL).

## Build and run

| Target             | What it does                                                                                          |
| ------------------ | ----------------------------------------------------------------------------------------------------- |
| `make build`       | Build userspace (`user/`), then the kernel, then convert the ELF to a raw image `kernel.bin`          |
| `make initramfs`   | Build `initramfs.cpio` (compiles and runs `make_initramfs.rs` on the host)                            |
| `make run`         | Run under QEMU `virt` (GICv3, 4 CPUs, 512 MiB) at EL1                                                 |
| `make run-el2`     | Same, with `virtualization=on`, to test the EL2 → EL1 transition                                      |
| `make run-dtb`     | Run under QEMU with `fake_sm6375.dtb`, a DTB shaped like the SM6375 one (see `gen_fake_dtb.py`)       |
| `make bootimg`     | Build `boot.img` (header v4) for `fastboot boot boot.img` on the Poco X4 Pro                          |
| `make bootimg-qemu`| Build `boot-qemu.img` (header v4) with the QEMU `virt` base address `0x40000000`                      |

The kernel ELF is `target/aarch64-unknown-none-softfloat/release/mk` (the crate is named `mk`). Release builds use `opt-level = "s"`, LTO and `panic = "abort"`.

## Running on a real device

Target: Poco X4 Pro (SM6375 / Blair).

- DDR starts at `0x80000000`. The kernel is loaded at `0x80080000`: the arm64 image header sets `text_offset = 0x80000`, and `make bootimg` uses `--base 0x80000000 --kernel_offset 0x80000`.
- The linker script's default load address is `0x40080000` (QEMU `virt`). It is overridden through the `__load_base` symbol, set with `LOAD_BASE`: use `LOAD_BASE=0x80080000 make bootimg`. `LOAD_BASE` must be a multiple of 2 MiB plus `0x80000`.
- The device tree comes from the device's `vendor_boot`, patched so that the console GENI UART node has `status = "okay"`.
- The image is started with `fastboot boot boot.img`, so nothing is written to flash.

> **Warning:** booting on real hardware is experimental. Know how to recover your device before you try.

## Project layout

| Path                  | Description                                                                                     |
| --------------------- | ----------------------------------------------------------------------------------------------- |
| `Cargo.toml`          | Kernel crate `mk` (`#![no_std]`, `#![no_main]`, no dependencies)                                |
| `src/main.rs`         | `kernel_main`, exception handler and panic handler                                              |
| `src/boot.S`          | Entry point, arm64 image header, EL2 → EL1, vector table, secondary-CPU entry                   |
| `linker.ld`           | Linker script; `.init_array` is collected explicitly and run from `kernel_main`                 |
| `src/*.rs` | Kernel modules: `fdt`, `mmu`, `pmm`, `mem`, `aspace`, `gic`, `irq`, `timer`, `smp`, `sched`, `proc`, `elf`, `syscall`, `ipc`, `port`, `shm`, `obj`, `initramfs`, `platform`, `uart`, `console`, `wdt`, `sync`, `config` |
| `src/fs.rs`, `src/virtio_blk.rs` | FAT32 over a polled virtio-blk driver. Present in the tree but not declared in `main.rs`, so **not built** |
| `src/drivers/`        | SM6375 drivers: `gcc_sm6375`, `spmi`, `pmic_sm6375`, `geni_uart`, `msm_uart`, `pinmux`, `smmu`, `ufs`, `gpt`, `clock_verify`, `vibro` |
| `user/`               | Independent userspace crate `vv1raos-user` (selftest, shell)                                    |
| `make_initramfs.rs`   | Host tool that builds the cpio initramfs                                                        |
| `build.rs`            | Cargo build script                                                                              |
| `rust-toolchain.toml` | Pinned toolchain configuration                                                                  |

## Boot

### Entry (`src/boot.S`)

1. Masks all exceptions (DAIF) and checks the current exception level; EL2 is dropped to EL1 (`HCR_EL2.RW = 1`, counter access enabled, GICv3 system-register access enabled), EL1 is used directly. The MPIDR check is skipped on purpose: SM6375's CPU0 has `Aff1 = 1`.
2. At EL1: MMU and caches off, stack set up, `VBAR_EL1` installed, FP/SIMD enabled, BSS cleared, then `kernel_main(dtb)`.
3. Secondary cores enter at `secondary_entry` (via PSCI `CPU_ON`), repeat the EL2 → EL1 drop, take a 32 KiB stack from `__secondary_stacks` (four slots, indexed by the CPU slot number), and call `secondary_main`.

The arm64 image header declares a little-endian kernel with 4 KiB pages.

### `kernel_main` stages (`src/main.rs`)

1. Boot CPU init, driver registration (`.init_array`), DTB parse
2. Watchdog off, UART consoles up
3. PMM init (reserving kernel, DTB, no-map and reserved-memory regions), MMU on, PMM self-test
4. Shared memory, ports, IPC and initramfs init
5. Platform devices and driver probing, TLMM pinmux, clock dump
6. GICv3 and SGI self-test, Generic Timer, scheduler
7. SMP bring-up and self-test
8. Spawn `init` (PID 1) and start the scheduler

Fatal setup errors (invalid DTB, no RAM, no GICv3 node, `CNTFRQ_EL0 == 0`) are logged and the CPU halts in `wfe`.

### Kernel image layout (`linker.ld`)

`.text` (with `.text.boot` first), `.rodata`, `.data`, `.init_array`, `.bss`, then a 64 KiB boot stack; each is 4 KiB aligned. `__image_size` is the size from `__kernel_start` to `__kernel_end`.

## Consoles

- **PL011** (QEMU `virt`): base address from the DTB.
- **GENI SE UART** (SM6375): the legacy UARTDM drivers (`uart.rs`, `msm_uart.rs`) are not used on this SoC, since every UART is a GENI serial engine inside QUPv3. On the Poco X4 Pro the console node is `/soc/qcom,qup_uart@4c8c000` (`compatible = "qcom,msm-geni-console"`). The driver assumes XBL/ABL has already configured the engine in UART mode with clocks on; it only selects FIFO mode, clears pending interrupts, and polls the TX path. **It is transmit-only: there is no receive support yet.**

When both consoles are present, output goes to both (under QEMU this exercises the GENI path too). `emergency_write` bypasses the console lock and is used by the exception and panic handlers. Input comes only from the PL011: the `read` syscall (used by the shell) waits on it, so there is no keyboard input on the phone yet.

## Platform driver model

Drivers describe themselves with a `Driver` (name, up to 8 `compatible` strings, `probe`) and register through a constructor placed in `.init_array` (the `platform_driver!` macro does this). `kernel_main` runs all constructors before parsing the DTB.

- **Population:** only DTB nodes whose `compatible` matches a registered driver become platform devices. A real SM6375 DTB has hundreds of such nodes and the registry is limited (32 drivers, 64 devices), so the filter keeps the registry from filling with devices nobody handles.
- **Resources:** memory regions, IRQs, named clocks, power domains and IOMMU stream IDs.
- **MMIO mapping:** before `probe`, every memory resource of the device is identity-mapped as device memory, since regions like GCC, SPMI and UFS are not in the boot-time MMIO list.
- **Probe order:** devices are probed in the order they were registered from the DTB. There is no dependency resolution or deferred probe. A failed probe is logged (`platform: probe failed for <name>: <error>`) and the device is marked failed.

## Kernel internals

### Memory

- **Page tables:** 4 KiB granule, 48-bit VA, `TTBR0` only (`EPD1 = 1`), identity mapping (VA == PA), 2 MiB blocks where possible. The kernel image is mapped W^X: text is read-execute, rodata is read-only, data and BSS are read-write and non-executable. All RAM is Normal write-back and non-executable.
- **Device memory:** everything below the first RAM bank (capped at 4 GiB) is mapped as Device-nGnRnE. Regions that the DTB marks `no-map` are unmapped, unless they overlap the kernel image. Device regions found later by the platform layer are mapped into the live tables by `map_device_identity`.
- **PMM:** a bump pointer over the main RAM range plus a singly linked free list. It skips up to 256 reserved ranges (kernel image, DTB, `no-map` and all of `/reserved-memory`, which on SM6375 is around 20 regions). Pages are zeroed on allocation. `alloc_contiguous` serves large buffers such as the initramfs.
- **ASIDs:** 8-bit, 1 to 255 for processes, 0 for the kernel. They are reused after 255 address spaces, so switching to a reused ASID invalidates its TLB entries on all cores.

### Processes and address spaces

Each process has its own L0 table and a copy of the kernel's L1 table. Its user window is one L1 entry (1 GiB) starting at `0x40_0000_0000`:

| Range (offset from `USER_BASE`) | Use                                                     |
| ------------------------------- | ------------------------------------------------------- |
| `0x00_0000` to `0x07_0000`      | ELF image (`ET_EXEC`, AArch64, `PT_LOAD` segments)      |
| below `0x08_0000`               | Main thread stack, 16 pages, top at `USER_BASE + 0x80000` |
| `0x10_0000` to `0x20_0000`      | Shared-memory window                                    |
| `0x20_0000` to `0x1000_0000`    | Anonymous memory: heap and thread stacks                |

Pages owned by a process carry a software bit (bit 55) and are freed when the address space is destroyed. Shared-memory and MMIO pages are not marked, since they have their own reference counts. `copy_from_user` and `copy_to_user` walk the process's page tables instead of dereferencing user addresses, and the latter only writes to pages the process can write.

A fault from EL0 inside the anonymous window is resolved by demand paging. Any other fault kills the process with exit code 255.

Process limits (`proc.rs`): 8 processes, 8 threads per process, 16 capabilities per process, 32 mappings per process. A finished child stays a zombie until its parent collects the exit code with `wait`; orphans are reaped automatically.

### Scheduler and SMP

- Up to 32 kernel threads; slots 0 to 3 are the per-core idle threads. Each thread has a 128 KiB kernel stack.
- Priorities 0 to 7 (default 3). A process cannot raise a thread above its `max_prio`. A server that handles a request temporarily inherits the client's priority.
- Time slice of 5 ticks (50 ms at 100 Hz). One global scheduler lock protects the thread table, and each core keeps its own current thread, `TTBR0` and ASID in a per-CPU structure.
- A context switch has two steps: the outgoing thread becomes `Switching`, and it becomes `Ready` only after its stack pointer has been saved, so another core cannot pick it up too early.
- The kernel preempts only on return to EL0 (and at its own scheduling points such as idle, sleep and IPC), never in the middle of EL1 code.
- A `wake` that arrives before the thread has blocked is remembered, so wake-ups are not lost on SMP.
- IPIs use SGI 2 (`IPI_RESCHED`). Only CPU0 advances the global tick counter.
- At most 4 cores are used (`MAX_CPUS = 4`, taken from the first four `cpu@` nodes of the DTB). SM6375 has 8 cores.
- Locks: `SpinLock` masks IRQs on the current core while held. `IrqGuard` is a single-core critical section.

### Objects and capabilities

All kernel objects live in one table of 256 entries (endpoint, shared memory, MMIO, port) with a type, an owner PID and a reference count. A process reaches an object only through a capability in its own table: an object ID plus rights (`SEND`, `RECV`, `MAP`, `GRANT`). Rights are checked on every syscall, and the object type is checked through the table, so it cannot be forged. A capability can be sent inside an IPC message only if it has `GRANT`, and the receiver gets it without `GRANT`. When a process dies, every other process's capabilities to its objects are revoked and its objects are destroyed.

### IPC

- **Synchronous (endpoints):** `call`, `recv`, `reply` rendezvous. A message is 4 words, up to 1024 bytes of data and at most one capability. The kernel fills in the sender's PID, so it cannot be spoofed. Waits can end by timeout, by `cancel` from another thread, or because the peer died. There are 16 endpoints, each with a send queue of 8.
- **Asynchronous (ports):** `port_send` queues a message (the queue holds 8, and a full queue returns `ERR_AGAIN`), `port_notify` sends a signal with no payload, and `port_recv` returns either, with sender PID 0 for a notification. There are 16 ports.
- **Futex:** `futex_wait` and `futex_wake` key on the physical address of a 32-bit word, so they work between processes through shared memory. Registering as a waiter and rechecking the value happen under one lock, which avoids a lost wake-up on SMP.

### System calls

`svc #0` from EL0. The number is in `x8`, arguments in `x0` to `x4`, and the status comes back in `x0` (0 is success, otherwise an error code from `ipc.rs`). Extra results come back in `x1` and up. IPC messages are passed as a 64-byte `UMsg` structure in process memory.

| Group      | Calls                                                                                                           |
| ---------- | --------------------------------------------------------------------------------------------------------------- |
| Basic      | `YIELD` (0), `WRITE` (1), `EXIT` (2), `GETPID` (3), `SLEEP` (4), `TICKS` (5), `GETTID` (18), `READ` (28)        |
| IPC        | `CALL` (6), `RECV` (7), `REPLY` (8), `EP_CREATE` (13), `CANCEL` (21)                                           |
| Memory     | `SHM_CREATE` (12), `SHM_MAP` (9), `MAP_ANON` (14), `UNMAP` (15)                                                |
| Threads    | `THREAD_CREATE` (16), `THREAD_EXIT` (17), `SET_PRIO` (22), `FUTEX_WAIT` (19), `FUTEX_WAKE` (20)                |
| Ports      | `PORT_CREATE` (24), `PORT_SEND` (25), `PORT_RECV` (26), `PORT_NOTIFY` (27)                                     |
| Capability | `CAP_DROP` (23)                                                                                                 |
| Processes  | `SPAWN` (37), `WAIT` (38)                                                                                       |
| Statistics | `MEM_STATS` (39), `KSTATS` (40)                                                                                 |

`EXIT` ends the whole process, `THREAD_EXIT` only the calling thread. `WRITE` is limited to 256 bytes and prefixes the output with the PID.

### initramfs

The kernel finds the ramdisk through `/chosen` (`linux,initrd-start` and `linux,initrd-end`), copies it into physically contiguous memory from the PMM, and parses it as a cpio `newc` archive.

- gzip is accepted, but only if the deflate stream uses stored (uncompressed) blocks; compressed blocks are rejected.
- The archive is expanded into a static 64 MiB buffer.
- Up to 32 files, with names up to 63 bytes. Empty files are skipped.
- `spawn` looks for `bin/<name>`, then `<name>`, then `sbin/<name>`.

## UFS driver (SM6375)

> **Status: rewritten, not verified on hardware.**

### Operating modes

- **Takeover (default):** if the bootloader has already brought UFS up (`HCE = 1`, `HCS.DP = 1`), the kernel does not touch the PHY, `HCE` or the link. It stops any request lists left by the bootloader and installs its own. This is the main path for `fastboot boot`.
- **Full init (fallback, `FORCE_FULL_INIT = true` or controller not up):** PHY init, `HCE` cycle, `DME_LINKSTARTUP` (3 attempts). The PHY sequence is a minimal, unverified register sequence; real QMP v4 tables from Linux (`phy-qcom-qmp-ufs`) are needed, so this path will most likely not work on real hardware yet.

All UFSHCI structures (UTRD, UPIU, PRDT) were written against UFSHCI 3.0 and Linux `ufshci.h`/`ufs.h` from memory and have not been tested on a device.

### Probe sequence

1. Map the HC resource (and the PHY, defaulting to `0x4807000`); the driver matches `qcom,ufshc` and `qcom,ufs-qcom`.
2. Enable the `core`, `bus` and `phy` clocks through the SM6375 GCC driver (failures are non-fatal, the bootloader may have enabled them), then verify the clock tree.
3. Request SMMU bypass for the controller's stream ID, before any DMA.
4. Takeover or full init (see above), then hand the controller the kernel's own request lists.
5. In takeover mode, if `TEST UNIT READY` on LUN 0 already succeeds, device init is skipped. Otherwise: `NOP OUT`, set `fDeviceInit` and wait for it to clear, then `TEST UNIT READY`.
6. `READ CAPACITY(10)` gives the block count and block size.
7. `ufs: probe ok`, then `gpt_init()` runs.

### Components

| Component                     | Status | Notes                                                                           |
| ----------------------------- | ------ | ------------------------------------------------------------------------------- |
| Takeover path                 | ⏳     | Implemented; untested on hardware                                               |
| UFS PHY init (QMP v4)         | ❌     | Minimal sequence, **unverified**; real tables needed                            |
| Link startup                  | ⏳     | `DME_LINKSTARTUP` with retries; untested                                        |
| Device init                   | ⏳     | `NOP OUT` + `fDeviceInit` query flag; untested                                  |
| SCSI `READ(10)`               | ⏳     | Implemented; untested on hardware                                               |
| SCSI `READ CAPACITY(10)`      | ⏳     | Capacity and block size read at probe; untested on hardware                     |
| Block device API              | ✅     | `ufs_read_sectors`, `ufs_get_capacity`, `ufs_block_size`                        |
| GPT (ESP lookup)              | ⏳     | Runs after a successful probe; see [GPT](#gpt)                                  |
| initramfs from UFS            | ⏳     | Planned: load cpio from UFS instead of RAM                                      |
| SCSI `WRITE(10)`              | ❌     | Not implemented                                                                 |

### Design notes and limits

- Only **LUN 0** is used.
- One request slot (slot 0) is used at a time, behind a spinlock; completion is polled on the doorbell register. The task management list is not used.
- The DMA area (request list, command descriptor, PRDT and a 16 KiB data buffer) is a static, 4 KiB-aligned structure. Reads are split into chunks that fit the 16 KiB buffer.
- Buffers are assumed to be identity-mapped (VA == PA) and cacheable (Normal WB), so cache maintenance (`dc civac`) is done around DMA.
- `READ(10)` and `READ CAPACITY(10)` use 32-bit LBAs.

### API

```rust
/// Logical block size of LUN 0 in bytes (usually 4096); 0 if UFS is not ready.
pub fn ufs_block_size() -> usize

/// Capacity of LUN 0 in logical blocks.
pub fn ufs_get_capacity() -> Result<u64, DriverError>

/// Read `count` logical blocks of LUN 0 starting at `lba` into `buf`.
/// `buf` must hold at least `count * ufs_block_size()` bytes.
/// Returns the number of bytes read.
pub fn ufs_read_sectors(lba: u64, count: u16, buf: &mut [u8]) -> Result<usize, DriverError>
```

`ufs_read_sectors` returns `NotReady` before a successful probe and `InvalidArg` for `count == 0`, a too-small buffer, or a range past the end of the device.

Example (a device with 4096-byte logical blocks):

```rust
let mut buf = [0u8; 4096];
// Read one logical block (4 KiB) at LBA 0
let n = ufs_read_sectors(0, 1, &mut buf)?;
assert_eq!(n, 4096);
```

### GPT

`gpt_init()` runs after a successful UFS probe. It exists so the kernel can find its initramfs on UFS without relying on the bootloader's `initrd-start`/`initrd-end` in `/chosen`.

- Reads the GPT header at LBA 1 and checks the `EFI PART` signature. A header CRC mismatch is only logged, because some bootloaders do not update it.
- Requires 128-byte partition entries and a UFS block size between 128 and 4096 bytes. LBAs are in the device's logical blocks.
- Scans the entry array for the EFI System Partition (type GUID `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`) and caches the first match. Other partition types are not looked up yet.
- API: `gpt_find_esp()`, `gpt_esp_info()` (first LBA and size in blocks), `gpt_read_esp(lba_offset, count, buf)`.
- Logs `gpt: ESP found at LBA <first>..<last> (<n> MiB)` or `gpt: ESP not found: <reason>`.

### initramfs integration

**Current scheme (RAM):**

1. The bootloader places `initramfs.cpio` in RAM (the ramdisk in `boot.img`, or `-initrd` under QEMU).
2. The kernel parses the cpio archive from RAM in `initramfs::init()`.
3. Files are loaded through the ELF loader.

**Planned scheme (UFS):**

1. The kernel locates the initramfs partition through GPT (the ESP today).
2. It reads the cpio header from UFS via `ufs_read_sectors()` and checks for the `070701` (newc) magic.
3. Files are loaded on demand from UFS.

### Device tree requirements

The bootloader must provide a DTB in which the UFS nodes are enabled.

```dts
ufshc@4804000 {
    compatible = "qcom,ufshc";
    reg = <0x4804000 0x3000>;
    interrupts = <GIC_SPI 356 IRQ_TYPE_LEVEL_HIGH>;
    clocks = <&gcc GCC_UFS_CORE_CLK>,
             <&gcc GCC_UFS_AXI_CLK>,
             <&gcc GCC_UFS_PHY_CLK>;
    clock-names = "core", "bus", "phy";
    status = "okay";
};

ufsphy_mem@4807000 {
    compatible = "qcom,ufs-phy-qmp-v4";
    reg = <0x4807000 0xddc>;
    clocks = <&gcc GCC_UFS_PHY_CLK>;
    status = "okay";
};
```

> In the original DTB both nodes are `status = "disabled"`. The bootloader must patch them or pass a corrected DTB.

### Testing

**QEMU (`virt`):** UFS is not probed (no node in the DTB). The initramfs is loaded from RAM and the `stage 8 OK` self-test passes.

**Hardware:**

1. The bootloader enables UFS in the DTB.
2. The kernel probes the driver and prints `ufs: HC at <base>, size=<size>, PHY at <base>`.
3. In takeover mode the log shows `ufs: HC already up (HCE=1, DP=1), taking over`.
4. In full-init mode: `ufs: PHY init done (unverified)`, `ufs: HC enabled`, `ufs: link startup complete`.
5. `ufs: LUN 0: <blocks> blocks x <size> bytes`, then `ufs: probe ok`, then the `gpt:` line.
6. The initramfs is read from a partition via `ufs_read_sectors()` (planned).

### Debugging

Expected log lines:

```
ufs: HC at <base>, size=<size>, PHY at <base>
ufs: HC already up (HCE=1, DP=1), taking over      (takeover mode)
ufs: full HC init                                  (full-init mode)
ufs: PHY init done (unverified)
ufs: HC enabled
ufs: link startup complete
ufs: LUN 0: <blocks> blocks x <size> bytes
ufs: probe ok
gpt: ESP found at LBA <first>..<last> (<n> MiB)
```

Other messages worth knowing: `ufs: doorbell busy`, `ufs: NOP OUT failed`, `ufs: fDeviceInit did not clear`, `ufs: request failed, OCS=<n>`, `ufs: fatal error, IS=<n>`.

Useful UFSHCI registers:

| Register  | Offset | Meaning                                                              |
| --------- | ------ | -------------------------------------------------------------------- |
| `IS`      | `0x20` | Interrupt status (`ULSS` = link startup, `UTRCS` = transfer complete) |
| `HCS`     | `0x30` | Host controller status (`UCRDY` = UIC command ready)                 |
| `HCE`     | `0x34` | Host controller enable; must read `1` after enabling                 |
| `UTRLDBR` | `0x58` | Doorbell register; bit 0 = slot 0 active                             |

Early console for debugging (the GENI UART on SM6375, at `0x4c8c000` on the Poco X4 Pro):

```rust
crate::drivers::geni_uart::early_init(0x4c8c000);
kprintln!("UFS: starting probe...");
```

If the kernel faults, `exception_handler` prints the exception vector, `ESR`, `ELR`, `FAR`, registers, a frame-pointer backtrace, stack words and the page-table walk of the faulting process, then halts. The vibration-motor indicator (`vibro`) signals boot stages on a device with no console.

### Roadmap

- [ ] Verify probe and takeover path on hardware
- [ ] Replace the unverified PHY sequence with the real QMP v4 tables from Linux
- [ ] GPT: look up arbitrary partitions, not only the ESP
- [ ] On-demand file loading from cpio (avoid loading the whole initramfs into RAM)
- [ ] SCSI `WRITE(10)` for persistence (logs, configuration)
- [ ] UART receive on the GENI console
- [ ] Use all 8 SM6375 cores (`MAX_CPUS` is 4)
- [ ] Compressed (deflate) gzip support for the initramfs
- [x] READ CAPACITY
- [x] GPT ESP lookup

## References

- [JEDEC UFS v3.1 (JESD220D)](https://www.jedec.org/standards-documents/docs/jesd220d)
- UFSHCI v3.0: Universal Flash Storage Host Controller Interface
- Linux Qualcomm UFS driver: `drivers/scsi/ufs/ufs-qcom.c`
- Linux Qualcomm GENI serial driver: `drivers/tty/serial/qcom_geni_serial.c`
- QMP PHY v4: Qualcomm Multi-Purpose PHY for UFS HS-G3

## License

MIT. See [LICENSE](LICENSE).
