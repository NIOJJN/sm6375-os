//! Единая таблица объектов ядра.
//!
//! Каждый объект: тип, владелец (pid), счётчик ссылок, полезная нагрузка
//! (индекс во вложенной таблице: ipc::eps, shm::SHMS, port::PORTS).
//! Capabilities в таблицах процессов ссылаются на объект по obj_id, а не по
//! отдельным индексам endpoint/shm. Это позволяет:
//!  * единообразно отслеживать время жизни объектов (refcount);
//!  * отзывать все capabilities на объекты умершего процесса;
//!  * добавлять новые типы объектов (ports, threads, ...) без изменения Cap.
use crate::sync::SpinLock;

pub const MAX_OBJ: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ObjType {
    None = 0,
    Endpoint = 1,
    Shm = 2,
    Mmio = 3,
    Port = 4,
}

const OBJ_NONE: Obj = Obj { obj_type: ObjType::None, owner_pid: 0, refs: 0, payload: 0 };

/// obj_id 0 зарезервирован под Cap::NONE; распределение начинается с 1.
const FIRST_OBJ_ID: usize = 1;

#[repr(C, align(8))]
struct Obj {
    obj_type: ObjType,
    owner_pid: usize,
    refs: usize,
    payload: usize,
}

static OBJS: SpinLock<[Obj; MAX_OBJ]> = SpinLock::new([OBJ_NONE; MAX_OBJ]);

/// Коллбэк очистки типоспецифичных ресурсов (вызывается когда refs доходит до 0).
type CleanupFn = fn(usize); // payload
static CLEANUPS: SpinLock<[Option<CleanupFn>; 5]> = SpinLock::new([None; 5]);

/// Регистрирует функцию очистки для типа объекта. Вызывается при инициализации подсистемы.
pub fn obj_register_cleanup(obj_type: ObjType, f: CleanupFn) {
    let mut c = CLEANUPS.lock();
    c[obj_type as usize] = Some(f);
}

/// Выделяет объект и возвращает его obj_id (>= 1).
pub fn obj_alloc(obj_type: ObjType, owner_pid: usize, payload: usize) -> Option<usize> {
    let mut objs = OBJS.lock();
    let id = (FIRST_OBJ_ID..MAX_OBJ).find(|&i| objs[i].obj_type == ObjType::None)?;
    objs[id] = Obj { obj_type, owner_pid, refs: 1, payload };
    Some(id)
}

/// Увеличивает refcount. Возвращает false, если объекта не существует.
pub fn obj_acquire(id: usize) -> bool {
    let mut objs = OBJS.lock();
    if id >= MAX_OBJ || id == 0 || objs[id].obj_type == ObjType::None {
        return false;
    }
    objs[id].refs += 1;
    true
}

/// Уменьшает refcount. Возвращает true, если объект стал мёртвым (refs == 0)
/// и его типоспецифичные ресурсы очищены.
pub fn obj_release(id: usize) -> bool {
    // Коллбэк очистки вызывается строго ПОСЛЕ отпускания OBJS: он берёт свои
    // блокировки (SHMS/PORTS/...), и удержание OBJS при этом дало бы ABBA-deadlock.
    let (dead, cleanup, payload) = {
        let mut objs = OBJS.lock();
        if id >= MAX_OBJ || id == 0 || objs[id].obj_type == ObjType::None {
            return false;
        }
        let o = &mut objs[id];
        if o.refs > 0 {
            o.refs -= 1;
        }
        if o.refs == 0 {
            let cleanup = {
                let c = CLEANUPS.lock();
                c[o.obj_type as usize]
            };
            let payload = o.payload;
            o.obj_type = ObjType::None;
            o.owner_pid = 0;
            o.payload = 0;
            (true, cleanup, payload)
        } else {
            (false, None, 0)
        }
    };
    if dead {
        if let Some(f) = cleanup {
            f(payload);
        }
        return true;
    }
    false
}

pub fn obj_type_of(id: usize) -> Option<ObjType> {
    let objs = OBJS.lock();
    if id >= MAX_OBJ || id == 0 { return None; }
    let t = objs[id].obj_type;
    if t == ObjType::None { None } else { Some(t) }
}

pub fn obj_owner(id: usize) -> Option<usize> {
    let objs = OBJS.lock();
    if id >= MAX_OBJ || id == 0 { return None; }
    let o = &objs[id];
    if o.obj_type == ObjType::None { None } else { Some(o.owner_pid) }
}

pub fn obj_payload(id: usize) -> Option<usize> {
    let objs = OBJS.lock();
    if id >= MAX_OBJ || id == 0 { return None; }
    let o = &objs[id];
    if o.obj_type == ObjType::None { None } else { Some(o.payload) }
}

/// Уничтожает все объекты, принадлежащие процессу `pid` (вызывается после отзыва caps).
pub fn destroy_owner(pid: usize) {
    // 1. Помечаем объекты как мёртвые и собираем их (тип, payload).
    let to_cleanup: [(ObjType, usize); 16] = {
        let mut objs = OBJS.lock();
        let mut tmp = [(ObjType::None, 0); 16];
        let mut n = 0;
        for o in objs.iter_mut() {
            if o.obj_type != ObjType::None && o.owner_pid == pid {
                let t = o.obj_type;
                let p = o.payload;
                o.obj_type = ObjType::None;
                o.owner_pid = 0;
                o.refs = 0;
                o.payload = 0;
                if n < 16 {
                    tmp[n] = (t, p);
                    n += 1;
                }
            }
        }
        tmp
    };

    // 2. Копируем коллбэки очистки (Option<fn> — Copy, можно скопировать массив).
    let fns = {
        let c = CLEANUPS.lock();
        *c // Copy
    };

    // 3. Вызываем очистку вне блокировок.
    for &(t, p) in to_cleanup.iter() {
        if t == ObjType::None { continue; }
        if let Some(f) = fns[t as usize] {
            f(p);
        }
    }
}
