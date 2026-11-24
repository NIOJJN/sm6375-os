//! Системные вызовы ядра (svc #0): номер в x8, аргументы x0..x4, результат x0 (0 = успех), данные x1..x5.
use core::arch::asm;
use core::fmt::{self, Write};
use core::sync::atomic::AtomicU32;

pub const USER_BASE: usize = 0x40_0000_0000;
/// Окно разделяемой памяти (должно совпадать с syscall.rs ядра).
pub const SHM_BASE: usize = USER_BASE + 0x10_0000;
/// Окно анонимной памяти (куча, стеки потоков).
pub const HEAP_BASE: usize = USER_BASE + 0x20_0000;

pub const INF: u64 = u64::MAX; // таймаут: ждать бесконечно (0 — не ждать)
pub const NO_CAP: u64 = u64::MAX;

// Коды ошибок ядра (статус в x0)
pub const ERR_INVAL: u64 = u64::MAX;
pub const ERR_DEAD: u64 = u64::MAX - 1;
pub const ERR_NOREPLY: u64 = u64::MAX - 2;
pub const ERR_BUSY: u64 = u64::MAX - 3;
pub const ERR_TIMEOUT: u64 = u64::MAX - 4;
pub const ERR_CANCELED: u64 = u64::MAX - 5;
pub const ERR_AGAIN: u64 = u64::MAX - 6;
pub const ERR_NOMEM: u64 = u64::MAX - 7;

const SYS_YIELD: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
const SYS_GETPID: u64 = 3;
const SYS_SLEEP: u64 = 4;
const SYS_TICKS: u64 = 5;
const SYS_CALL: u64 = 6;
const SYS_RECV: u64 = 7;
const SYS_REPLY: u64 = 8;
const SYS_SHM_MAP: u64 = 9;
const SYS_SHM_CREATE: u64 = 12;
const SYS_EP_CREATE: u64 = 13;
const SYS_MAP_ANON: u64 = 14;
const SYS_UNMAP: u64 = 15;
const SYS_THREAD_CREATE: u64 = 16;
const SYS_THREAD_EXIT: u64 = 17;
const SYS_GETTID: u64 = 18;
const SYS_FUTEX_WAIT: u64 = 19;
const SYS_FUTEX_WAKE: u64 = 20;
const SYS_CANCEL: u64 = 21;
const SYS_SET_PRIO: u64 = 22;
const SYS_CAP_DROP: u64 = 23;
const SYS_READ: u64 = 28;
// Legacy FS-номера оставлены для shell (ядро их больше не реализует — initramfs read-only).
const SYS_FS_LIST: u64 = 29;
const SYS_FS_READ: u64 = 30;
const SYS_FS_WRITE: u64 = 31;
const SYS_FS_MKDIR: u64 = 32;
const SYS_FS_UNLINK: u64 = 33;
const SYS_FS_STAT: u64 = 34;
const SYS_FS_RENAME: u64 = 36;
const SYS_SPAWN: u64 = 37;
const SYS_WAIT: u64 = 38;
const SYS_MEM_STATS: u64 = 39;
const SYS_KSTATS: u64 = 40;

#[inline(always)]
fn svc(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> [u64; 6] {
    let mut x0 = a0;
    let mut x1 = a1;
    let mut x2 = a2;
    let mut x3 = a3;
    let mut x4 = a4;
    let x5: u64;
    unsafe {
        asm!(
            "svc #0",
            in("x8") nr,
            inout("x0") x0,
            inout("x1") x1,
            inout("x2") x2,
            inout("x3") x3,
            inout("x4") x4,
            out("x5") x5,
            options(nostack),
        );
    }
    [x0, x1, x2, x3, x4, x5]
}

pub fn write_raw(b: &[u8]) {
    let mut off = 0;
    while off < b.len() {
        let n = core::cmp::min(b.len() - off, 200);
        svc(SYS_WRITE, b.as_ptr() as u64 + off as u64, n as u64, 0, 0, 0);
        off += n;
    }
}

struct Out {
    buf: [u8; 160],
    len: usize,
}

impl Out {
    fn flush(&mut self) {
        if self.len > 0 {
            write_raw(&self.buf[..self.len]);
            self.len = 0;
        }
    }
}

impl Write for Out {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &c in s.as_bytes() {
            if self.len == self.buf.len() {
                self.flush();
            }
            self.buf[self.len] = c;
            self.len += 1;
        }
        Ok(())
    }
}

pub fn print_args(args: fmt::Arguments) {
    let mut o = Out { buf: [0; 160], len: 0 };
    let _ = o.write_fmt(args);
    o.flush();
}

#[macro_export]
macro_rules! uprintln {
    ($($arg:tt)*) => { $crate::sys::print_args(format_args!("{}\n", format_args!($($arg)*))) };
}

/// Форматирует в буфер и возвращает строку (для отрисовки текста).
pub fn fmt_into<'a>(buf: &'a mut [u8], args: fmt::Arguments) -> &'a str {
    struct W<'b> {
        buf: &'b mut [u8],
        len: usize,
    }
    impl Write for W<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let b = s.as_bytes();
            let n = core::cmp::min(b.len(), self.buf.len() - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
            self.len += n;
            Ok(())
        }
    }
    let mut w = W { buf: &mut *buf, len: 0 };
    let _ = w.write_fmt(args);
    let n = w.len;
    let b: &'a [u8] = buf;
    core::str::from_utf8(&b[..n]).unwrap_or("?")
}

pub fn exit(code: u64) -> ! {
    svc(SYS_EXIT, code, 0, 0, 0, 0);
    loop {}
}

pub fn sleep(ticks: u64) {
    svc(SYS_SLEEP, ticks, 0, 0, 0, 0);
}

/// Сообщение IPC (раскладка совпадает с UMsg ядра): 4 слова + буфер данных + capability.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Msg {
    pub w: [u64; 4],
    pub cap: u64, // отправка: handle передаваемой capability (нужен GRANT) или NO_CAP; приём: новый handle или NO_CAP
    pub buf: u64, // адрес буфера данных
    pub len: u64, // отправка: байт; приём: ёмкость буфера на входе, принято на выходе
    pub from: u64, // приём: pid отправителя
}

impl Msg {
    pub const fn new() -> Msg {
        Msg { w: [0; 4], cap: NO_CAP, buf: 0, len: 0, from: 0 }
    }
    /// Сообщение с данными из буфера.
    pub fn with_data(w: [u64; 4], data: &[u8]) -> Msg {
        Msg { w, cap: NO_CAP, buf: data.as_ptr() as u64, len: data.len() as u64, from: 0 }
    }
    /// Структура приёма с буфером заданной ёмкости.
    pub fn with_buf(buf: &mut [u8]) -> Msg {
        Msg { w: [0; 4], cap: NO_CAP, buf: buf.as_mut_ptr() as u64, len: buf.len() as u64, from: 0 }
    }
}

/// Синхронный вызов: отправить `send`, дождаться ответа в `recv`. Возвращает статус (0 — успех).
pub fn call(h: u64, send: &Msg, recv: &mut Msg, timeout: u64) -> u64 {
    svc(SYS_CALL, h, send as *const Msg as u64, recv as *mut Msg as u64, timeout, 0)[0]
}

/// Принять сообщение (timeout: 0 — не ждать, INF — бесконечно).
pub fn recv(h: u64, recv: &mut Msg, timeout: u64) -> u64 {
    svc(SYS_RECV, h, recv as *mut Msg as u64, timeout, 0, 0)[0]
}

/// Ответить на последний принятый запрос.
pub fn reply(send: &Msg) -> u64 {
    svc(SYS_REPLY, send as *const Msg as u64, 0, 0, 0, 0)[0]
}

pub fn yield_now() {
    svc(SYS_YIELD, 0, 0, 0, 0, 0);
}

pub fn ticks() -> u64 {
    svc(SYS_TICKS, 0, 0, 0, 0, 0)[0]
}

pub fn getpid() -> u64 {
    svc(SYS_GETPID, 0, 0, 0, 0, 0)[0]
}

pub fn gettid() -> u64 {
    svc(SYS_GETTID, 0, 0, 0, 0, 0)[0]
}

/// Создать объект разделяемой памяти; возвращает handle (права MAP и GRANT).
pub fn shm_create(pages: u64) -> Option<u64> {
    let r = svc(SYS_SHM_CREATE, pages, 0, 0, 0, 0);
    if r[0] == 0 {
        Some(r[1])
    } else {
        None
    }
}

/// Создать endpoint; возвращает handle (SEND, RECV и GRANT).
pub fn ep_create() -> Option<u64> {
    let r = svc(SYS_EP_CREATE, 0, 0, 0, 0, 0);
    if r[0] == 0 {
        Some(r[1])
    } else {
        None
    }
}

/// Отобразить `pages` анонимных обнулённых страниц по адресу `va` (в окне HEAP).
pub fn map_anon(va: usize, pages: usize) -> bool {
    svc(SYS_MAP_ANON, va as u64, pages as u64, 0, 0, 0)[0] == 0
}

/// Снять отображение, которое начинается с `va` (anon/shm); память возвращается системе.
pub fn unmap(va: usize) -> bool {
    svc(SYS_UNMAP, va as u64, 0, 0, 0, 0)[0] == 0
}

/// Новый поток процесса: стартует в `entry(arg)` со стеком, вершина которого `sp`. Возвращает tid.
pub fn thread_create(entry: extern "C" fn(u64) -> !, sp: usize, arg: u64) -> Option<u64> {
    let r = svc(SYS_THREAD_CREATE, entry as usize as u64, sp as u64, arg, 0, 0);
    if r[0] == 0 {
        Some(r[1])
    } else {
        None
    }
}

/// Завершить только этот поток (последний поток завершает процесс).
pub fn thread_exit(code: u64) -> ! {
    svc(SYS_THREAD_EXIT, code, 0, 0, 0, 0);
    loop {}
}

/// Futex: если *addr == expected — уснуть до futex_wake/таймаута; иначе ERR_AGAIN.
pub fn futex_wait(addr: &AtomicU32, expected: u32, timeout: u64) -> u64 {
    svc(SYS_FUTEX_WAIT, addr as *const AtomicU32 as u64, expected as u64, timeout, 0, 0)[0]
}

/// Разбудить до `n` потоков, ждущих `addr`. Возвращает, сколько разбужено.
pub fn futex_wake(addr: &AtomicU32, n: u64) -> u64 {
    svc(SYS_FUTEX_WAKE, addr as *const AtomicU32 as u64, n, 0, 0, 0)[1]
}

/// Прервать ожидание IPC/futex у потока своего процесса (тот получит ERR_CANCELED).
pub fn cancel(tid: u64) -> u64 {
    svc(SYS_CANCEL, tid, 0, 0, 0, 0)[0]
}

/// Приоритет потока 0..=7 (tid = u64::MAX — текущий). Верхняя граница задаётся ядром для процесса.
pub fn set_prio(tid: u64, prio: u64) -> u64 {
    svc(SYS_SET_PRIO, tid, prio, 0, 0, 0)[0]
}

pub fn cap_drop(h: u64) -> bool {
    svc(SYS_CAP_DROP, h, 0, 0, 0, 0)[0] == 0
}

pub fn shm_map(h: u64, va: usize) -> bool {
    svc(SYS_SHM_MAP, h, va as u64, 0, 0, 0)[0] == 0
}

pub fn read(buf: &mut [u8]) -> usize {
    let r = svc(SYS_READ, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0);
    if r[0] == u64::MAX { 0 } else { r[0] as usize }
}

/// Создать процесс из ELF в initramfs (`name`, например "shell" -> "bin/shell").
/// `max_prio` — верхняя граница приоритета потоков (0 — по умолчанию).
pub fn spawn(name: &str, arg0: u64, arg1: u64, max_prio: u64) -> Option<u64> {
    let r = svc(
        SYS_SPAWN,
        name.as_ptr() as u64,
        name.len() as u64,
        arg0,
        arg1,
        max_prio,
    );
    if r[0] == 0 { Some(r[1]) } else { None }
}

/// Дождаться завершения ребёнка и забрать код выхода (None — ошибка или таймаут).
pub fn wait(pid: u64, timeout: u64) -> Option<u64> {
    let r = svc(SYS_WAIT, pid, timeout, 0, 0, 0);
    if r[0] == 0 { Some(r[1]) } else { None }
}

/// (занятые страницы, свободные страницы).
pub fn mem_stats() -> (u64, u64) {
    let r = svc(SYS_MEM_STATS, 0, 0, 0, 0, 0);
    (r[0], r[1])
}

/// (IPC-вызовы, ответы, переключения контекста).
pub fn kstats() -> (u64, u64, u64) {
    let r = svc(SYS_KSTATS, 0, 0, 0, 0, 0);
    (r[0], r[1], r[2])
}

pub fn read_line(buf: &mut [u8]) -> usize {
    let mut n = 0usize;
    while n + 1 < buf.len() {
        let mut c = [0u8; 1];
        let got = read(&mut c);
        if got == 0 {
            sleep(1);
            continue;
        }
        let b = c[0];
        if b == b'\r' { continue; }
        if b == b'\n' {
            write_raw(b"\n");
            break;
        }
        if b == 8 || b == 127 {
            if n > 0 {
                n -= 1;
                write_raw(b"\x08 \x08");
            }
            continue;
        }
        if b >= 0x20 && b < 0x7f {
            buf[n] = b;
            n += 1;
            write_raw(&c);
        }
    }
    buf[n] = 0;
    n
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FsEntry {
    pub name: [u8; 12],
    pub size: u32,
    pub is_dir: u32,
}

pub fn fs_list(path: &str, out: &mut [FsEntry]) -> Option<usize> {
    let r = svc(SYS_FS_LIST, path.as_ptr() as u64, path.len() as u64, out.as_mut_ptr() as u64, out.len() as u64, 0);
    if r[0] == u64::MAX { None } else { Some(r[0] as usize) }
}

pub fn fs_read(path: &str, out: &mut [u8]) -> Option<usize> {
    let r = svc(SYS_FS_READ, path.as_ptr() as u64, path.len() as u64, out.as_mut_ptr() as u64, out.len() as u64, 0);
    if r[0] == u64::MAX { None } else { Some(r[0] as usize) }
}

pub fn fs_write(path: &str, data: &[u8]) -> bool {
    svc(SYS_FS_WRITE, path.as_ptr() as u64, path.len() as u64, data.as_ptr() as u64, data.len() as u64, 0)[0] == 0
}

pub fn fs_mkdir(path: &str) -> bool {
    svc(SYS_FS_MKDIR, path.as_ptr() as u64, path.len() as u64, 0, 0, 0)[0] == 0
}

pub fn fs_unlink(path: &str) -> bool {
    svc(SYS_FS_UNLINK, path.as_ptr() as u64, path.len() as u64, 0, 0, 0)[0] == 0
}

pub fn fs_stat(path: &str) -> Option<FsEntry> {
    let mut e = FsEntry { name: [0;12], size: 0, is_dir: 0 };
    let r = svc(SYS_FS_STAT, path.as_ptr() as u64, path.len() as u64, &mut e as *mut _ as u64, 0, 0);
    if r[0] == u64::MAX { None } else { Some(e) }
}

pub fn fs_rename(old: &str, new: &str) -> bool {
    svc(SYS_FS_RENAME, old.as_ptr() as u64, old.len() as u64, new.as_ptr() as u64, new.len() as u64, 0)[0] == 0
}

pub fn putc(b: u8) {
    let buf = [b];
    write_raw(&buf);
}

/// Отладочный вывод через SYS_WRITE (проходит через консоль), только для отладки.
pub fn dbg_print(s: &str) {
    let _ = print_args(format_args!("{}\n", s));
}
