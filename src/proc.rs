//! Процессы пользовательского пространства (EL0): адресное пространство, потоки, capability, учёт отображений,
//! полное освобождение ресурсов при завершении.
//!
//! Capability теперь ссылается на объект в единой таблице obj::OBJS по obj_id.
//! При завершении процесса все capabilities других процессов на его объекты отзываются.
use crate::{
    aspace::{AddressSpace, USER_BASE},
    elf, initramfs, ipc,
    mmu::{self, Mem},
    obj, pmm, sched,
    sync::{Global, IrqGuard},
};
use core::arch::{asm, global_asm};

/// ELF процесса линкуется в [USER_BASE, USER_BASE+0x70000); ниже вершины стек главного потока (16 страниц).
pub const USER_STACK_TOP: usize = USER_BASE + 0x80000;
const STACK_PAGES: usize = 16;

pub const MAX_PROCS: usize = 8;
pub const MAX_CAPS: usize = 16;
pub const MAX_MAPS: usize = 32;
pub const MAX_PROC_THREADS: usize = 8;
/// Максимальный приоритет потоков процесса по умолчанию (ядро может поднять через `set_max_prio`).
pub const DEFAULT_MAX_PRIO: u8 = 4;

pub const RIGHT_SEND: u8 = 1;
pub const RIGHT_RECV: u8 = 2;
pub const RIGHT_MAP: u8 = 4;
pub const RIGHT_GRANT: u8 = 8; // можно передать другому процессу в сообщении

pub const MAP_ANON: u8 = 1;
pub const MAP_SHM: u8 = 2;

/// Возможность (capability): процесс может использовать объект ядра только через свою таблицу.
/// obj_id — индекс в единой таблице obj::OBJS (0 = Cap::NONE). Тип объекта узнаётся через obj::obj_type_of.
#[derive(Clone, Copy)]
pub struct Cap {
    pub obj_id: u8,
    pub rights: u8,
}

impl Cap {
    pub const NONE: Cap = Cap { obj_id: 0, rights: 0 };
}

/// Запись об отображении, созданном процессом (для `unmap` и для освобождения при выходе).
#[derive(Clone, Copy)]
pub struct MapRec {
    pub used: bool,
    pub kind: u8,
    pub id: u8,
    pub va: usize,
    pub pages: usize,
}

const MAP_NONE: MapRec = MapRec { used: false, kind: 0, id: 0, va: 0, pages: 0 };

#[derive(Clone, Copy)]
struct Process {
    used: bool,
    alive: bool,
    /// pid родителя (0 — процесс создан ядром/сирота). Нужен для wait/waitpid.
    parent: u8,
    root: usize,
    asid: usize,
    exit_code: u64,
    nthreads: usize,
    max_prio: u8,
    caps: [Cap; MAX_CAPS],
    maps: [MapRec; MAX_MAPS],
}

const PROC_NEW: Process = Process {
    used: false,
    alive: false,
    parent: 0,
    root: 0,
    asid: 0,
    exit_code: 0,
    nthreads: 0,
    max_prio: DEFAULT_MAX_PRIO,
    caps: [Cap::NONE; MAX_CAPS],
    maps: [MAP_NONE; MAX_MAPS],
};

static PROCS: Global<[Process; MAX_PROCS]> = Global::new([PROC_NEW; MAX_PROCS]);

extern "C" {
    fn enter_user(entry: usize, sp: usize, a0: u64, a1: u64) -> !;
}

// Переход в EL0: x0 = entry, x1 = SP_EL0, x2/x3 -> x0/x1 пользователя; остальные регистры обнуляются.
global_asm!(
    r#"
.section .text
.global enter_user
enter_user:
    msr     daifset, #2
    msr     elr_el1, x0
    msr     sp_el0, x1
    msr     spsr_el1, xzr
    mov     x0, x2
    mov     x1, x3
    mov     x2, xzr
    mov     x3, xzr
    mov     x4, xzr
    mov     x5, xzr
    mov     x6, xzr
    mov     x7, xzr
    mov     x8, xzr
    mov     x9, xzr
    mov     x10, xzr
    mov     x11, xzr
    mov     x12, xzr
    mov     x13, xzr
    mov     x14, xzr
    mov     x15, xzr
    mov     x16, xzr
    mov     x17, xzr
    mov     x18, xzr
    mov     x19, xzr
    mov     x20, xzr
    mov     x21, xzr
    mov     x22, xzr
    mov     x23, xzr
    mov     x24, xzr
    mov     x25, xzr
    mov     x26, xzr
    mov     x27, xzr
    mov     x28, xzr
    mov     x29, xzr
    mov     x30, xzr
    eret
"#
);

/// Синхронизация D-cache -> I-cache после записи кода (на реальном железе обязательна).
pub(crate) unsafe fn sync_icache(start: usize, len: usize) {
    let ctr: u64;
    asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags));
    let dline = 4usize << ((ctr >> 16) & 0xf);
    let mut a = start & !(dline - 1);
    while a < start + len {
        asm!("dc cvau, {}", in(reg) a, options(nostack));
        a += dline;
    }
    asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack));
}

/// Входная функция потока ядра, обслуживающего пользовательский поток: берёт параметры и прыгает в EL0.
extern "C" fn user_thread(_: usize) {
    let tid = sched::current_tid();
    let t = sched::uinit_read(tid);
    unsafe { enter_user(t.entry, t.sp, t.a0, t.a1) }
}

/// Строит адресное пространство: ELF + стек. При ошибке всё уже выделенное освобождается.
fn build_image(elf_data: &[u8]) -> Option<(AddressSpace, usize)> {
    let mut asp = AddressSpace::new()?;
    let Some(entry) = elf::load(&mut asp, elf_data) else {
        asp.destroy();
        return None;
    };
    for k in 0..STACK_PAGES {
        let Some(pa) = pmm::alloc_page() else {
            asp.destroy();
            return None;
        };
        if !asp.map_page(USER_STACK_TOP - (k + 1) * 4096, pa, Mem::UserData, true) {
            pmm::free_page(pa);
            asp.destroy();
            return None;
        }
    }
    Some((asp, entry))
}

/// Создаёт процесс из ELF в initramfs с одним (главным) потоком. Возвращает pid (>= 1).
/// `name` используется для поиска файла в initramfs (например, "selftest" -> "bin/selftest").
pub fn spawn(name: &'static str, arg0: u64, arg1: u64) -> Option<usize> {
    spawn_impl(name, name, arg0, arg1, 0, DEFAULT_MAX_PRIO)
}

/// Создаёт процесс по запросу другого процесса (sys_spawn).
/// `parent` — pid вызывающего, `max_prio` — верхняя граница приоритета потоков.
pub fn spawn_from(
    name: &str,
    arg0: u64,
    arg1: u64,
    parent: usize,
    max_prio: u8,
) -> Option<usize> {
    // Имя потока в планировщике статическое: имена из user-space живут только
    // до конца syscall, а Thread хранит &'static str.
    spawn_impl(name, "user", arg0, arg1, parent, max_prio)
}

fn spawn_impl(
    name: &str,
    sched_name: &'static str,
    arg0: u64,
    arg1: u64,
    parent: usize,
    max_prio: u8,
) -> Option<usize> {
    // Ищем ELF в initramfs: пробуем "bin/<name>", "<name>", "sbin/<name>"
    let mut buf = [0u8; 32];
    let n = name.as_bytes();
    let nlen = n.len().min(15);

    // bin/<name>
    buf[..4].copy_from_slice(b"bin/");
    buf[4..4 + nlen].copy_from_slice(&n[..nlen]);
    let elf_data = initramfs::find(core::str::from_utf8(&buf[..4 + nlen]).ok()?)
        .or_else(|| initramfs::find(name))
        .or_else(|| {
            buf[..5].copy_from_slice(b"sbin/");
            buf[5..5 + nlen].copy_from_slice(&n[..nlen]);
            initramfs::find(core::str::from_utf8(&buf[..5 + nlen]).ok()?)
        })?;
    let slot = {
        let _g = IrqGuard::new();
        let mut ps = PROCS.get();
        // Zombie (alive=false, parent!=0) держит слот, пока родитель не заберёт код.
        let i = (0..MAX_PROCS)
            .find(|&i| !ps[i].used || (!ps[i].alive && ps[i].parent == 0))?;
        // alive=true сразу: иначе параллельный spawn увидит слот свободным и займёт его.
        ps[i] = Process {
            used: true,
            alive: true,
            parent: parent.min(u8::MAX as usize) as u8,
            max_prio: max_prio.min(sched::PRIO_MAX),
            ..PROC_NEW
        };
        i
    };
    let Some((asp, entry)) = build_image(elf_data) else {
        unsafe { PROCS.get()[slot] = PROC_NEW };
        return None;
    };
    let pid = slot + 1;
    let _g = IrqGuard::new();
    let mut ps = PROCS.get();
    ps[slot].root = asp.root;
    ps[slot].asid = asp.asid;
    ps[slot].alive = true;
    ps[slot].nthreads = 1;
    let uinit = sched::UInit { entry, sp: USER_STACK_TOP, a0: arg0, a1: arg1 };
    let Some(tid) = sched::spawn_in(sched_name, user_thread, 0, asp.root, asp.asid, pid, uinit) else {
        asp.destroy();
        ps[slot] = PROC_NEW;
        return None;
    };
    drop(ps);
    drop(_g);
    Some(pid)
}

/// Создаёт ещё один поток существующего процесса: стартует в EL0 по адресу `entry` со стеком `sp` и x0 = `arg`.
pub fn thread_create(pid: usize, entry: usize, sp: usize, arg: u64) -> Option<usize> {
    if pid == 0 || pid > MAX_PROCS {
        return None;
    }
    let _g = IrqGuard::new();
    let mut ps = PROCS.get();
    let p = &mut ps[pid - 1];
    if !p.used || !p.alive || p.nthreads >= MAX_PROC_THREADS {
        return None;
    }
    let uinit = sched::UInit { entry, sp, a0: arg, a1: 0 };
    let tid = sched::spawn_in("uthread", user_thread, 0, p.root, p.asid, pid, uinit)?;
    p.nthreads += 1;
    drop(ps);
    drop(_g);
    Some(tid)
}

pub fn set_max_prio(pid: usize, prio: u8) {
    if pid == 0 || pid > MAX_PROCS {
        return;
    }
    let _g = IrqGuard::new();
    unsafe { PROCS.get()[pid - 1].max_prio = prio.min(sched::PRIO_MAX) };
}

pub fn max_prio(pid: usize) -> u8 {
    if pid == 0 || pid > MAX_PROCS {
        return 0;
    }
    let _g = IrqGuard::new();
    unsafe { PROCS.get()[pid - 1].max_prio }
}

/// Корень таблиц и ASID процесса по pid.
pub fn root_of(pid: usize) -> Option<(usize, usize)> {
    if pid == 0 || pid > MAX_PROCS {
        return None;
    }
    let _g = IrqGuard::new();
    let p = unsafe { PROCS.get()[pid - 1] };
    if p.used && p.alive {
        Some((p.root, p.asid))
    } else {
        None
    }
}

// ---- capability ----

/// Выдаёт процессу `pid` возможность на объект `obj_id`; возвращает номер handle.
/// refcount объекта увеличивается при установке capability.
pub fn cap_install(pid: usize, obj_id: usize, rights: u8) -> Option<usize> {
    if pid == 0 || pid > MAX_PROCS || obj_id == 0 {
        return None;
    }
    if obj::obj_type_of(obj_id).is_none() {
        return None;
    }
    let _g = IrqGuard::new();
    let mut __pg_p = PROCS.get(); let p = &mut __pg_p[pid - 1];
    if !p.used {
        return None;
    }
    let h = (0..MAX_CAPS).find(|&h| p.caps[h].obj_id == 0)?;
    p.caps[h] = Cap { obj_id: obj_id as u8, rights };
    obj::obj_acquire(obj_id);
    Some(h)
}

/// Установка capability, пришедшей в сообщении: ссылка на объект уже взята отправителем и переходит в таблицу.
pub fn cap_install_staged(pid: usize, c: Cap) -> Option<usize> {
    if pid == 0 || pid > MAX_PROCS || c.obj_id == 0 {
        return None;
    }
    let _g = IrqGuard::new();
    let mut __pg_p = PROCS.get(); let p = &mut __pg_p[pid - 1];
    if !p.used || !p.alive {
        return None;
    }
    if obj::obj_type_of(c.obj_id as usize).is_none() {
        return None;
    }
    let h = (0..MAX_CAPS).find(|&h| p.caps[h].obj_id == 0)?;
    p.caps[h] = c;
    obj::obj_acquire(c.obj_id as usize);
    Some(h)
}

pub fn cap_get(pid: usize, handle: usize) -> Option<Cap> {
    if pid == 0 || pid > MAX_PROCS || handle >= MAX_CAPS {
        return None;
    }
    let _g = IrqGuard::new();
    let c = unsafe { PROCS.get()[pid - 1].caps[handle] };
    if c.obj_id == 0 {
        None
    } else {
        Some(c)
    }
}

/// Освобождает ссылку, которую держала capability (уменьшает refcount объекта).
pub fn release_cap(c: Cap) {
    if c.obj_id != 0 {
        obj::obj_release(c.obj_id as usize);
    }
}

/// Берёт ссылку на объект capability (для отправки в сообщении).
pub fn retain_cap(c: Cap) {
    if c.obj_id != 0 {
        obj::obj_acquire(c.obj_id as usize);
    }
}

/// Удаляет capability из таблицы процесса.
pub fn cap_drop(pid: usize, handle: usize) -> bool {
    if pid == 0 || pid > MAX_PROCS || handle >= MAX_CAPS {
        return false;
    }
    let _g = IrqGuard::new();
    let mut __pg_p = PROCS.get(); let p = &mut __pg_p[pid - 1];
    let c = p.caps[handle];
    if c.obj_id == 0 {
        return false;
    }
    p.caps[handle] = Cap::NONE;
    release_cap(c);
    true
}

/// Отзывает все capabilities всех процессов, ссылающиеся на объекты, принадлежащие процессу `pid`.
/// Вызывается при завершении процесса, перед уничтожением его собственных объектов.
fn revoke_caps_for_owner(pid: usize) {
    let _g = IrqGuard::new();
    for q in 0..MAX_PROCS {
        let mut __pg_p = PROCS.get(); let p = &mut __pg_p[q];
        if !p.used {
            continue;
        }
        for h in 0..MAX_CAPS {
            let c = p.caps[h];
            if c.obj_id == 0 {
                continue;
            }
            if let Some(owner) = obj::obj_owner(c.obj_id as usize) {
                if owner == pid {
                    p.caps[h] = Cap::NONE;
                    obj::obj_release(c.obj_id as usize);
                }
            }
        }
    }
}

// ---- учёт отображений ----

pub fn map_add(pid: usize, kind: u8, id: usize, va: usize, pages: usize) -> bool {
    if pid == 0 || pid > MAX_PROCS {
        return false;
    }
    let _g = IrqGuard::new();
    let mut __pg_p = PROCS.get(); let p = &mut __pg_p[pid - 1];
    match (0..MAX_MAPS).find(|&i| !p.maps[i].used) {
        Some(i) => {
            p.maps[i] = MapRec { used: true, kind, id: id as u8, va, pages };
            true
        }
        None => false,
    }
}

/// Находит и удаляет запись об отображении, начинающемся с `va`.
pub fn map_remove(pid: usize, va: usize) -> Option<MapRec> {
    if pid == 0 || pid > MAX_PROCS {
        return None;
    }
    let _g = IrqGuard::new();
    let mut __pg_p = PROCS.get(); let p = &mut __pg_p[pid - 1];
    let i = (0..MAX_MAPS).find(|&i| p.maps[i].used && p.maps[i].va == va)?;
    let r = p.maps[i];
    p.maps[i] = MAP_NONE;
    Some(r)
}

// ---- завершение ----

/// Освобождает ресурсы процесса: отзывает чужие capabilities на его объекты, выпускает свои capability,
/// отображения, адресное пространство со всеми страницами.
fn reap(pid: usize, code: u64) {
    mmu::switch_ttbr0(mmu::kernel_root(), 0); // дальше работаем в адресном пространстве ядра

    // 1. Отзываем capabilities ВСЕХ процессов на объекты, принадлежащие этому процессу.
    revoke_caps_for_owner(pid);

    // 2. Освобождаем собственные capability процесса (уменьшаем refcount объектов).
    //    Отображения MAP_SHM отдельно не освобождаем: ссылка на объект учитывается
    //    capability, а не записью об отображении (иначе был бы double release).
    let (root, asid, max_prio) = {
        let mut g = PROCS.get();
        let p = &mut g[pid - 1];
        for c in p.caps.iter() {
            release_cap(*c);
        }
        p.alive = false;
        p.nthreads = 0;
        (p.root, p.asid, p.max_prio)
    };
    if root != 0 {
        AddressSpace { root, asid }.destroy();
    }
    let mut g = PROCS.get();
    {
        let p = &mut g[pid - 1];
        let parent = p.parent;
        let max_prio = p.max_prio;
        if parent != 0 {
            // Zombie: код завершения ждёт родителя (wait/waitpid).
            *p = Process {
                used: true,
                alive: false,
                parent,
                exit_code: code,
                max_prio,
                ..PROC_NEW
            };
        } else {
            // Родителя нет (процесс ядра или сирота) — забирать статус некому.
            *p = PROC_NEW;
        }
    }
    // Дети завершившегося процесса становятся сиротами.
    for i in 0..MAX_PROCS {
        if g[i].used && g[i].parent == pid as u8 {
            g[i].parent = 0;
            if !g[i].alive {
                g[i] = PROC_NEW;
            }
        }
    }
}

/// Завершает процесс целиком: убивает все его потоки, будит зависших на нём, освобождает память.
/// Текущий поток при этом тоже умирает.
pub fn kill_process(pid: usize, code: u64) -> ! {
    let me = sched::current_tid();
    let mask = sched::threads_mask(pid);
    let mut others = [0usize; sched::MAX_THREADS];
    let mut n = 0;
    for tid in sched::IDLE_THREADS..sched::MAX_THREADS {
        if mask & (1 << tid) != 0 && tid != me {
            others[n] = tid;
            n += 1;
            sched::kill_thread(tid);
        }
    }
    // Ждём, пока контексты остальных потоков реально покинут CPU: только после
    // этого можно освобождать их IPC-состояние и адресное пространство.
    // IRQ при ожидании должны быть разрешены, иначе чужое ядро не сможет
    // переключиться (дедлок).
    for &tid in &others[..n] {
        sched::wait_thread_dead(tid);
        ipc::cleanup_thread(tid);
        sched::release_thread(tid);
    }
    ipc::cleanup_thread(me);
    ipc::cleanup_process(pid);
    reap(pid, code);
    sched::thread_exit()
}

/// Завершает только текущий поток процесса. Если он был последним — завершается весь процесс.
pub fn thread_exit_current(code: u64) -> ! {
    let _g = IrqGuard::new();
    let pid = sched::current_pid();
    let me = sched::current_tid();
    let last = {
        let mut __pg_p = PROCS.get(); let p = &mut __pg_p[pid - 1];
        p.nthreads = p.nthreads.saturating_sub(1);
        p.nthreads == 0
    };
    if last {
        kill_process(pid, code)
    }
    ipc::cleanup_thread(me);
    sched::thread_exit()
}

/// (жив ли, код завершения) для pid; None — такого процесса не было.
pub fn exit_status(pid: usize) -> Option<(bool, u64)> {
    if pid == 0 || pid > MAX_PROCS {
        return None;
    }
    let _g = IrqGuard::new();
    let p = unsafe { PROCS.get()[pid - 1] };
    if p.used {
        Some((p.alive, p.exit_code))
    } else {
        None
    }
}

/// Забрать статус завершившегося ребёнка (waitpid).
/// None — это не ребёнок `parent` (или такого pid нет); Some(None) — ещё жив;
/// Some(Some(code)) — код возврата, слот процесса освобождён.
pub fn try_wait(pid: usize, parent: usize) -> Option<Option<u64>> {
    if pid == 0 || pid > MAX_PROCS {
        return None;
    }
    let _g = IrqGuard::new();
    let mut ps = PROCS.get();
    let p = &mut ps[pid - 1];
    if !p.used || p.parent as usize != parent {
        return None;
    }
    if p.alive {
        return Some(None);
    }
    let code = p.exit_code;
    *p = PROC_NEW;
    Some(Some(code))
}
