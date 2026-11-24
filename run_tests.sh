#!/bin/bash
# Тест-раннер: обычный запуск + репетиция с fake DTB.
cd /mnt/d/TestingSuite/rustos

echo "=== kernel.bin / dtb ==="
ls -la kernel.bin fake_sm6375.dtb initramfs.cpio

echo "=== RUN 1: standard QEMU (virt DTB), 150s ==="
timeout 150 qemu-system-aarch64 -machine virt,gic-version=3 -cpu max -m 512M -smp 4 \
  -serial stdio -initrd initramfs.cpio -kernel kernel.bin > /tmp/q_std.log 2>&1
echo "qemu exit code: $?"
echo "--- head ---"
head -6 /tmp/q_std.log
echo "--- stage/probe ---"
grep -aE "stage 8|probed|gcc|spmi|pmic|ufs|panic|EXCEPTION" /tmp/q_std.log | head -12

echo "=== RUN 2: fake SM6375 DTB, 150s ==="
timeout 150 qemu-system-aarch64 -machine virt,gic-version=3 -cpu max -m 512M -smp 4 \
  -serial stdio -initrd initramfs.cpio -kernel kernel.bin -dtb fake_sm6375.dtb > /tmp/q_dtb.log 2>&1
echo "qemu exit code: $?"
echo "--- head ---"
head -22 /tmp/q_dtb.log
echo "--- stage/probe ---"
grep -aE "stage 8|probed|gcc|spmi|pmic|ufs|reserved|panic|EXCEPTION" /tmp/q_dtb.log | head -20
