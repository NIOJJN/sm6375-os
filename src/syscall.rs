//! Системные вызовы (svc #0 из EL0). Номер — в x8, аргументы — x0..x4, статус — в x0 (0 = успех, иначе код ошибки
//! из ipc.rs), дополнительные результаты — в x1... Сообщения IPC передаются структурой `UMsg` в памяти процесса.
//!
//! Capabilities теперь ссылаются на объекты единой таблицы obj::OBJS по obj_id.
//! Тип объекта проверяется через obj::obj_type_of (нельзя подделать kind).
use crate::{
    aspace::{AddressSpace, HEAP_BASE, HEAP_END, USER_BASE, USER_END},
    ipc::{self, Msg, ERR_AGAIN, ERR_CANCELED, ERR_INVAL, ERR_NOMEM, ERR_TIMEOUT},
    mmu::Mem,
    obj, pmm, port, proc, sched, shm, timer,
};
use core::arch::asm;
use proc::{MAP_ANON, MAP_SHM, RIGHT_GRANT, RIGHT_MAP, RIGHT_RECV, RIGHT_SEND};

/// Кадр, который сохраняет boot.S (SAVE_FRAME).
#[repr(C)]
pub struct TrapFrame {
    pub x: [u64; 18], // x0..x17
    pub x18: u64,
    pub x29: u64,
    pub x30: u64,
    pub elr: u64,
    pub spsr: u64,
    _pad: u64,
}

pub const SYS_YIELD: u64 = 0;
pub const SYS_WRITE: u64 = 1; // (ptr, len) -> len | err
pub const SYS_EXIT: u64 = 2; // (code): завершает ВЕСЬ процесс (все потоки)
pub const SYS_GETPID: u64 = 3; // -> x0 = pid
pub const SYS_SLEEP: u64 = 4; // (ticks)
pub const SYS_TICKS: u64 = 5; // -> x0 = тики
pub const SYS_CALL: u64 = 6; // (handle, &send UMsg, &recv UMsg, timeout)
pub const SYS_RECV: u64 = 7; // (handle, &recv UMsg, timeout)
pub const SYS_REPLY: u64 = 8; // (&send UMsg)
pub const SYS_SHM_MAP: u64 = 9; // (handle, va)
pub const SYS_SHM_CREATE: u64 = 12; // (pages) -> x1 = handle (MAP|GRANT)
pub const SYS_EP_CREATE: u64 = 13; // () -> x1 = handle (SEND|RECV|GRANT)
pub const SYS_MAP_ANON: u64 = 14; // (va, pages): анонимная память (куча, стеки потоков)
pub const SYS_UNMAP: u64 = 15; // (va): снять отображение, начинающееся с va (anon/shm)
pub const SYS_THREAD_CREATE: u64 = 16; // (entry, sp, arg) -> x1 = tid
pub const SYS_THREAD_EXIT: u64 = 17; // (code): завершает только этот поток
pub const SYS_GETTID: u64 = 18; // -> x0 = tid
pub const SYS_FUTEX_WAIT: u64 = 19; // (addr, expected, timeout)
pub const SYS_FUTEX_WAKE: u64 = 20; // (addr, n) -> x1 = разбужено
pub const SYS_CANCEL: u64 = 21; // (tid): прервать ожидание IPC/futex потока своего процесса
pub const SYS_SET_PRIO: u64 = 22; // (tid | u64::MAX = себя, prio)
pub const SYS_CAP_DROP: u64 = 23; // (handle)
// Асинхронные порты / нотификации:
pub const SYS_PORT_CREATE: u64 = 24; // () -> x1 = handle (RECV)
pub const SYS_PORT_SEND: u64 = 25; // (handle, &send UMsg, timeout)
pub const SYS_PORT_RECV: u64 = 26; // (handle, &recv UMsg, timeout)
pub const SYS_PORT_NOTIFY: u64 = 27; // (handle)
pub const SYS_READ: u64 = 28; // (ptr, len) -> bytes read
// Управление процессами из userspace (init как PID 1):
pub const SYS_SPAWN: u64 = 37; // (name_ptr, name_len, arg0, arg1, max_prio) -> x1 = pid
pub const SYS_WAIT: u64 = 38; // (pid, timeout) -> x1 = exit code (0 = успех, ERR_TIMEOUT)
pub const SYS_MEM_STATS: u64 = 39; // () -> x0 = allocated pages, x1 = free pages
pub const SYS_KSTATS: u64 = 40; // () -> x0 = ipc calls, x1 = replies, x2 = context switches

/// Окно для отображения разделяемой памяти в пользовательском адресном пространстве.
pub const SHM_BASE: usize = USER_BASE + 0x10_0000;
pub const SHM_END: usize = USER_BASE + 0x20_0000;

const ERR: u64 = ERR_INVAL;
const NO_CAP: u64 = u64::MAX;
const MAX_ANON_PAGES: usize = 256;

/// Сообщение в памяти процесса (64 байта).
#[repr(C)]
#[derive(Clone, Copy)]
struct UMsg {
    w: [u64; 4],
    cap: u64, // отправка: handle передаваемой capability или NO_CAP; приём: новый handle или NO_CAP
    buf: u64, // адрес буфера данных
    len: u64, // отправка: число байт; приём: ёмкость буфера на входе, принято байт на выходе
    from: u64, // приём: pid отправителя
}

fn asp_of(pid: usize) -> Option<AddressSpace> {
    proc::root_of(pid).map(|(root, asid)| AddressSpace { root, asid })
}

fn read_umsg(asp: &AddressSpace, va: usize) -> Option<UMsg> {
    let mut b = [0u8; 64];
    if !asp.copy_from_user(va, &mut b) {
        return None;
    }
    Some(unsafe { core::ptr::read_unaligned(b.as_ptr() as *const UMsg) })
}

fn write_umsg(asp: &AddressSpace, va: usize, m: &UMsg) -> bool {
    let b: [u8; 64] = unsafe { core::mem::transmute_copy(m) };
    asp.copy_to_user(va, &b)
}

/// Читает сообщение для отправки; capability фиксируется (для shm берётся ссылка).
fn load_send(pid: usize, asp: &AddressSpace, va: usize) -> Result<Msg, u64> {
    let u = read_umsg(asp, va).ok_or(ERR)?;
    let len = u.len as usize;
    if len > ipc::MAX_MSG_BYTES {
        return Err(ERR);
    }
    let mut m = Msg::EMPTY;
    m.w = u.w;
    m.len = len;
    if len > 0 && !asp.copy_from_user(u.buf as usize, &mut m.data[..len]) {
        return Err(ERR);
    }
    if u.cap != NO_CAP {
        let c = proc::cap_get(pid, u.cap as usize).ok_or(ERR)?;
        if c.rights & RIGHT_GRANT == 0 {
            return Err(ERR);
        }
        // Получатель не может передавать дальше: GRANT не копируется.
        let staged = proc::Cap { obj_id: c.obj_id, rights: c.rights & !RIGHT_GRANT };
        proc::retain_cap(staged);
        m.cap = Some(staged);
    }
    Ok(m)
}

/// Записывает полученное сообщение в структуру приёма; capability устанавливается в таблицу процесса.
fn store_recv(pid: usize, asp: &AddressSpace, va: usize, m: Msg, from: u64) -> u64 {
    let Some(mut u) = read_umsg(asp, va) else {
        if let Some(c) = m.cap {
            proc::release_cap(c);
        }
        return ERR;
    };
    let n = core::cmp::min(u.len as usize, m.len);
    if n > 0 && !asp.copy_to_user(u.buf as usize, &m.data[..n]) {
        if let Some(c) = m.cap {
            proc::release_cap(c);
        }
        return ERR;
    }
    u.w = m.w;
    u.len = n as u64;
    u.from = from;
    u.cap = NO_CAP;
    if let Some(c) = m.cap {
        match proc::cap_install_staged(pid, c) {
            Some(h) => u.cap = h as u64,
            None => proc::release_cap(c), // таблица заполнена: capability теряется, данные доставлены
        }
    }
    if write_umsg(asp, va, &u) {
        0
    } else {
        ERR
    }
}

fn sys_write(ptr: usize, len: usize) -> u64 {
    if len == 0 || len > 256 {
        return ERR;
    }
    let pid = sched::current_pid();
    let Some(asp) = asp_of(pid) else { return ERR };
    let mut buf = [0u8; 256];
    if !asp.copy_from_user(ptr, &mut buf[..len]) {
        return ERR;
    }
    match core::str::from_utf8(&buf[..len]) {
        Ok(s) => kprint!("[pid {}] {}", pid, s),
        Err(_) => kprint!("[pid {}] <non-utf8>\n", pid),
    }
    len as u64
}

/// Проверяет, что handle указывает на capability с нужным правом, и возвращает объект.
fn obj_cap(pid: usize, handle: u64, need: u8) -> Option<proc::Cap> {
    let c = proc::cap_get(pid, handle as usize)?;
    if c.rights & need == 0 {
        return None;
    }
    Some(c)
}

/// Проверяет, что capability — это endpoint с правом на отправку.
fn ep_cap(pid: usize, handle: u64, need: u8) -> Option<usize> {
    let c = obj_cap(pid, handle, need)?;
    if obj::obj_type_of(c.obj_id as usize) != Some(obj::ObjType::Endpoint) {
        return None;
    }
    Some(c.obj_id as usize)
}

fn sys_call(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(asp) = asp_of(pid) else { return ERR };
    let Some(obj_id) = ep_cap(pid, tf.x[0], RIGHT_SEND) else { return ERR };
    let send = match load_send(pid, &asp, tf.x[1] as usize) {
        Ok(m) => m,
        Err(e) => return e,
    };
    match ipc::call(obj_id, send, tf.x[3]) {
        Ok(r) => store_recv(pid, &asp, tf.x[2] as usize, r, 0),
        Err(e) => e,
    }
}

fn sys_recv(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(asp) = asp_of(pid) else { return ERR };
    let Some(obj_id) = ep_cap(pid, tf.x[0], RIGHT_RECV) else { return ERR };
    match ipc::recv(obj_id, tf.x[2]) {
        Ok((m, from)) => store_recv(pid, &asp, tf.x[1] as usize, m, from),
        Err(e) => e,
    }
}

fn sys_reply(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(asp) = asp_of(pid) else { return ERR };
    let m = match load_send(pid, &asp, tf.x[0] as usize) {
        Ok(m) => m,
        Err(e) => return e,
    };
    match ipc::reply(m) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Снимает отображения [va, va+pages) текущего процесса; owned-страницы возвращает в PMM.
fn unmap_range(asp: &mut AddressSpace, va: usize, pages: usize) {
    for i in 0..pages {
        if let Some((pa, owned)) = asp.unmap_page(va + i * 4096) {
            if owned {
                pmm::free_page(pa);
            }
        }
    }
    AddressSpace::flush_tlb();
}

fn sys_shm_map(tf: &mut TrapFrame, pid: usize) -> u64 {
    if pid >= 1 && pid <= 5 { kprintln!("[sys_shm_map] pid={} cap={}", pid, tf.x[0]); }
    let Some(cap) = proc::cap_get(pid, tf.x[0] as usize) else { return ERR };
    if cap.rights & RIGHT_MAP == 0 {
        return ERR;
    }
    if obj::obj_type_of(cap.obj_id as usize) != Some(obj::ObjType::Shm) {
        return ERR;
    }
    let va = tf.x[1] as usize;
    let Some((pages, n)) = shm::get(cap.obj_id as usize) else { return ERR };
    if va % 4096 != 0 || va < SHM_BASE || va + n * 4096 > SHM_END {
        return ERR;
    }
    let Some((root, asid)) = proc::root_of(pid) else { return ERR };
    let mut asp = AddressSpace { root, asid };
    for i in 0..n {
        if asp.translate(va + i * 4096).is_some() {
            unmap_range(&mut asp, va, i);
            return ERR; // место занято
        }
        if !asp.map_page(va + i * 4096, pages[i], Mem::UserData, false) {
            unmap_range(&mut asp, va, i);
            return ERR_NOMEM;
        }
    }
    if !proc::map_add(pid, MAP_SHM, cap.obj_id as usize, va, n) {
        unmap_range(&mut asp, va, n);
        return ERR_NOMEM;
    }
    if pid >= 1 && pid <= 5 { kprintln!("[sys_shm_map] done pid={} va={:#x} n={}", pid, va, n); }
    // obj::obj_acquire уже вызван при retain_cap; здесь дополнительной ссылки не нужно.
    unsafe { asm!("dsb ishst", "isb", options(nostack)) };
    0
}

fn sys_shm_create(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(obj_id) = shm::create(pid, tf.x[0] as usize) else { return ERR_NOMEM };
    match proc::cap_install(pid, obj_id, RIGHT_MAP | RIGHT_GRANT) {
        Some(h) => {
            // Ссылка создателя (refs=1 из obj_alloc) переходит capability.
            let _ = obj::obj_release(obj_id);
            tf.x[1] = h as u64;
            0
        }
        None => {
            // Нет места в таблице capability: освобождаем объект.
            let _ = obj::obj_release(obj_id);
            ERR_NOMEM
        }
    }
}

fn sys_ep_create(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(obj_id) = ipc::create_endpoint(pid) else { return ERR_NOMEM };
    match proc::cap_install(pid, obj_id, RIGHT_SEND | RIGHT_RECV | RIGHT_GRANT) {
        Some(h) => {
            // Ссылка создателя переходит capability.
            let _ = obj::obj_release(obj_id);
            tf.x[1] = h as u64;
            0
        }
        None => {
            let _ = obj::obj_release(obj_id);
            ERR_NOMEM
        }
    }
}

fn sys_map_anon(tf: &mut TrapFrame, pid: usize) -> u64 {
    let va = tf.x[0] as usize;
    let n = tf.x[1] as usize;
    if n == 0 || n > MAX_ANON_PAGES || va % 4096 != 0 || va < HEAP_BASE || va + n * 4096 > HEAP_END {
        return ERR;
    }
    let Some((root, asid)) = proc::root_of(pid) else { return ERR };
    let mut asp = AddressSpace { root, asid };
    for i in 0..n {
        let a = va + i * 4096;
        if asp.translate(a).is_some() {
            unmap_range(&mut asp, va, i);
            return ERR;
        }
        let Some(pa) = pmm::alloc_page() else {
            unmap_range(&mut asp, va, i);
            return ERR_NOMEM;
        };
        if !asp.map_page(a, pa, Mem::UserData, true) {
            pmm::free_page(pa);
            unmap_range(&mut asp, va, i);
            return ERR_NOMEM;
        }
    }
    if !proc::map_add(pid, MAP_ANON, 0, va, n) {
        unmap_range(&mut asp, va, n);
        return ERR_NOMEM;
    }
    unsafe { asm!("dsb ishst", "isb", options(nostack)) };
    0
}

fn sys_unmap(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(rec) = proc::map_remove(pid, tf.x[0] as usize) else { return ERR };
    let Some((root, asid)) = proc::root_of(pid) else { return ERR };
    let mut asp = AddressSpace { root, asid };
    unmap_range(&mut asp, rec.va, rec.pages);
    if rec.kind == MAP_SHM {
        // rec.id теперь хранит obj_id shm (как u8)
        let _ = obj::obj_release(rec.id as usize);
    }
    0
}

fn user_ptr_ok(va: usize) -> bool {
    va >= USER_BASE && va < USER_END
}

fn sys_thread_create(tf: &mut TrapFrame, pid: usize) -> u64 {
    let (entry, sp, arg) = (tf.x[0] as usize, tf.x[1] as usize, tf.x[2]);
    if !user_ptr_ok(entry) || !user_ptr_ok(sp) || sp % 16 != 0 {
        return ERR;
    }
    match proc::thread_create(pid, entry, sp, arg) {
        Some(tid) => {
            tf.x[1] = tid as u64;
            0
        }
        None => ERR_NOMEM,
    }
}

fn sys_futex_wait(tf: &mut TrapFrame, pid: usize) -> u64 {
    let addr = tf.x[0] as usize;
    if addr % 4 != 0 {
        return ERR;
    }
    let Some(asp) = asp_of(pid) else { return ERR };
    let Some(pa) = asp.translate(addr) else { return ERR };
    // Регистрация и перепроверка значения — атомарно внутри ipc::futex_wait.
    ipc::futex_wait(pa, tf.x[1] as u32, tf.x[2])
}

fn sys_futex_wake(tf: &mut TrapFrame, pid: usize) -> u64 {
    let addr = tf.x[0] as usize;
    if addr % 4 != 0 {
        return ERR;
    }
    let Some(asp) = asp_of(pid) else { return ERR };
    let Some(pa) = asp.translate(addr) else { return ERR };
    tf.x[1] = ipc::futex_wake(pa, tf.x[1] as usize) as u64;
    0
}

fn sys_cancel(tf: &mut TrapFrame, pid: usize) -> u64 {
    let tid = tf.x[0] as usize;
    if tid >= sched::MAX_THREADS || tid == sched::current_tid() || sched::pid_of(tid) != pid {
        return ERR;
    }
    if ipc::abort_wait(tid, ERR_CANCELED) {
        0
    } else {
        ERR
    }
}

fn sys_set_prio(tf: &mut TrapFrame, pid: usize) -> u64 {
    let tid = if tf.x[0] == u64::MAX { sched::current_tid() } else { tf.x[0] as usize };
    let prio = tf.x[1];
    if tid >= sched::MAX_THREADS || sched::pid_of(tid) != pid || prio > proc::max_prio(pid) as u64 {
        return ERR;
    }
    sched::set_prio(tid, prio as u8);
    0
}

// ---- Асинхронные порты / нотификации ----

fn sys_port_create(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(obj_id) = port::port_create(pid) else { return ERR_NOMEM };
    match proc::cap_install(pid, obj_id, RIGHT_RECV) {
        Some(h) => {
            // Ссылка создателя переходит capability.
            let _ = obj::obj_release(obj_id);
            tf.x[1] = h as u64;
            0
        }
        None => {
            let _ = obj::obj_release(obj_id);
            ERR_NOMEM
        }
    }
}

fn sys_port_send(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(cap) = proc::cap_get(pid, tf.x[0] as usize) else { return ERR };
    if cap.rights & RIGHT_SEND == 0 {
        return ERR;
    }
    if obj::obj_type_of(cap.obj_id as usize) != Some(obj::ObjType::Port) {
        return ERR;
    }
    let Some(asp) = asp_of(pid) else { return ERR };
    let send = match load_send(pid, &asp, tf.x[1] as usize) {
        Ok(m) => m,
        Err(e) => return e,
    };
    port::port_send(cap.obj_id as usize, send, pid as u64, tf.x[2])
}

fn sys_port_recv(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(cap) = proc::cap_get(pid, tf.x[0] as usize) else { return ERR };
    if cap.rights & RIGHT_RECV == 0 {
        return ERR;
    }
    if obj::obj_type_of(cap.obj_id as usize) != Some(obj::ObjType::Port) {
        return ERR;
    }
    let Some(asp) = asp_of(pid) else { return ERR };
    match port::port_recv(cap.obj_id as usize, tf.x[2]) {
        Ok((m, from)) => store_recv(pid, &asp, tf.x[1] as usize, m, from),
        Err(e) => e,
    }
}

fn sys_port_notify(tf: &mut TrapFrame, pid: usize) -> u64 {
    let Some(cap) = proc::cap_get(pid, tf.x[0] as usize) else { return ERR };
    // Нотификацию может отправить кто угодно, у кого есть handle (даже без SEND права — это просто сигнал).
    if obj::obj_type_of(cap.obj_id as usize) != Some(obj::ObjType::Port) {
        return ERR;
    }
    port::port_notify(cap.obj_id as usize)
}

fn read_user_path(pid: usize, ptr: usize, len: usize) -> Result<[u8; 128], u64> {
    if len == 0 || len >= 128 { return Err(ERR); }
    let Some(asp) = asp_of(pid) else { return Err(ERR) };
    let mut buf = [0u8; 128];
    if !asp.copy_from_user(ptr, &mut buf[..len]) { return Err(ERR); }
    Ok(buf)
}

fn sys_read(tf: &mut TrapFrame, pid: usize) -> u64 {
    let ptr = tf.x[0] as usize;
    let len = core::cmp::min(tf.x[1] as usize, 256);
    if len == 0 { return 0; }
    let Some(asp) = asp_of(pid) else { return ERR };
    // Block until at least one byte is available; yield/sleep to avoid burning CPU.
    let b0 = loop {
        if let Some(b) = crate::console::read_byte() { break b; }
        // Sleep briefly so other threads run; timer/IRQs will wake us.
        sched::sleep_ticks(1);
        if sched::need_resched() { sched::clear_resched(); sched::schedule(); }
    };
    let mut buf = [0u8; 256];
    buf[0] = b0;
    let mut n = 1usize;
    while n < len {
        if let Some(b) = crate::console::read_byte() { buf[n] = b; n += 1; } else { break; }
    }
    if !asp.copy_to_user(ptr, &buf[..n]) { return ERR; }
    n as u64
}

// FS syscalls removed (initramfs is read-only, no FS in kernel)

/// Создать процесс из ELF в initramfs. Имя — строка в памяти вызывающего процесса.
fn sys_spawn(tf: &mut TrapFrame, pid: usize) -> u64 {
    let ptr = tf.x[0] as usize;
    let len = tf.x[1] as usize;
    if len == 0 || len > 63 {
        return ERR;
    }
    let Some(asp) = asp_of(pid) else { return ERR };
    let mut buf = [0u8; 64];
    if !asp.copy_from_user(ptr, &mut buf[..len]) {
        return ERR;
    }
    let Ok(name) = core::str::from_utf8(&buf[..len]) else { return ERR };
    // x4 = верхняя граница приоритета (0 — по умолчанию).
    let max_prio = if tf.x[4] == 0 {
        proc::DEFAULT_MAX_PRIO
    } else {
        tf.x[4].min(u8::MAX as u64) as u8
    };
    match proc::spawn_from(name, tf.x[2], tf.x[3], pid, max_prio) {
        Some(child) => {
            tf.x[1] = child as u64;
            0
        }
        None => ERR,
    }
}

/// Дождаться завершения ребёнка и забрать код выхода (waitpid).
fn sys_wait(tf: &mut TrapFrame, parent: usize) -> u64 {
    let child = tf.x[0] as usize;
    let timeout = tf.x[1];
    let start = timer::ticks();
    loop {
        match proc::try_wait(child, parent) {
            None => return ERR, // не наш ребёнок или процесс уже забран
            Some(Some(code)) => {
                tf.x[1] = code;
                return 0;
            }
            Some(None) => {}
        }
        if timeout != u64::MAX {
            if timeout == 0 || timer::ticks().saturating_sub(start) >= timeout {
                return ERR_TIMEOUT;
            }
        }
        sched::sleep_ticks(1);
    }
}

/// Статистика физической памяти: x0 = занятые страницы, x1 = свободные.
fn sys_mem_stats(tf: &mut TrapFrame) -> u64 {
    tf.x[1] = pmm::free_pages_estimate() as u64;
    pmm::allocated_pages() as u64
}

/// Статистика ядра: x0 = IPC-вызовы, x1 = ответы, x2 = переключения контекста.
fn sys_kstats(tf: &mut TrapFrame) -> u64 {
    let (calls, replies) = ipc::stats();
    tf.x[1] = replies as u64;
    tf.x[2] = sched::switches();
    calls as u64
}

fn dispatch(tf: &mut TrapFrame) -> u64 {
    let pid = sched::current_pid();
    match tf.x[8] {
        SYS_YIELD => {
            sched::yield_now();
            0
        }
        SYS_WRITE => sys_write(tf.x[0] as usize, tf.x[1] as usize),
        SYS_EXIT => {
            kprintln!("[kernel] pid {} exited with code {}", pid, tf.x[0]);
            proc::kill_process(pid, tf.x[0])
        }
        SYS_GETPID => pid as u64,
        SYS_SLEEP => {
            sched::sleep_ticks(core::cmp::min(tf.x[0], 100_000));
            0
        }
        SYS_TICKS => timer::ticks(),
        SYS_CALL => sys_call(tf, pid),
        SYS_RECV => sys_recv(tf, pid),
        SYS_REPLY => sys_reply(tf, pid),
        SYS_SHM_MAP => sys_shm_map(tf, pid),
        SYS_SHM_CREATE => sys_shm_create(tf, pid),
        SYS_EP_CREATE => sys_ep_create(tf, pid),
        SYS_MAP_ANON => sys_map_anon(tf, pid),
        SYS_UNMAP => sys_unmap(tf, pid),
        SYS_THREAD_CREATE => sys_thread_create(tf, pid),
        SYS_THREAD_EXIT => proc::thread_exit_current(tf.x[0]),
        SYS_GETTID => sched::current_tid() as u64,
        SYS_FUTEX_WAIT => sys_futex_wait(tf, pid),
        SYS_FUTEX_WAKE => sys_futex_wake(tf, pid),
        SYS_CANCEL => sys_cancel(tf, pid),
        SYS_SET_PRIO => sys_set_prio(tf, pid),
        SYS_CAP_DROP => {
            if proc::cap_drop(pid, tf.x[0] as usize) {
                0
            } else {
                ERR
            }
        }
        SYS_PORT_CREATE => sys_port_create(tf, pid),
        SYS_PORT_SEND => sys_port_send(tf, pid),
        SYS_PORT_RECV => sys_port_recv(tf, pid),
        SYS_PORT_NOTIFY => sys_port_notify(tf, pid),
        SYS_READ => sys_read(tf, pid),
        SYS_SPAWN => sys_spawn(tf, pid),
        SYS_WAIT => sys_wait(tf, pid),
        SYS_MEM_STATS => sys_mem_stats(tf),
        SYS_KSTATS => sys_kstats(tf),
        _ => ERR,
    }
}

#[no_mangle]
pub extern "C" fn sync_lower_handler(tf: *mut TrapFrame) {
    let tf = unsafe { &mut *tf };
    let esr: u64;
    unsafe { asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack, preserves_flags)) };
    let ec = (esr >> 26) & 0x3f;
    if ec == 0x15 {
        // SVC из AArch64: ELR уже указывает на следующую инструкцию
        let ret = dispatch(tf);
        tf.x[0] = ret;
        if sched::need_resched() {
            sched::clear_resched();
            sched::schedule();
        } else {
            sched::preempt_check();
        }
        // TF corruption check removed after debug
        return;
    }
    // Data abort / instruction abort из EL0: пробуем demand paging, иначе убиваем процесс.
    let far: u64;
    unsafe { asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack, preserves_flags)) };
    let pid = sched::current_pid();
    let what = match ec {
        0x24 | 0x25 => "data abort",
        0x20 | 0x21 => "instruction abort",
        _ => "exception",
    };
    // Если адрес в пользовательском диапазоне — пытаемся выделить страницу по требованию.
    let is_data = ec == 0x24 || ec == 0x25;
    let is_inst = ec == 0x20 || ec == 0x21;
    if (is_data || is_inst) && (far as usize) >= USER_BASE && (far as usize) < USER_END {
        let va = (far & !0xfff) as usize;
        if handle_demand_page(pid, va, is_data) {
            return; // страница отображена, продолжаем выполнение
        }
    }
    kprintln!(
        "[kernel] FAULT pid {}: {} from EL0, EC={:#x} ESR={:#x} FAR={:#x} ELR={:#x} -> process killed",
        pid, what, ec, esr, far, tf.elr
    );
    proc::kill_process(pid, 255)
}

/// Demand paging: выделяет физическую страницу и отображает её по `va` в адресном пространстве процесса.
/// `write_fault` — true, если это была попытка записи (для будущего COW).
fn handle_demand_page(pid: usize, va: usize, write_fault: bool) -> bool {
    // Выделяем страницу только в окне анонимной памяти (не в text/rodata — это было бы нарушение W^X).
    if va < HEAP_BASE || va >= HEAP_END {
        return false;
    }
    let Some(pa) = pmm::alloc_page() else {
        kprintln!("[kernel] demand page: out of memory at va={:#x}", va);
        return false;
    };
    let Some((root, asid)) = proc::root_of(pid) else {
        pmm::free_page(pa);
        return false;
    };
    let mut asp = AddressSpace { root, asid };
    // Страница не отображена — создаём отображение.
    if !asp.map_page(va, pa, Mem::UserData, true) {
        pmm::free_page(pa);
        return false;
    }
    if !proc::map_add(pid, MAP_ANON, 0, va, 1) {
        asp.unmap_page(va);
        pmm::free_page(pa);
        return false;
    }
    unsafe { asm!("dsb ishst", "isb", options(nostack)) };
    true
}
