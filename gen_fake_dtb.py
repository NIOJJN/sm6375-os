"""Генератор fake DTB для репетиции загрузки SM6375 на QEMU virt.

Смешивает:
  - QEMU-совместимые узлы (CPU, PSCI, таймер, GIC-600 @ QEMU адреса, PL011),
    чтобы ядро реально грузилось на `-machine virt`;
  - телефонные узлы SM6375 (форма как в DTB Poco X4 Pro): /soc с
    #address-cells=1, geni-console @ 0x4c8c000, WDT @ 0xf410000,
    GCC @ 0x1400000, SPMI @ 0x1c40000 + PMIC, UFS @ 0x4804000 + PHY,
    а также fake no-map область для проверки резервирования.

Цель: прогнать на QEMU ВЕСЬ путь телефона — парсинг ac=1, включение
GENI-консоли, отключение WDT, probe драйверов с runtime-мэппингом MMIO.

Использование:
  python3 gen_fake_dtb.py                # -> fake_sm6375.dtb
  qemu-system-aarch64 ... -dtb fake_sm6375.dtb
"""
import struct

# ---------- helpers ----------

class DTB:
    def __init__(self):
        self.strings = bytearray()
        self.strmap = {}
        self.struct = bytearray()
        self.depth = 0

    def s(self, name):
        if name not in self.strmap:
            self.strmap[name] = len(self.strings)
            self.strings += name.encode() + b"\x00"
        return self.strmap[name]

    def begin(self, name):
        self.struct += struct.pack(">I", 1)
        self.struct += name.encode() + b"\x00"
        while len(self.struct) % 4:
            self.struct += b"\x00"
        self.depth += 1

    def end(self):
        self.struct += struct.pack(">I", 2)
        self.depth -= 1

    def prop_bytes(self, name, data):
        self.struct += struct.pack(">I", 3)
        self.struct += struct.pack(">II", len(data), self.s(name))
        self.struct += data
        while len(self.struct) % 4:
            self.struct += b"\x00"

    def prop_u32(self, name, values):
        self.prop_bytes(name, b"".join(struct.pack(">I", v) for v in values))

    def prop_str(self, name, s):
        self.prop_bytes(name, s.encode() + b"\x00")

    def build(self):
        self.struct += struct.pack(">I", 9)  # END
        off_struct = 56
        off_strings = off_struct + len(self.struct)
        total = off_strings + len(self.strings)
        header = struct.pack(
            ">10I",
            0xD00DFEED,      # magic
            total,
            off_struct,
            off_strings,
            40,              # off_mem_rsvmap
            17,              # version
            16,              # last_comp_version
            0,               # boot_cpuid
            len(self.strings),
            len(self.struct),
        )
        rsv = struct.pack(">QQ", 0, 0)  # terminator
        return header + rsv + bytes(self.struct) + bytes(self.strings)


d = DTB()

d.begin("")  # /
d.prop_u32("#address-cells", [2])
d.prop_u32("#size-cells", [2])
d.prop_str("compatible", "qcom,blair")
d.prop_str("model", "Fake SM6375 (veux) for QEMU virt")

# ---- RAM 512M @ 0x40000000 (QEMU patches this too) ----
d.begin("memory@40000000")
d.prop_str("device_type", "memory")
d.prop_u32("reg", [0, 0x40000000, 0, 0x20000000])
d.end()

# ---- chosen (QEMU допишет initrd-start/end и bootargs) ----
d.begin("chosen")
d.end()

# ---- PSCI (как на телефоне: smc; для QEMU — hvc) ----
d.begin("psci")
d.prop_str("compatible", "arm,psci-1.0")
d.prop_bytes("compatible", b"arm,psci-0.2\x00arm,psci-1.0\x00")
d.prop_str("method", "hvc")
d.prop_u32("cpu_on", [0xC4000003])
d.end()

# ---- CPUs ----
d.begin("cpus")
d.prop_u32("#address-cells", [1])
d.prop_u32("#size-cells", [0])
for i in range(4):
    d.begin(f"cpu@{i}")
    d.prop_str("device_type", "cpu")
    d.prop_str("compatible", "arm,cortex-a55")
    d.prop_u32("reg", [i])
    d.prop_str("enable-method", "psci")
    d.end()
d.end()

# ---- arch timer (INTID 30 = PPI 14) ----
d.begin("timer")
d.prop_str("compatible", "arm,armv8-timer")
d.prop_u32("interrupts", [1, 13, 0x104, 1, 14, 0x104, 1, 11, 0x104, 1, 10, 0x104])
d.end()

# ---- GICv3 @ QEMU virt ----
d.begin("intc@8000000")
d.prop_str("compatible", "arm,gic-v3")
d.prop_u32("reg", [0, 0x08000000, 0, 0x10000, 0, 0x080A0000, 0, 0x20000])
d.prop_u32("#interrupt-cells", [3])
d.prop_bytes("interrupt-controller", b"")
d.prop_u32("phandle", [1])
d.end()

# ---- reserved-memory: fake no-map (проверка исключения из PMM/MMU) ----
d.begin("reserved-memory")
d.prop_u32("#address-cells", [2])
d.prop_u32("#size-cells", [2])
d.prop_bytes("ranges", b"")
d.begin("fake_nomap@50000000")
d.prop_u32("reg", [0, 0x50000000, 0, 0x1000000])  # 16 MiB
d.prop_bytes("no-map", b"")
d.end()
# Shadow для телефонных MMIO: зарезервировано БЕЗ no-map. PMM не выдаёт эти
# страницы, MMU мапит как Normal -> обращения драйверов не абортят, а пишут
# в RAM-тень (читается 0 = "устройство неактивно", как нужно поллингу).
d.begin("mmio_shadow@4f000000")
d.prop_u32("reg", [0, 0x4F000000, 0, 0x1000000])  # 16 MiB
d.end()
d.end()

# ---- soc: телефонная форма (#address-cells=1!) ----
d.begin("soc")
d.prop_str("compatible", "simple-bus")
d.prop_u32("#address-cells", [1])
d.prop_u32("#size-cells", [1])
d.prop_bytes("ranges", b"")

# PL011 — видимый вывод на QEMU (dual-sink: PL011 + GENI одновременно)
d.begin("uart@9000000")
d.prop_str("compatible", "arm,pl011")
d.prop_u32("reg", [0x09000000, 0x1000])
d.prop_str("status", "ok")
d.end()

# Консольный GENI UART (форма как в DTB Poco X4 Pro, адрес — shadow)
d.begin("qup_uart@4c8c000")
d.prop_str("compatible", "qcom,msm-geni-console")
d.prop_u32("reg", [0x4F000000, 0x4000])
d.prop_str("status", "ok")
d.prop_u32("interrupt-parent", [1])
d.prop_u32("interrupts", [0, 510, 4])
d.end()

# APSS watchdog
d.begin("wdt@f410000")
d.prop_str("compatible", "qcom,msm-watchdog")
d.prop_u32("reg", [0x4F010000, 0x1000])
d.end()

# GCC (Blair)
d.begin("clock-controller@1400000")
d.prop_str("compatible", "qcom,blair-gccsyscon")
d.prop_u32("reg", [0x4F020000, 0x1F0000])
d.prop_str("status", "ok")
d.end()

# SPMI + PMIC
d.begin("spmi@1c40000")
d.prop_str("compatible", "qcom,spmi-pmic-arb")
d.prop_u32("reg", [0x4F030000, 0x1000])
d.prop_str("status", "ok")
d.begin("pmic@0")
d.prop_str("compatible", "qcom,pmi8998")
d.prop_u32("reg", [0])
d.end()
d.end()

# UFS HC + PHY (включены, как после patch_dtb.py)
d.begin("ufshc@4804000")
d.prop_str("compatible", "qcom,ufshc")
d.prop_u32("reg", [0x4F040000, 0x3000, 0x4F050000, 0x8000])
d.prop_str("status", "ok")
d.end()

d.begin("ufsphy_mem@4807000")
d.prop_str("compatible", "qcom,ufs-phy-qmp-v4")
d.prop_u32("reg", [0x4F060000, 0xDDC])
d.prop_str("status", "ok")
d.end()

d.end()  # soc
d.end()  # /

with open("fake_sm6375.dtb", "wb") as f:
    f.write(d.build())
print(f"fake_sm6375.dtb written ({len(d.build())} bytes)")
