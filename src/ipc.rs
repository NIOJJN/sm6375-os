//! Синхронный IPC (rendezvous, как в L4/seL4) + futex.
//!
//! Endpoint теперь — объект в единой таблице obj::OBJS. cap.handle в процессе хранит obj_id,
//! а не индекс в локальном массиве. Ядро samo подставляет from_pid (підделывание невозможно).
//! Сообщение: 4 слова + до MAX_MSG_BYTES байт данных + (необязательно) одна capability.
//!   * `call`  — отправить и ждать ответа (с таймаутом);
//!   * `recv`  — получить сообщение (с таймаутом; 0 — опрос без ожидания);
//!   * `reply` — ответить тому, чей запрос получен последним.
//! Передача capability: ссылка (для shm) берётся при отправке и переходит к получателю при установке в его таблицу;
//! при любой ошибке доставки ссылка возвращается (`release_cap`), поэтому память shm не утекает.
//! Ожидания можно прервать: по таймауту (на тике), `cancel` (из другого потока) или смертью партнёра.
//! Один процесс может иметь много потоков; `reply_to` и состояние ожидания хранятся на поток.
use crate::{
    obj, proc::{self, Cap},
    sched, timer,
    sync::{Global, IrqGuard},
};
use core::ptr::read_volatile;
use core::sync::atomic::{AtomicU64, Ordering};

pub const ERR_INVAL: u64 = u64::MAX; // -1
pub const ERR_DEAD: u64 = u64::MAX - 1; // -2: партнёр/endpoint умер
pub const ERR_NOREPLY: u64 = u64::MAX - 2; // -3
pub const ERR_BUSY: u64 = u64::MAX - 3; // -4
pub const ERR_TIMEOUT: u64 = u64::MAX - 4; // -5
pub const ERR_CANCELED: u64 = u64::MAX - 5; // -6
pub const ERR_AGAIN: u64 = u64::MAX - 6; // -7: значение futex изменилось
pub const ERR_NOMEM: u64 = u64::MAX - 7; // -8

pub const MAX_MSG_BYTES: usize = 1024;
pub const INFINITE: u64 = u64::MAX;

const NONE: usize = usize::MAX;
const MAX_EP: usize = 16;
const SENDQ: usize = 8;
const MAX_WAITERS: usize = 32;

#[derive(Clone, Copy)]
pub struct Msg {
    pub w: [u64; 4],
    pub cap: Option<Cap>,
    pub len: usize,
    pub data: [u8; MAX_MSG_BYTES],
}

impl Msg {
    pub const EMPTY: Msg = Msg { w: [0; 4], cap: None, len: 0, data: [0; MAX_MSG_BYTES] };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wait {
    None,
    Send(usize),  // в очереди отправителей endpoint'а (хранит obj_id)
    Recv(usize),  // единственный получатель endpoint'а (хранит obj_id)
    Reply(usize), // ждёт ответа от сервера (tid клиента)
    Futex,
}

#[derive(Clone, Copy)]
struct Endpoint {
    used: bool,
    obj_id: usize,   // ссылка на объект в obj::OBJS
    owner_pid: usize,
    recv_wait: usize,      // tid ждущего получателя
    sendq: [usize; SENDQ], // tid ждущих отправителей
    sendq_n: usize,
}
const EP_NEW: Endpoint = Endpoint { used: false, obj_id: 0, owner_pid: 0, recv_wait: NONE, sendq: [NONE; SENDQ], sendq_n: 0 };

#[derive(Clone, Copy)]
struct ThreadIpc {
    msg: Msg,
    from_pid: u64, // подставляется ядром; клиент не может подделать
    reply_to: usize, // кому отвечать (tid клиента)
    status: u64,
    wait: Wait,
}
const TH_NEW: ThreadIpc = ThreadIpc { msg: Msg::EMPTY, from_pid: 0, reply_to: NONE, status: 0, wait: Wait::None };

#[derive(Clone, Copy)]
struct Waiter {
    used: bool,
    tid: usize,
    key: usize, // физический адрес слова futex
}

struct State {
    eps: [Endpoint; MAX_EP],
    th: [ThreadIpc; sched::MAX_THREADS],
    fx: [Waiter; MAX_WAITERS],
}

static IPC: Global<State> = Global::new(State {
    eps: [EP_NEW; MAX_EP],
    th: [TH_NEW; sched::MAX_THREADS],
    fx: [Waiter { used: false, tid: NONE, key: 0 }; MAX_WAITERS],
});
static CALLS: AtomicU64 = AtomicU64::new(0);
static REPLIES: AtomicU64 = AtomicU64::new(0);

pub fn stats() -> (u64, u64) {
    (CALLS.load(Ordering::Relaxed), REPLIES.load(Ordering::Relaxed))
}

fn deadline_of(timeout: u64) -> u64 {
    if timeout == INFINITE {
        u64::MAX
    } else {
        timer::ticks().saturating_add(timeout)
    }
}

fn drop_cap(c: Option<Cap>) {
    if let Some(c) = c {
        proc::release_cap(c);
    }
}

fn sendq_remove(ep: &mut Endpoint, tid: usize) {
    let mut k = 0;
    while k < ep.sendq_n {
        if ep.sendq[k] == tid {
            for i in k + 1..ep.sendq_n {
                ep.sendq[i - 1] = ep.sendq[i];
            }
            ep.sendq_n -= 1;
            return;
        }
        k += 1;
    }
}

/// Находит слот endpoint'а по obj_id. Проверяет, что объект живой и является Endpoint.
fn ep_slot(obj_id: usize) -> Option<usize> {
    if obj::obj_type_of(obj_id) != Some(obj::ObjType::Endpoint) {
        return None;
    }
    let payload = obj::obj_payload(obj_id)?;
    if payload >= MAX_EP {
        return None;
    }
    Some(payload)
}

/// Инициализация IPC: очистка слота endpoint при уничтожении объекта.
pub fn init() {
    obj::obj_register_cleanup(obj::ObjType::Endpoint, ep_cleanup);
}

/// Освобождает слот endpoint по payload-индексу (obj уже свободен).
fn ep_cleanup(ep_index: usize) {
    if ep_index >= MAX_EP {
        return;
    }
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    st.eps[ep_index] = EP_NEW;
}

/// Создаёт endpoint и регистрирует его как объект в единой таблице. Возвращает obj_id.
pub fn create_endpoint(owner_pid: usize) -> Option<usize> {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    let ep_index = (0..MAX_EP).find(|&i| !st.eps[i].used)?;
    let obj_id = obj::obj_alloc(obj::ObjType::Endpoint, owner_pid, ep_index)?;
    st.eps[ep_index] = Endpoint { used: true, obj_id, owner_pid, ..EP_NEW };
    Some(obj_id)
}

/// Синхронный вызов: отправить `msg` на endpoint `obj_id`, дождаться ответа.
/// from_pid подставляется ядром из scheduler — клиент не может подделать отправителя.
pub fn call(obj_id: usize, msg: Msg, timeout: u64) -> Result<Msg, u64> {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    let ep_index = match ep_slot(obj_id) {
        Some(i) => i,
        None => { drop(st); drop_cap(msg.cap); return Err(ERR_DEAD); }
    };
    if !st.eps[ep_index].used || st.eps[ep_index].obj_id != obj_id {
        drop(st);
        drop_cap(msg.cap);
        return Err(ERR_DEAD);
    }
    let me = sched::current_tid();
    let my_pid = sched::pid_of(me) as u64; // ядро определяет отправителя
    let my_prio = sched::prio_of(me);
    st.th[me].status = 0;
    if st.eps[ep_index].recv_wait != NONE {
        // Получатель уже ждёт: отдаём сообщение напрямую (cap переходит к нему) и будим его.
        let r = st.eps[ep_index].recv_wait;
        st.eps[ep_index].recv_wait = NONE;
        st.th[r].msg = msg;
        st.th[r].from_pid = my_pid; // kernel-set identity
        st.th[r].reply_to = me;
        st.th[r].status = 0;
        st.th[r].wait = Wait::None;
        st.th[me].wait = Wait::Reply(r);
        sched::boost(r, my_prio);
        sched::wake(r);
    } else {
        if timeout == 0 {
            drop(st);
            drop_cap(msg.cap);
            return Err(ERR_TIMEOUT);
        }
        let n = st.eps[ep_index].sendq_n;
        if n >= SENDQ {
            drop(st);
            drop_cap(msg.cap);
            return Err(ERR_BUSY);
        }
        st.th[me].msg = msg;
        st.eps[ep_index].sendq[n] = me;
        st.eps[ep_index].sendq_n = n + 1;
        st.th[me].wait = Wait::Send(obj_id);
    }
    CALLS.store(CALLS.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    drop(st);
    sched::block_current_until(deadline_of(timeout)); // вернёмся после reply / таймаута / cancel / смерти сервера
    // Состояние изменил другой поток: читаем volatile.
    let mut st = IPC.get();
    let status = unsafe { read_volatile(&st.th[me].status as *const u64) };
    if status != 0 {
        return Err(status);
    }
    let m = unsafe { read_volatile(&st.th[me].msg as *const Msg) };
    st.th[me].msg.cap = None; // владение cap перешло к `m`
    Ok(m)
}

/// Принять сообщение с endpoint `obj_id`. Возвращает (msg, from_pid).
/// from_pid подставляется ядром — подделать невозможно.
pub fn recv(obj_id: usize, timeout: u64) -> Result<(Msg, u64), u64> {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    let ep_index = match ep_slot(obj_id) {
        Some(i) => i,
        None => return Err(ERR_DEAD),
    };
    if !st.eps[ep_index].used || st.eps[ep_index].obj_id != obj_id {
        return Err(ERR_DEAD);
    }
    let me = sched::current_tid();
    let my_pid = sched::pid_of(me);
    if st.eps[ep_index].owner_pid != my_pid {
        return Err(ERR_INVAL);
    }
    if st.eps[ep_index].recv_wait != NONE {
        return Err(ERR_BUSY);
    }
    // Предыдущий запрос остался без ответа — не оставляем клиента ждать вечно.
    let prev = st.th[me].reply_to;
    if prev != NONE {
        if st.th[prev].wait == Wait::Reply(me) {
            st.th[prev].status = ERR_NOREPLY;
            st.th[prev].wait = Wait::None;
            sched::wake(prev);
        }
        st.th[me].reply_to = NONE;
        sched::restore_prio(me);
    }
    if st.eps[ep_index].sendq_n > 0 {
        // Есть ждущий отправитель: забираем сообщение, клиент остаётся заблокированным до reply.
        let c = st.eps[ep_index].sendq[0];
        for i in 1..st.eps[ep_index].sendq_n {
            st.eps[ep_index].sendq[i - 1] = st.eps[ep_index].sendq[i];
        }
        st.eps[ep_index].sendq_n -= 1;
        let m = st.th[c].msg;
        st.th[c].msg.cap = None; // cap перемещён в `m`
        let from = sched::pid_of(c) as u64; // kernel-set identity
        st.th[me].reply_to = c;
        st.th[c].wait = Wait::Reply(me);
        sched::boost(me, sched::prio_of(c));
        return Ok((m, from));
    }
    if timeout == 0 {
        return Err(ERR_TIMEOUT);
    }
    st.eps[ep_index].recv_wait = me;
    st.th[me].wait = Wait::Recv(obj_id);
    st.th[me].status = 0;
    drop(st);
    sched::block_current_until(deadline_of(timeout));
    let mut st = IPC.get();
    let status = unsafe { read_volatile(&st.th[me].status as *const u64) };
    if status != 0 {
        return Err(status);
    }
    let m = unsafe { read_volatile(&st.th[me].msg as *const Msg) };
    let from = unsafe { read_volatile(&st.th[me].from_pid as *const u64) };
    st.th[me].msg.cap = None;
    Ok((m, from))
}

pub fn reply(msg: Msg) -> Result<(), u64> {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    let me = sched::current_tid();
    let c = st.th[me].reply_to;
    if c == NONE {
        drop(st);
        drop_cap(msg.cap);
        return Err(ERR_NOREPLY);
    }
    st.th[me].reply_to = NONE;
    sched::restore_prio(me);
    if st.th[c].wait != Wait::Reply(me) {
        // Клиент уже отвалился (таймаут/cancel/смерть): ответ некому доставлять.
        drop(st);
        drop_cap(msg.cap);
        return Err(ERR_NOREPLY);
    }
    st.th[c].msg = msg;
    st.th[c].status = 0;
    st.th[c].wait = Wait::None;
    REPLIES.store(REPLIES.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    sched::wake(c);
    Ok(())
}

/// Прерывает ожидание потока `tid` с кодом `status` (таймаут, cancel). false — поток ничего не ждал.
pub fn abort_wait(tid: usize, status: u64) -> bool {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    if tid >= sched::MAX_THREADS {
        return false;
    }
    let mut pending: Option<Cap> = None;
    match st.th[tid].wait {
        Wait::Send(obj_id) => {
            let ep_index = match ep_slot(obj_id) {
                Some(i) => i,
                None => return false,
            };
            sendq_remove(&mut st.eps[ep_index], tid);
            pending = st.th[tid].msg.cap.take();
        }
        Wait::Recv(obj_id) => {
            let ep_index = match ep_slot(obj_id) {
                Some(i) => i,
                None => return false,
            };
            if st.eps[ep_index].recv_wait == tid {
                st.eps[ep_index].recv_wait = NONE;
            }
        }
        Wait::Reply(_) => {} // сервер увидит несоответствие при reply
        Wait::Futex => {
            for w in st.fx.iter_mut() {
                if w.used && w.tid == tid {
                    w.used = false;
                }
            }
        }
        Wait::None => return false,
    }
    st.th[tid].wait = Wait::None;
    st.th[tid].status = status;
    drop(st);
    // release_cap может уничтожить объект и взять IPC-лок в cleanup: только вне лока.
    drop_cap(pending);
    sched::wake(tid);
    true
}

/// Поток `tid` завершается (штатно или убит): убрать его из всех очередей и разбудить зависших на нём.
pub fn cleanup_thread(tid: usize) {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    if tid >= sched::MAX_THREADS {
        return;
    }
    match st.th[tid].wait {
        Wait::Send(obj_id) => {
            let ep_index = match ep_slot(obj_id) {
                Some(i) => i,
                None => return,
            };
            sendq_remove(&mut st.eps[ep_index], tid);
        }
        Wait::Recv(obj_id) => {
            let ep_index = match ep_slot(obj_id) {
                Some(i) => i,
                None => return,
            };
            if st.eps[ep_index].recv_wait == tid {
                st.eps[ep_index].recv_wait = NONE;
            }
        }
        Wait::Futex => {
            for w in st.fx.iter_mut() {
                if w.used && w.tid == tid {
                    w.used = false;
                }
            }
        }
        _ => {}
    }
    // Клиент, чей запрос этот поток уже принял, не должен зависнуть.
    let c = st.th[tid].reply_to;
    if c != NONE && st.th[c].wait == Wait::Reply(tid) {
        st.th[c].status = ERR_DEAD;
        st.th[c].wait = Wait::None;
        sched::wake(c);
    }
    // Сообщение, которое поток не успел забрать, и его cap.
    let cap = st.th[tid].msg.cap.take();
    st.th[tid] = TH_NEW;
    drop(st);
    // Вне IPC-лока: obj_release может вызвать cleanup, который снова берёт IPC.
    drop_cap(cap);
}

/// Процесс `pid` завершён: будит ждущих на его endpoint'ах с ERR_DEAD.
/// Само Zerstörung объектов обрабатывается obj::destroy_owner в proc::reap.
pub fn cleanup_process(pid: usize) {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    let mut pending: [Option<Cap>; sched::MAX_THREADS] = [None; sched::MAX_THREADS];
    let mut np = 0;
    for i in 0..MAX_EP {
        if st.eps[i].used && st.eps[i].owner_pid == pid {
            for k in 0..st.eps[i].sendq_n {
                let s = st.eps[i].sendq[k];
                let c = st.th[s].msg.cap.take();
                if np < pending.len() {
                    pending[np] = c;
                    np += 1;
                }
                st.th[s].status = ERR_DEAD;
                st.th[s].wait = Wait::None;
                sched::wake(s);
            }
            // recv_wait ждёт на этом endpoint'е — разбудить с ошибкой.
            if st.eps[i].recv_wait != NONE {
                let r = st.eps[i].recv_wait;
                st.eps[i].recv_wait = NONE;
                st.th[r].status = ERR_DEAD;
                st.th[r].wait = Wait::None;
                sched::wake(r);
            }
            st.eps[i] = EP_NEW;
        }
    }
    drop(st);
    // Освобождаем capability вне IPC-лока (cleanup объекта снова возьмёт IPC).
    for c in pending[..np].iter() {
        drop_cap(*c);
    }
}

// ---- futex: ожидание по физическому адресу слова (работает и между процессами через shm) ----

/// Блокирует поток до изменения слова по `key` (физ. адрес). `expected` — значение,
/// которое вызывающий видел до входа. 0 — разбужен, ERR_AGAIN — слово уже изменилось.
///
/// Регистрация ожидающего и перепроверка значения выполняются под IPC-локом с
/// замаскированными IRQ. Это закрывает гонку на SMP: если другой CPU изменил слово
/// и вызвал `futex_wake` до регистрации, пробуждение не потеряется — мы сами увидим
/// новое значение и вернём ERR_AGAIN вместо бесконечного сна.
pub fn futex_wait(key: usize, expected: u32, timeout: u64) -> u64 {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    if timeout == 0 {
        return ERR_TIMEOUT;
    }
    let Some(i) = (0..MAX_WAITERS).find(|&i| !st.fx[i].used) else { return ERR_BUSY };
    let me = sched::current_tid();
    st.fx[i] = Waiter { used: true, tid: me, key };
    st.th[me].wait = Wait::Futex;
    st.th[me].status = 0;
    let changed = unsafe { read_volatile(key as *const u32) } != expected;
    if changed {
        st.fx[i] = Waiter { used: false, tid: NONE, key: 0 };
        st.th[me].wait = Wait::None;
        return ERR_AGAIN;
    }
    drop(st);
    sched::block_current_until(deadline_of(timeout));
    let st = IPC.get();
    unsafe { read_volatile(&st.th[me].status as *const u64) }
}

/// Будит до `n` потоков, ждущих слово `key`. Возвращает число разбуженных.
pub fn futex_wake(key: usize, n: usize) -> usize {
    let _g = IrqGuard::new();
    let mut st = IPC.get();
    let mut woken = 0;
    for i in 0..MAX_WAITERS {
        if woken >= n {
            break;
        }
        if st.fx[i].used && st.fx[i].key == key {
            let t = st.fx[i].tid;
            st.fx[i].used = false;
            st.th[t].wait = Wait::None;
            st.th[t].status = 0;
            sched::wake(t);
            woken += 1;
        }
    }
    woken
}
