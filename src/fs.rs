//! Minimal FAT32 filesystem on top of virtio-blk.
//! Supports: ls, read, write/create/truncate, mkdir, rm (files/empty dirs), stat.
//! Limitations: 8.3 names only, no long filenames, single mounted volume, small fixed buffers.
use crate::{
    sync::SpinLock,
    virtio_blk::{self, SECTOR_SIZE},
};

const MAX_PATH: usize = 128;
const MAX_NAME: usize = 12; // 8.3 + null
const MAX_ENTRIES: usize = 64;
const CLUSTER_BUF_SECTORS: usize = 8;
const MAX_FILE_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy)]
pub struct FsEntry {
    pub name: [u8; MAX_NAME],
    pub size: u32,
    pub is_dir: bool,
    pub cluster: u32,
}

impl FsEntry {
    const fn empty() -> Self {
        Self { name: [0; MAX_NAME], size: 0, is_dir: false, cluster: 0 }
    }
    pub fn name_str(&self) -> &str {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(MAX_NAME);
        core::str::from_utf8(&self.name[..n]).unwrap_or("?")
    }
}

struct Fat32 {
    mounted: bool,
    bytes_per_sector: u32,
    sectors_per_cluster: u32,
    reserved_sectors: u32,
    fat_count: u32,
    sectors_per_fat: u32,
    root_cluster: u32,
    fat_start: u32,
    data_start: u32,
    total_sectors: u32,
}

impl Fat32 {
    const fn new() -> Self {
        Self {
            mounted: false, bytes_per_sector: 512, sectors_per_cluster: 1, reserved_sectors: 0,
            fat_count: 2, sectors_per_fat: 0, root_cluster: 2, fat_start: 0, data_start: 0, total_sectors: 0,
        }
    }
}

static FS: SpinLock<Fat32> = SpinLock::new(Fat32::new());

fn read_sector(sec: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), ()> {
    virtio_blk::read_sector(sec as u64, buf)
}
fn write_sector(sec: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), ()> {
    virtio_blk::write_sector(sec as u64, buf)
}

fn le16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]) }
fn set_le16(b: &mut [u8], o: usize, v: u16) { let x=v.to_le_bytes(); b[o]=x[0]; b[o+1]=x[1]; }
fn set_le32(b: &mut [u8], o: usize, v: u32) { let x=v.to_le_bytes(); b[o]=x[0]; b[o+1]=x[1]; b[o+2]=x[2]; b[o+3]=x[3]; }

pub fn init() {
    if !virtio_blk::is_ready() {
        crate::kprintln!("FS   no block device, FAT32 not mounted");
        return;
    }
    let mut sec = [0u8; SECTOR_SIZE];
    if read_sector(0, &mut sec).is_err() { crate::kprintln!("FS   cannot read sector 0"); return; }
    let mut bpb_sec = 0u32;
    // MBR?
    if sec[510] == 0x55 && sec[511] == 0xAA && sec[0x1BE + 4] != 0 {
        let part_type = sec[0x1BE + 4];
        let lba = le32(&sec, 0x1BE + 8);
        if matches!(part_type, 0x0B | 0x0C | 0x06 | 0x04 | 0x0E) && lba > 0 {
            bpb_sec = lba;
            if read_sector(bpb_sec, &mut sec).is_err() { crate::kprintln!("FS   cannot read BPB"); return; }
        }
    }
    if sec[510] != 0x55 || sec[511] != 0xAA {
        crate::kprintln!("FS   no FAT32 signature"); return;
    }
    let bps = le16(&sec, 11) as u32;
    let spc = sec[13] as u32;
    let reserved = le16(&sec, 14) as u32;
    let fats = sec[16] as u32;
    let spf16 = le16(&sec, 22) as u32;
    let spf32 = le32(&sec, 36);
    let spf = if spf16 != 0 { spf16 } else { spf32 };
    let total16 = le16(&sec, 19) as u32;
    let total32 = le32(&sec, 32);
    let total = if total16 != 0 { total16 } else { total32 };
    let root = le32(&sec, 44);
    if bps != 512 || spc == 0 || spc > 64 || fats == 0 || spf == 0 {
        crate::kprintln!("FS   unsupported FAT parameters bps={} spc={} fats={} spf={}", bps, spc, fats, spf);
        return;
    }
    let fat_start = bpb_sec + reserved;
    let data_start = fat_start + fats * spf;
    let mut fs = FS.lock();
    *fs = Fat32 {
        mounted: true, bytes_per_sector: bps, sectors_per_cluster: spc, reserved_sectors: reserved,
        fat_count: fats, sectors_per_fat: spf, root_cluster: if root == 0 { 2 } else { root },
        fat_start, data_start, total_sectors: total,
    };
    crate::kprintln!("FS   FAT32 mounted: spc={} spf={} root={} data={}", spc, spf, fs.root_cluster, data_start);
}

pub fn is_mounted() -> bool { FS.lock().mounted }

fn cluster_to_sector(fs: &Fat32, cluster: u32) -> u32 {
    fs.data_start + (cluster.saturating_sub(2)) * fs.sectors_per_cluster
}

fn read_fat(fs: &Fat32, cluster: u32) -> Result<u32, ()> {
    let off = cluster * 4;
    let sec = fs.fat_start + off / fs.bytes_per_sector;
    let idx = (off % fs.bytes_per_sector) as usize;
    let mut buf = [0u8; SECTOR_SIZE];
    read_sector(sec, &mut buf)?;
    Ok(le32(&buf, idx) & 0x0FFF_FFFF)
}

fn write_fat(fs: &Fat32, cluster: u32, value: u32) -> Result<(), ()> {
    let off = cluster * 4;
    let sec_off = off / fs.bytes_per_sector;
    let idx = (off % fs.bytes_per_sector) as usize;
    for f in 0..fs.fat_count {
        let sec = fs.fat_start + f * fs.sectors_per_fat + sec_off;
        let mut buf = [0u8; SECTOR_SIZE];
        read_sector(sec, &mut buf)?;
        let old = le32(&buf, idx);
        set_le32(&mut buf, idx, (old & 0xF000_0000) | (value & 0x0FFF_FFFF));
        write_sector(sec, &buf)?;
    }
    Ok(())
}

fn next_cluster(fs: &Fat32, cluster: u32) -> Result<Option<u32>, ()> {
    let v = read_fat(fs, cluster)?;
    if v >= 0x0FFF_FFF8 || v == 0 { Ok(None) } else { Ok(Some(v)) }
}

fn alloc_cluster(fs: &Fat32) -> Result<u32, ()> {
    let total_data_secs = fs.total_sectors.saturating_sub(fs.data_start);
    let max_clusters = total_data_secs / fs.sectors_per_cluster + 2;
    for c in 2..max_clusters {
        let v = read_fat(fs, c)?;
        if v == 0 {
            write_fat(fs, c, 0x0FFF_FFFF)?;
            // zero cluster
            let start = cluster_to_sector(fs, c);
            let zero = [0u8; SECTOR_SIZE];
            for s in 0..fs.sectors_per_cluster { write_sector(start + s, &zero)?; }
            return Ok(c);
        }
    }
    Err(())
}

fn free_chain(fs: &Fat32, mut cluster: u32) -> Result<(), ()> {
    loop {
        if cluster < 2 || cluster >= 0x0FFF_FFF8 { break; }
        let next = read_fat(fs, cluster)?;
        write_fat(fs, cluster, 0)?;
        if next >= 0x0FFF_FFF8 || next == 0 { break; }
        cluster = next;
    }
    Ok(())
}

fn read_cluster(fs: &Fat32, cluster: u32, buf: &mut [u8]) -> Result<usize, ()> {
    let bytes = (fs.sectors_per_cluster * fs.bytes_per_sector) as usize;
    if buf.len() < bytes { return Err(()); }
    let start = cluster_to_sector(fs, cluster);
    for s in 0..fs.sectors_per_cluster {
        let mut sec = [0u8; SECTOR_SIZE];
        read_sector(start + s, &mut sec)?;
        let off = s as usize * SECTOR_SIZE;
        buf[off..off + SECTOR_SIZE].copy_from_slice(&sec);
    }
    Ok(bytes)
}

fn write_cluster(fs: &Fat32, cluster: u32, buf: &[u8]) -> Result<(), ()> {
    let bytes = (fs.sectors_per_cluster * fs.bytes_per_sector) as usize;
    if buf.len() < bytes { return Err(()); }
    let start = cluster_to_sector(fs, cluster);
    for s in 0..fs.sectors_per_cluster {
        let off = s as usize * SECTOR_SIZE;
        let mut sec = [0u8; SECTOR_SIZE];
        sec.copy_from_slice(&buf[off..off + SECTOR_SIZE]);
        write_sector(start + s, &sec)?;
    }
    Ok(())
}

fn name_to_83(name: &str, out: &mut [u8; 11]) -> bool {
    *out = [b' '; 11];
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." { return false; }
    let mut parts = name.splitn(2, '.');
    let base = parts.next().unwrap_or("");
    let ext = parts.next().unwrap_or("");
    if base.is_empty() || base.len() > 8 || ext.len() > 3 { return false; }
    for (i, b) in base.bytes().enumerate() {
        let c = b.to_ascii_uppercase();
        if !(c.is_ascii_alphanumeric() || c == b'_' || c == b'-') { return false; }
        out[i] = c;
    }
    for (i, b) in ext.bytes().enumerate() {
        let c = b.to_ascii_uppercase();
        if !(c.is_ascii_alphanumeric() || c == b'_' || c == b'-') { return false; }
        out[8 + i] = c;
    }
    true
}

fn name83_to_str(raw: &[u8; 11], out: &mut [u8; MAX_NAME]) -> usize {
    let mut n = 0usize;
    for i in 0..8 {
        let c = raw[i];
        if c == b' ' { break; }
        out[n] = c.to_ascii_lowercase();
        n += 1;
    }
    let mut has_ext = false;
    for i in 8..11 { if raw[i] != b' ' { has_ext = true; break; } }
    if has_ext {
        out[n] = b'.'; n += 1;
        for i in 8..11 {
            let c = raw[i];
            if c == b' ' { break; }
            out[n] = c.to_ascii_lowercase();
            n += 1;
        }
    }
    out[n] = 0;
    n
}

fn eq_name83(raw: &[u8; 11], name: &str) -> bool {
    let mut n83 = [b' '; 11];
    if !name_to_83(name, &mut n83) { return false; }
    &n83 == raw
}

fn dir_entry_name(e: &[u8]) -> [u8; 11] {
    let mut n = [b' '; 11];
    n.copy_from_slice(&e[0..11]);
    n
}

fn make_dir_entry(name83: &[u8; 11], attr: u8, cluster: u32, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[0..11].copy_from_slice(name83);
    e[11] = attr;
    set_le16(&mut e, 20, ((cluster >> 16) & 0xFFFF) as u16);
    set_le16(&mut e, 26, (cluster & 0xFFFF) as u16);
    set_le32(&mut e, 28, size);
    e
}

fn split_path(path: &str, parts: &mut [[u8; MAX_NAME]; 16]) -> usize {
    let mut n = 0;
    for p in path.split('/') {
        if p.is_empty() || p == "." { continue; }
        if n >= 16 { break; }
        let mut buf = [0u8; MAX_NAME];
        let bytes = p.as_bytes();
        let m = core::cmp::min(bytes.len(), MAX_NAME - 1);
        buf[..m].copy_from_slice(&bytes[..m]);
        parts[n] = buf;
        n += 1;
    }
    n
}

fn part_str(p: &[u8; MAX_NAME]) -> &str {
    let n = p.iter().position(|&c| c == 0).unwrap_or(MAX_NAME);
    core::str::from_utf8(&p[..n]).unwrap_or("")
}

fn find_in_dir(fs: &Fat32, dir_cluster: u32, name: &str) -> Result<Option<([u8;32], u32)>, ()> {
    let mut cl = dir_cluster;
    loop {
        let start = cluster_to_sector(fs, cl);
        for s in 0..fs.sectors_per_cluster {
            let mut sec = [0u8; SECTOR_SIZE];
            read_sector(start + s, &mut sec)?;
            for i in 0..(SECTOR_SIZE / 32) {
                let off = i * 32;
                let first = sec[off];
                if first == 0x00 { return Ok(None); }
                if first == 0xE5 { continue; }
                let attr = sec[off + 11];
                if attr == 0x0F { continue; }
                let raw = dir_entry_name(&sec[off..off+32]);
                if eq_name83(&raw, name) {
                    let mut e = [0u8; 32];
                    e.copy_from_slice(&sec[off..off+32]);
                    return Ok(Some((e, start + s)));
                }
            }
        }
        match next_cluster(fs, cl)? { Some(nc) => cl = nc, None => return Ok(None) }
    }
}

fn resolve_parent(fs: &Fat32, path: &str) -> Result<(u32, [u8; MAX_NAME]), ()> {
    let mut parts = [[0u8; MAX_NAME]; 16];
    let n = split_path(path, &mut parts);
    if n == 0 { return Err(()); }
    let mut dir = fs.root_cluster;
    for i in 0..n-1 {
        let name = part_str(&parts[i]);
        let Some((e,_)) = find_in_dir(fs, dir, name)? else { return Err(()); };
        if e[11] & 0x10 == 0 { return Err(()); }
        let cl = ((le16(&e,20) as u32) << 16) | le16(&e,26) as u32;
        dir = if cl == 0 { fs.root_cluster } else { cl };
    }
    Ok((dir, parts[n-1]))
}

fn resolve_path(fs: &Fat32, path: &str) -> Result<Option<([u8;32], u32)>, ()> {
    let mut parts = [[0u8; MAX_NAME]; 16];
    let n = split_path(path, &mut parts);
    if n == 0 {
        let mut e = [0u8;32]; e[11]=0x10;
        set_le16(&mut e,20, ((fs.root_cluster>>16)&0xFFFF) as u16);
        set_le16(&mut e,26, (fs.root_cluster&0xFFFF) as u16);
        return Ok(Some((e, 0)));
    }
    let mut dir = fs.root_cluster;
    for i in 0..n {
        let name = part_str(&parts[i]);
        let Some((e, sec_abs)) = find_in_dir(fs, dir, name)? else { return Ok(None); };
        if i == n-1 { return Ok(Some((e, sec_abs))); }
        if e[11] & 0x10 == 0 { return Ok(None); }
        let cl = ((le16(&e,20) as u32) << 16) | le16(&e,26) as u32;
        dir = if cl == 0 { fs.root_cluster } else { cl };
    }
    Ok(None)
}

pub fn list_dir(path: &str, out: &mut [FsEntry; MAX_ENTRIES]) -> Result<usize, ()> {
    let fs = FS.lock();
    if !fs.mounted { return Err(()); }
    let dir_cluster = if path.is_empty() || path == "/" {
        fs.root_cluster
    } else {
        match resolve_path(&fs, path)? {
            Some((e,_)) if e[11] & 0x10 != 0 => {
                let cl = ((le16(&e,20) as u32)<<16) | le16(&e,26) as u32;
                if cl == 0 { fs.root_cluster } else { cl }
            }
            _ => return Err(())
        }
    };
    let mut count = 0usize;
    let mut cl = dir_cluster;
    loop {
        let start = cluster_to_sector(&fs, cl);
        for s in 0..fs.sectors_per_cluster {
            let mut sec = [0u8; SECTOR_SIZE];
            read_sector(start + s, &mut sec)?;
            for i in 0..(SECTOR_SIZE/32) {
                let off = i*32;
                let first = sec[off];
                if first == 0x00 { return Ok(count); }
                if first == 0xE5 { continue; }
                let attr = sec[off+11];
                if attr == 0x0F { continue; }
                if sec[off] == b'.' { continue; }
                if count >= MAX_ENTRIES { return Ok(count); }
                let mut raw = [b' ';11]; raw.copy_from_slice(&sec[off..off+11]);
                let mut ent = FsEntry::empty();
                name83_to_str(&raw, &mut ent.name);
                ent.size = le32(&sec, off+28);
                ent.is_dir = attr & 0x10 != 0;
                ent.cluster = ((le16(&sec,off+20) as u32)<<16) | le16(&sec,off+26) as u32;
                out[count] = ent;
                count += 1;
            }
        }
        match next_cluster(&fs, cl)? { Some(nc) => cl = nc, None => break }
    }
    Ok(count)
}

pub fn read_file(path: &str, out: &mut [u8]) -> Result<usize, ()> {
    let fs = FS.lock();
    if !fs.mounted { return Err(()); }
    let Some((e,_)) = resolve_path(&fs, path)? else { return Err(()); };
    if e[11] & 0x10 != 0 { return Err(()); }
    let size = le32(&e,28) as usize;
    let mut cl = ((le16(&e,20) as u32)<<16) | le16(&e,26) as u32;
    let to_read = core::cmp::min(size, out.len());
    let mut done = 0usize;
    if cl < 2 { return Ok(0); }
    while done < to_read && cl >= 2 && cl < 0x0FFF_FFF8 {
        let start = cluster_to_sector(&fs, cl);
        for s in 0..fs.sectors_per_cluster {
            if done >= to_read { break; }
            let mut sec = [0u8; SECTOR_SIZE];
            read_sector(start+s, &mut sec)?;
            let n = core::cmp::min(SECTOR_SIZE, to_read - done);
            out[done..done+n].copy_from_slice(&sec[..n]);
            done += n;
        }
        match next_cluster(&fs, cl)? { Some(nc)=>cl=nc, None=>break }
    }
    Ok(done)
}

fn find_free_dir_slot(fs: &Fat32, dir_cluster: u32) -> Result<(u32, u32, usize), ()> {
    let mut cl = dir_cluster;
    loop {
        let start = cluster_to_sector(fs, cl);
        for s in 0..fs.sectors_per_cluster {
            let mut sec = [0u8; SECTOR_SIZE];
            read_sector(start+s, &mut sec)?;
            for i in 0..(SECTOR_SIZE/32) {
                let first = sec[i*32];
                if first == 0x00 || first == 0xE5 { return Ok((cl, s, i)); }
            }
        }
        match next_cluster(fs, cl)? {
            Some(nc) => cl = nc,
            None => {
                let new_cl = alloc_cluster(fs)?;
                write_fat(fs, cl, new_cl)?;
                return Ok((new_cl, 0, 0));
            }
        }
    }
}

pub fn write_file(path: &str, data: &[u8]) -> Result<(), ()> {
    let fs = FS.lock();
    if !fs.mounted { return Err(()); }
    let (dir, leaf) = resolve_parent(&fs, path)?;
    let leaf_name = part_str(&leaf);
    let mut name83 = [b' ';11];
    if !name_to_83(leaf_name, &mut name83) { return Err(()); }

    if let Some((e, sec_abs)) = find_in_dir(&fs, dir, leaf_name)? {
        if e[11] & 0x10 != 0 { return Err(()); }
        let old_cl = ((le16(&e,20) as u32)<<16) | le16(&e,26) as u32;
        if old_cl >= 2 { free_chain(&fs, old_cl)?; }
        let mut sec = [0u8; SECTOR_SIZE];
        read_sector(sec_abs, &mut sec)?;
        for i in 0..(SECTOR_SIZE/32) {
            let off=i*32;
            if sec[off]==0 { break; }
            if sec[off]==0xE5 { continue; }
            if sec[off+11]==0x0F { continue; }
            let raw = dir_entry_name(&sec[off..off+32]);
            if eq_name83(&raw, leaf_name) { sec[off]=0xE5; write_sector(sec_abs,&sec)?; break; }
        }
    }

    let mut first_cl = 0u32;
    let mut prev_cl = 0u32;
    let mut written = 0usize;
    let cluster_bytes = (fs.sectors_per_cluster * fs.bytes_per_sector) as usize;
    if !data.is_empty() {
        let mut remaining = data.len();
        loop {
            let cl = alloc_cluster(&fs)?;
            if first_cl == 0 { first_cl = cl; }
            if prev_cl >= 2 { write_fat(&fs, prev_cl, cl)?; }
            write_fat(&fs, cl, 0x0FFF_FFFF)?;
            let mut cbuf = [0u8; 4096];
            let cb = core::cmp::min(cluster_bytes, cbuf.len());
            let take = core::cmp::min(cb, data.len().saturating_sub(written));
            if take > 0 { cbuf[..take].copy_from_slice(&data[written..written+take]); }
            write_cluster(&fs, cl, &cbuf[..cb])?;
            written += take;
            prev_cl = cl;
            if remaining <= take { break; }
            remaining -= take;
            if written >= data.len() { break; }
        }
    }

    let (cl, s, i) = find_free_dir_slot(&fs, dir)?;
    let start = cluster_to_sector(&fs, cl);
    let mut sec = [0u8; SECTOR_SIZE];
    read_sector(start+s, &mut sec)?;
    let e = make_dir_entry(&name83, 0x20, first_cl, data.len() as u32);
    let off = i*32;
    sec[off..off+32].copy_from_slice(&e);
    write_sector(start+s, &sec)?;
    Ok(())
}

pub fn mkdir(path: &str) -> Result<(), ()> {
    let fs = FS.lock();
    if !fs.mounted { return Err(()); }
    let (dir, leaf) = resolve_parent(&fs, path)?;
    let leaf_name = part_str(&leaf);
    if find_in_dir(&fs, dir, leaf_name)?.is_some() { return Err(()); }
    let mut name83 = [b' ';11];
    if !name_to_83(leaf_name, &mut name83) { return Err(()); }
    let new_cl = alloc_cluster(&fs)?;
    let cluster_bytes = (fs.sectors_per_cluster * fs.bytes_per_sector) as usize;
    let mut cbuf = [0u8; 4096];
    let cb = core::cmp::min(cluster_bytes, cbuf.len());
    let mut dotn=[b' ';11]; dotn[0]=b'.';
    let mut dotdotn=[b' ';11]; dotdotn[0]=b'.'; dotdotn[1]=b'.';
    let dot = make_dir_entry(&dotn, 0x10, new_cl, 0);
    let parent_cl = if dir == fs.root_cluster { 0 } else { dir };
    let dotdot = make_dir_entry(&dotdotn, 0x10, parent_cl, 0);
    cbuf[0..32].copy_from_slice(&dot);
    cbuf[32..64].copy_from_slice(&dotdot);
    write_cluster(&fs, new_cl, &cbuf[..cb])?;

    let (dcl, s, i) = find_free_dir_slot(&fs, dir)?;
    let start = cluster_to_sector(&fs, dcl);
    let mut sec = [0u8; SECTOR_SIZE];
    read_sector(start+s, &mut sec)?;
    let e = make_dir_entry(&name83, 0x10, new_cl, 0);
    sec[i*32..i*32+32].copy_from_slice(&e);
    write_sector(start+s, &sec)?;
    Ok(())
}

pub fn unlink(path: &str) -> Result<(), ()> {
    let fs = FS.lock();
    if !fs.mounted { crate::kprintln!("DBG unlink: not mounted"); return Err(()); }
    let (dir, leaf) = match resolve_parent(&fs, path) { Ok(v) => v, Err(_) => { crate::kprintln!("DBG unlink: resolve_parent err"); return Err(()); } };
    let leaf_name = part_str(&leaf);
    let (e, sec_abs) = match find_in_dir(&fs, dir, leaf_name) {
        Ok(Some(v)) => v,
        Ok(None) => { crate::kprintln!("DBG unlink: not found '{}'", leaf_name); return Err(()); }
        Err(_) => { crate::kprintln!("DBG unlink: find_in_dir err"); return Err(()); }
    };
    crate::kprintln!("DBG unlink: found '{}' sec_abs={} attr={:#x}", leaf_name, sec_abs, e[11]);
    if e[11] & 0x10 != 0 {
        let cl = ((le16(&e,20) as u32)<<16) | le16(&e,26) as u32;
        let start_cl = if cl==0 { fs.root_cluster } else { cl };
        let mut c = start_cl;
        loop {
            let st = cluster_to_sector(&fs, c);
            for s in 0..fs.sectors_per_cluster {
                let mut sec=[0u8;SECTOR_SIZE];
                read_sector(st+s,&mut sec)?;
                for i in 0..(SECTOR_SIZE/32) {
                    let off=i*32; let first=sec[off];
                    if first==0 { break; }
                    if first==0xE5 { continue; }
                    if sec[off+11]==0x0F { continue; }
                    if sec[off]==b'.' { continue; }
                    return Err(());
                }
            }
            match next_cluster(&fs, c)? { Some(nc)=>c=nc, None=>break }
        }
        if cl >= 2 { free_chain(&fs, cl)?; }
    } else {
        let cl = ((le16(&e,20) as u32)<<16) | le16(&e,26) as u32;
        if cl >= 2 { free_chain(&fs, cl)?; }
    }
    let mut sec=[0u8;SECTOR_SIZE];
    if read_sector(sec_abs, &mut sec).is_err() { crate::kprintln!("DBG unlink: re-read sec_abs err"); return Err(()); }
    for i in 0..(SECTOR_SIZE/32) {
        let off=i*32;
        if sec[off]==0 { break; }
        if sec[off]==0xE5 { continue; }
        if sec[off+11]==0x0F { continue; }
        let raw=dir_entry_name(&sec[off..off+32]);
        if eq_name83(&raw, leaf_name) {
            sec[off]=0xE5;
            if write_sector(sec_abs,&sec).is_err() { crate::kprintln!("DBG unlink: write_sector err sec_abs={}", sec_abs); return Err(()); }
            break;
        }
    }
    Ok(())
}

pub fn stat(path: &str) -> Result<FsEntry, ()> {
    let fs = FS.lock();
    if !fs.mounted { return Err(()); }
    if path.is_empty() || path=="/" {
        let mut e=FsEntry::empty(); e.name[0]=b'/'; e.is_dir=true; e.cluster=fs.root_cluster; return Ok(e);
    }
    let Some((e,_s)) = resolve_path(&fs, path)? else { return Err(()); };
    let mut out=FsEntry::empty();
    let mut raw=[b' ';11]; raw.copy_from_slice(&e[0..11]);
    name83_to_str(&raw, &mut out.name);
    out.size=le32(&e,28);
    out.is_dir=e[11]&0x10!=0;
    out.cluster=((le16(&e,20) as u32)<<16)|le16(&e,26) as u32;
    Ok(out)
}

pub fn rename(old: &str, new: &str) -> Result<(), ()> {
    let mut data = [0u8; MAX_FILE_SIZE];
    let n = read_file(old, &mut data)?;
    write_file(new, &data[..n])?;
    unlink(old)
}
