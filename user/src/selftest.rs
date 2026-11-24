//! Самотест ядра из пользовательского пространства: потоки + futex, анонимная память, большие сообщения IPC
//! с передачей capability, таймауты, отмена ожидания, приоритеты. Роль 3. Роль 4 (leaker) нужна ядру для проверки
//! освобождения памяти: процесс занимает память и выходит.
use crate::sys::*;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::*};

static NEXT_STACK: AtomicUsize = AtomicUsize::new(HEAP_BASE);

/// Создаёт поток со своим стеком (32 КиБ анонимной памяти).
fn spawn_thread(entry: extern "C" fn(u64) -> !, arg: u64) -> Option<u64> {
    let va = NEXT_STACK.fetch_add(0x10000, Relaxed);
    if !map_anon(va, 8) {
        return None;
    }
    thread_create(entry, va + 0x8000, arg)
}

// ---- 1. потоки + futex-мьютекс ----
static COUNTER: AtomicU32 = AtomicU32::new(0);
static LOCK: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicU32 = AtomicU32::new(0);

fn lock() {
    loop {
        if LOCK.compare_exchange(0, 1, Acquire, Relaxed).is_ok() {
            return;
        }
        futex_wait(&LOCK, 1, INF);
    }
}

fn unlock() {
    LOCK.store(0, Release);
    futex_wake(&LOCK, 1);
}

extern "C" fn worker(_: u64) -> ! {
    for _ in 0..200 {
        lock();
        let v = COUNTER.load(Relaxed);
        yield_now(); // нарочно уступаем внутри критической секции: без мьютекса обновления потерялись бы
        COUNTER.store(v + 1, Relaxed);
        unlock();
    }
    DONE.fetch_add(1, AcqRel);
    futex_wake(&DONE, 1);
    thread_exit(0)
}

fn t_threads() -> bool {
    COUNTER.store(0, Relaxed);
    DONE.store(0, Relaxed);
    for i in 0..3 {
        if spawn_thread(worker, i).is_none() {
            return false;
        }
    }
    for _ in 0..200 {
        let d = DONE.load(Acquire);
        if d == 3 {
            return COUNTER.load(Acquire) == 600;
        }
        futex_wait(&DONE, d, 50);
    }
    false
}

// ---- 2. анонимная память ----
fn t_anon() -> bool {
    let va = HEAP_BASE + 0x40_0000;
    if !map_anon(va, 16) {
        return false;
    }
    let p = va as *mut u64;
    for i in 0..16 * 512 {
        unsafe { p.add(i).write_volatile(i as u64 ^ 0x55aa) };
    }
    for i in 0..16 * 512 {
        if unsafe { p.add(i).read_volatile() } != i as u64 ^ 0x55aa {
            return false;
        }
    }
    if map_anon(va, 1) {
        return false; // повторное отображение занятого адреса должно быть отклонено
    }
    if !unmap(va) || unmap(va) {
        return false;
    }
    // после unmap страница возвращена системе; новая — снова нулевая
    if !map_anon(va, 1) {
        return false;
    }
    let zero = unsafe { (va as *const u64).read_volatile() } == 0;
    unmap(va) && zero
}

// ---- 3. большое сообщение + передача capability ----
static EP: AtomicU64 = AtomicU64::new(0);
static SRV_OK: AtomicU32 = AtomicU32::new(0);

extern "C" fn ipc_server(_: u64) -> ! {
    let ep = EP.load(Relaxed);
    let mut inb = [0u8; 1024];
    let mut m = Msg::with_buf(&mut inb);
    if recv(ep, &mut m, 500) != 0 {
        SRV_OK.store(2, Relaxed);
        thread_exit(1)
    }
    let mut ok = m.w == [1, 2, 3, 4] && m.len == 1000;
    for i in 0..1000 {
        if inb[i] != (i % 251) as u8 {
            ok = false;
        }
    }
    let Some(h) = shm_create(1) else { thread_exit(2) };
    if !shm_map(h, SHM_BASE + 0x4000) {
        thread_exit(3)
    }
    unsafe { ((SHM_BASE + 0x4000) as *mut u64).write_volatile(0xC0FF_EE12_3456) };
    let mut out = [0u8; 1000];
    for i in 0..1000 {
        out[i] = inb[999 - i];
    }
    let mut r = Msg::with_data([4, 3, 2, 1], &out);
    r.cap = h; // передаём capability на shm вместе с ответом
    let st = reply(&r);
    SRV_OK.store(if ok && st == 0 { 1 } else { 2 }, Relaxed);
    thread_exit(0)
}

fn t_ipc() -> bool {
    let Some(ep) = ep_create() else { return false };
    EP.store(ep, Relaxed);
    SRV_OK.store(0, Relaxed);
    if spawn_thread(ipc_server, 0).is_none() {
        return false;
    }
    let mut payload = [0u8; 1000];
    for i in 0..1000 {
        payload[i] = (i % 251) as u8;
    }
    let s = Msg::with_data([1, 2, 3, 4], &payload);
    let mut out = [0u8; 1024];
    let mut r = Msg::with_buf(&mut out);
    if call(ep, &s, &mut r, 500) != 0 || r.w != [4, 3, 2, 1] || r.len != 1000 || r.cap == NO_CAP {
        return false;
    }
    for i in 0..1000 {
        if out[i] != payload[999 - i] {
            return false;
        }
    }
    // полученная capability работает: общая страница содержит то, что записал «сервер»
    if !shm_map(r.cap, SHM_BASE + 0x8000) {
        return false;
    }
    if unsafe { ((SHM_BASE + 0x8000) as *const u64).read_volatile() } != 0xC0FF_EE12_3456 {
        return false;
    }
    // права урезаны: у полученной capability нет GRANT, передать её дальше нельзя
    let mut s2 = Msg::new();
    s2.cap = r.cap;
    let mut r2 = Msg::new();
    if call(ep, &s2, &mut r2, 5) != ERR_INVAL {
        return false;
    }
    // дождаться сервера и убедиться, что он доволен
    sleep(5);
    SRV_OK.load(Relaxed) == 1 && cap_drop(r.cap) && !cap_drop(r.cap)
}

// ---- 4. таймауты ----
static EP2: AtomicU64 = AtomicU64::new(0);
static LAZY_REPLY: AtomicU64 = AtomicU64::new(0);

extern "C" fn lazy_server(_: u64) -> ! {
    let ep = EP2.load(Relaxed);
    let mut m = Msg::new();
    if recv(ep, &mut m, INF) == 0 {
        sleep(60); // принял запрос и «завис»
        LAZY_REPLY.store(reply(&Msg::new()), Relaxed);
    }
    thread_exit(0)
}

fn t_timeouts() -> bool {
    let Some(ep) = ep_create() else { return false };
    let mut m = Msg::new();
    if recv(ep, &mut m, 0) != ERR_TIMEOUT {
        return false; // опрос без ожидания
    }
    let t0 = ticks();
    let st = recv(ep, &mut m, 10);
    let dt = ticks() - t0;
    if st != ERR_TIMEOUT || dt < 10 || dt > 25 {
        return false;
    }
    let w = AtomicU32::new(0);
    let t0 = ticks();
    let st = futex_wait(&w, 0, 10);
    let dt = ticks() - t0;
    if st != ERR_TIMEOUT || dt < 10 || dt > 25 {
        return false;
    }
    if futex_wait(&w, 1, INF) != ERR_AGAIN {
        return false; // значение не совпало — не засыпаем
    }
    // call к серверу, который принял запрос и не отвечает: клиент уходит по таймауту
    EP2.store(ep, Relaxed);
    LAZY_REPLY.store(0, Relaxed);
    if spawn_thread(lazy_server, 0).is_none() {
        return false;
    }
    let s = Msg::new();
    let mut r = Msg::new();
    let t0 = ticks();
    let st = call(ep, &s, &mut r, 20);
    let dt = ticks() - t0;
    if st != ERR_TIMEOUT || dt < 20 || dt > 45 {
        return false;
    }
    // поздний ответ сервера не должен быть доставлен кому-то другому
    sleep(80);
    LAZY_REPLY.load(Relaxed) == ERR_NOREPLY
}

// ---- 5. отмена ожидания ----
static EP3: AtomicU64 = AtomicU64::new(0);
static CANCEL_ST: AtomicU64 = AtomicU64::new(0);

extern "C" fn blocked_thread(_: u64) -> ! {
    let mut m = Msg::new();
    CANCEL_ST.store(recv(EP3.load(Relaxed), &mut m, INF), Relaxed);
    thread_exit(0)
}

fn t_cancel() -> bool {
    let Some(ep) = ep_create() else { return false };
    EP3.store(ep, Relaxed);
    CANCEL_ST.store(0, Relaxed);
    let Some(tid) = spawn_thread(blocked_thread, 0) else { return false };
    sleep(5); // поток уходит в recv
    if cancel(tid) != 0 {
        return false;
    }
    sleep(5);
    CANCEL_ST.load(Relaxed) == ERR_CANCELED && cancel(tid) != 0 // повторная отмена: поток уже ничего не ждёт
}

// ---- 6. приоритеты ----
static SPIN: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicU32 = AtomicU32::new(0);
static HI_WOKE: AtomicU64 = AtomicU64::new(0);

extern "C" fn spinner(_: u64) -> ! {
    while STOP.load(Relaxed) == 0 {
        SPIN.fetch_add(1, Relaxed);
    }
    thread_exit(0)
}

extern "C" fn hi_thread(delay: u64) -> ! {
    sleep(delay);
    HI_WOKE.store(ticks(), Relaxed);
    thread_exit(0)
}

fn t_priority() -> bool {
    SPIN.store(0, Relaxed);
    STOP.store(0, Relaxed);
    HI_WOKE.store(0, Relaxed);
    let Some(spin) = spawn_thread(spinner, 0) else { return false };
    if set_prio(spin, 1) != 0 {
        return false;
    }
    let start = ticks();
    let Some(hi) = spawn_thread(hi_thread, 20) else { return false };
    if set_prio(hi, 5) != 0 {
        return false;
    }
    if set_prio(hi, 7) == 0 {
        return false; // выше разрешённого процессу максимума
    }
    sleep(40); // пока главный поток спит, крутится spinner (приоритет 1); hi просыпается и обязан вытеснить его
    let woke = HI_WOKE.load(Relaxed);
    STOP.store(1, Relaxed);
    sleep(3);
    woke >= start + 20 && woke <= start + 23 && SPIN.load(Relaxed) > 0
}

pub fn run() -> ! {
    let tests: [(&str, fn() -> bool); 6] = [
        ("threads + futex mutex", t_threads),
        ("anonymous memory map/unmap", t_anon),
        ("big IPC message + capability transfer", t_ipc),
        ("timeouts (recv, futex, call, late reply)", t_timeouts),
        ("cancel of a blocked wait", t_cancel),
        ("priorities and preemption", t_priority),
    ];
    let mut fails = 0;
    for (name, f) in tests.iter() {
        let ok = f();
        uprintln!("selftest {}: {}", name, if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    }
    exit(if fails == 0 { 0 } else { 1 })
}

/// Роль 4: занимает память (анонимные страницы, shm, свои таблицы страниц) и выходит.
/// Ядро проверяет, что после этого число занятых страниц вернулось к исходному.
pub fn leaker() -> ! {
    if !map_anon(HEAP_BASE, 64) {
        exit(1)
    }
    let p = HEAP_BASE as *mut u64;
    for i in 0..64 * 512 {
        unsafe { p.add(i).write_volatile(i as u64) };
    }
    let Some(h) = shm_create(8) else { exit(2) };
    if !shm_map(h, SHM_BASE) {
        exit(3)
    }
    unsafe { (SHM_BASE as *mut u64).write_volatile(0x1234) };
    exit(0)
}
