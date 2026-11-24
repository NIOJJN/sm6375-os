"""Патчер DTB внутри vendor_boot.img для Poco X4 Pro (veux / SM6375).

Включает узлы, нужные rustos (status "disabled" -> "ok"), БЕЗ изменения размера
DTB (пропатченные значения дополняются NUL до исходной длины), поэтому образ
остаётся валидным, а DTB можно записать обратно по тому же смещению.

Включает:
  - /soc/ufshc@4804000      (qcom,ufshc)      — UFS host controller
  - /soc/ufsphy_mem@4807000 (qcom,ufs-phy-*)  — UFS PHY

Использование:
  python3 patch_dtb.py            # читает vendor_boot.img, пишет vendor_boot_patched.img
"""
import struct

SRC = "src/drivers/img/vendor_boot.img"
DST = "vendor_boot_patched.img"
DTB_OFFSET = 3141632  # смещение DTB внутри vendor_boot (найдено ранее)
DTB_SIZE = 324539

ENABLE_NODES = ["ufshc@4804000", "ufsphy_mem@4807000"]


def main():
    with open(SRC, "rb") as f:
        vb = bytearray(f.read())

    dtb = bytearray(vb[DTB_OFFSET:DTB_OFFSET + DTB_SIZE])
    if struct.unpack(">I", dtb[:4])[0] != 0xD00DFEED:
        raise SystemExit("DTB magic not found at offset")

    off_struct = struct.unpack(">I", dtb[8:12])[0]
    off_strings = struct.unpack(">I", dtb[12:16])[0]
    size_struct = struct.unpack(">I", dtb[36:40])[0]

    def get_string(idx):
        end = off_strings + idx
        while dtb[end] != 0:
            end += 1
        return bytes(dtb[off_strings + idx:end])

    pos = off_struct
    end = off_struct + size_struct
    path = []
    patched = []

    while pos + 4 <= end:
        token = struct.unpack(">I", dtb[pos:pos + 4])[0]
        pos += 4
        if token == 0x1:  # BEGIN_NODE
            e = pos
            while dtb[e] != 0:
                e += 1
            path.append(dtb[pos:e].decode("ascii", errors="replace"))
            pos = (e + 1 + 3) & ~3
        elif token == 0x2:  # END_NODE
            path.pop()
        elif token == 0x3:  # PROP
            prop_len = struct.unpack(">I", dtb[pos:pos + 4])[0]
            nameoff = struct.unpack(">I", dtb[pos + 4:pos + 8])[0]
            pos += 8
            name = get_string(nameoff)
            val_off = pos
            pos = (pos + prop_len + 3) & ~3

            node = path[-1] if path else ""
            if name == b"status" and any(node.endswith(n) for n in ENABLE_NODES):
                cur = bytes(dtb[val_off:val_off + prop_len]).rstrip(b"\x00")
                if cur == b"disabled":
                    new = b"ok" + b"\x00" * (prop_len - 2)  # та же длина!
                    dtb[val_off:val_off + prop_len] = new
                    patched.append("/".join(path))

    if not patched:
        print("Nothing patched (nodes already enabled or not found)")

    vb[DTB_OFFSET:DTB_OFFSET + DTB_SIZE] = dtb
    with open(DST, "wb") as f:
        f.write(vb)

    print(f"Patched nodes: {patched}")
    print(f"Written {DST} ({len(vb)} bytes)")
    print("Прошивка: fastboot flash vendor_boot vendor_boot_patched.img")


if __name__ == "__main__":
    main()
