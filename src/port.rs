//! Асинхронные порты и нотификации поверх единой таблицы объектов.
//!
//! Порт — это объект (ObjType::Port) с очередью сообщений и счётчиком нотификаций.
//!   * `port_send`  — асинхронная отправка (очередь);
//!   * `port_recv`  — приём сообщения или нотификации (from_pid = 0 означает нотификацию);
//!   * `port_notify`— сигнал без полезной нагрузки (как eventfd / seL4 Signal).
//! Порты не требуют ответа (нет reply_to), в отличие от синхронного call/reply.
use crate::{
    ipc::{self, ERR_AGAIN, ERR_DEAD, ERR_INVAL, ERR_TIMEOUT},
    obj,
    sync::SpinLock,
};
use core::sync::atomic::{AtomicU64, Ordering};

pub const MAX_PORTS: usize = 16;
pub const PORT_QUEUE_SIZE: usize = 8;

#[derive(Clone, Copy)]
struct PortData {
    used: bool,
    obj_id: usize,
    owner_pid: usize,
    head: usize,
    tail: usize,
    count: usize,
    queue: [Option<(ipc::Msg, u64)>; PORT_QUEUE_SIZE], // (сообщение, pid отправителя; pid=0 = нотификация)
    recv_wait: usize, // tid ждущего получателя
    sendq: [usize; PORT_QUEUE_SIZE], // tid ждущих отправителей (очередь piena)
    sendq_n: usize,
    notify_count: u64,
    notify_wait: usize, // tid ждущего нотификацию
}

const PORT_NEW: PortData = PortData {
    used: false,
    obj_id: 0,
    owner_pid: 0,
    head: 0,
    tail: 0,
    count: 0,
    queue: [None; PORT_QUEUE_SIZE],
    recv_wait: usize::MAX,
    sendq: [usize::MAX; PORT_QUEUE_SIZE],
    sendq_n: 0,
    notify_count: 0,
    notify_wait: usize::MAX,
};

static PORTS: SpinLock<[PortData; MAX_PORTS]> = SpinLock::new([PORT_NEW; MAX_PORTS]);
static PORT_SENDS: AtomicU64 = AtomicU64::new(0);
static PORT_RECVS: AtomicU64 = AtomicU64::new(0);
static PORT_NOTIFIES: AtomicU64 = AtomicU64::new(0);

pub fn stats() -> (u64, u64, u64) {
    (
        PORT_SENDS.load(Ordering::Relaxed),
        PORT_RECVS.load(Ordering::Relaxed),
        PORT_NOTIFIES.load(Ordering::Relaxed),
    )
}

/// Инициализация: регистрирует коллбэк очистки для объектов Port.
pub fn init() {
    obj::obj_register_cleanup(obj::ObjType::Port, port_cleanup);
}

fn port_cleanup(port_index: usize) {
    if port_index >= MAX_PORTS {
        return;
    }
    let mut ports = PORTS.lock();
    ports[port_index] = PORT_NEW;
}

/// Создаёт порт. owner_pid — владелец (может recv). Возвращает obj_id.
pub fn port_create(owner_pid: usize) -> Option<usize> {
    // Слот резервируется до obj_alloc, иначе на SMP два ядра возьмут один индекс.
    let port_index = {
        let mut ports = PORTS.lock();
        let i = (0..MAX_PORTS).find(|&i| !ports[i].used)?;
        ports[i].used = true;
        i
    };
    let obj_id = match obj::obj_alloc(obj::ObjType::Port, owner_pid, port_index) {
        Some(id) => id,
        None => {
            PORTS.lock()[port_index] = PORT_NEW;
            return None;
        }
    };
    let mut ports = PORTS.lock();
    ports[port_index] = PortData { used: true, obj_id, owner_pid, ..PORT_NEW };
    Some(obj_id)
}

/// Асинхронная отправка сообщения на порт. Если очередь piena — возвращает ERR_AGAIN (клиент должен повторить).
pub fn port_send(obj_id: usize, msg: ipc::Msg, sender_pid: u64, _timeout: u64) -> u64 {
    let mut ports = PORTS.lock();
    let port_index = match obj::obj_payload(obj_id) {
        Some(i) if i < MAX_PORTS && ports[i].obj_id == obj_id => i,
        _ => return ERR_DEAD,
    };
    let ep = &mut ports[port_index];

    // Если есть место — клаём сразу.
    if ep.count < PORT_QUEUE_SIZE {
        ep.queue[ep.tail] = Some((msg, sender_pid));
        ep.tail = (ep.tail + 1) % PORT_QUEUE_SIZE;
        ep.count += 1;
        // Будим получателя, если ждёт.
        if ep.recv_wait != usize::MAX {
            let r = ep.recv_wait;
            ep.recv_wait = usize::MAX;
            drop(ports);
            sched_wake(r);
        } else {
            drop(ports);
        }
        PORT_SENDS.fetch_add(1, Ordering::Relaxed);
        return 0;
    }

    // Очередь piena — не блокируемся (MVP), клиент повторит позже.
    ERR_AGAIN
}

/// Приём из порта. Возвращает (msg, from_pid). from_pid = 0 — нотификация.
pub fn port_recv(obj_id: usize, timeout: u64) -> Result<(ipc::Msg, u64), u64> {
    let port_index = {
        let ports = PORTS.lock();
        match obj::obj_payload(obj_id) {
            Some(i) if i < MAX_PORTS && ports[i].obj_id == obj_id => i,
            _ => return Err(ERR_DEAD),
        }
    };

    loop {
        let mut ports = PORTS.lock();
        let ep = &mut ports[port_index];
        if ep.owner_pid != sched_current_pid() {
            drop(ports);
            return Err(ERR_INVAL);
        }

        // Есть pending нотификация?
        if ep.notify_count > 0 {
            ep.notify_count -= 1;
            drop(ports);
            PORT_RECVS.fetch_add(1, Ordering::Relaxed);
            return Ok((ipc::Msg::EMPTY, 0));
        }

        // Есть сообщение в очереди?
        if ep.count > 0 {
            let (msg, from) = ep.queue[ep.head].take().unwrap();
            ep.head = (ep.head + 1) % PORT_QUEUE_SIZE;
            ep.count -= 1;
            // Если был ждущий отправитель (очередь была piena), будим его.
            if ep.sendq_n > 0 {
                let s = ep.sendq[0];
                for i in 1..ep.sendq_n {
                    ep.sendq[i - 1] = ep.sendq[i];
                }
                ep.sendq_n -= 1;
                let wake_tid = s;
                drop(ports);
                sched_wake(wake_tid);
            } else {
                drop(ports);
            }
            PORT_RECVS.fetch_add(1, Ordering::Relaxed);
            return Ok((msg, from));
        }

        // Ничего нет — ждём (если timeout > 0).
        if timeout == 0 {
            drop(ports);
            return Err(ERR_TIMEOUT);
        }
        let me = sched_current_tid();
        ep.recv_wait = me;
        drop(ports);

        sched_block_until(
            if timeout == u64::MAX { u64::MAX } else { crate::timer::ticks().saturating_add(timeout) },
        );

        // После пробуждения проверяем снова.
    }
}

/// Нотификация на порт (сигнал без данных). Увеличивает счётчик и будит получателя.
pub fn port_notify(obj_id: usize) -> u64 {
    let mut ports = PORTS.lock();
    let port_index = match obj::obj_payload(obj_id) {
        Some(i) if i < MAX_PORTS && ports[i].obj_id == obj_id => i,
        _ => return ERR_DEAD,
    };
    let ep = &mut ports[port_index];
    ep.notify_count += 1;
    PORT_NOTIFIES.fetch_add(1, Ordering::Relaxed);
    if ep.recv_wait != usize::MAX {
        let r = ep.recv_wait;
        ep.recv_wait = usize::MAX;
        drop(ports);
        sched_wake(r);
    } else {
        drop(ports);
    }
    0
}

// ---- Вспомогательные обёртки над sched/timer (чтобы не зависеть от блокировок внутри) ----

fn sched_current_pid() -> usize {
    crate::sched::current_pid()
}

fn sched_current_tid() -> usize {
    crate::sched::current_tid()
}

fn sched_wake(tid: usize) {
    crate::sched::wake(tid);
}

fn sched_block_until(deadline: u64) {
    crate::sched::block_current_until(deadline);
}
