//! Потоки ядра и SMP-планировщик с приоритетами и вытеснением.
//!
//! * Глобальная таблица потоков защищена `SCHED` (SpinLock), а «текущий поток»
//!   хранится в per-CPU структуре `smp::PerCpu`. Поэтому каждое ядро имеет
//!   собственный контекст и собственный TTBR0/ASID.
//! * Потоки 0..MAX_CPUS — idle-потоки ядер. Они не выбираются обычным
//!   планировщиком и нужны как контекст, на который ядро уходит, когда готовых
//!   потоков нет.
//! * Готовый поток может выполниться на любом CPU: выбор и перевод в Running
//!   выполняются под `SCHED`, поэтому два ядра не возьмут один поток.
//! * Переключение контекста идёт в два шага: текущий поток переводится в
//!   `Switching`, после фактического сохранения SP новое ядро/поток вызывает
//!   `complete_switch` и делает его `Ready`. Это закрывает гонку «другое ядро
//!   увидело Ready раньше, чем сохранился SP».
use crate::{
    ipc, mmu, smp,
    sync::{irq_restore, irq_save, IrqGuard, SpinLock, Global},
    timer,
};
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Параметры первого входа в EL0 для каждого потока ядра, обслуживающего пользовательский поток.
#[derive(Clone, Copy)]
pub struct UInit {
    pub entry: usize,
    pub sp: usize,
    pub a0: u64,
    pub a1: u64,
}

impl UInit {
    pub const fn new() -> Self {
        Self { entry: 0, sp: 0, a0: 0, a1: 0 }
    }
}

static UINIT: Global<[UInit; MAX_THREADS]> = Global::new([UInit::new(); MAX_THREADS]);

/// Записывает параметры входа в EL0 для потока `tid`.
/// Должен вызываться ДО перевода потока в Ready (под той же блокировкой SCHED).
pub fn uinit_write(tid: usize, u: UInit) {
    if tid < MAX_THREADS {
        UINIT.get()[tid] = u;
    }
}

/// Читает параметры входа в EL0 для потока `tid`.
pub fn uinit_read(tid: usize) -> UInit {
    if tid < MAX_THREADS {
        UINIT.get()[tid]
    } else {
        UInit::new()
    }
}

pub const MAX_THREADS: usize = 32;
/// Idle-потоки занимают слоты 0..MAX_CPUS (по одному на ядро).
pub const IDLE_THREADS: usize = smp::MAX_CPUS;
const STACK_SIZE: usize = crate::config::limits::KERNEL_STACK_PAGES * 4096;
const SLICE_TICKS: u32 = 5;
pub const PRIO_DEFAULT: u8 = 3;
pub const PRIO_MAX: u8 = 7;
const NO_DEADLINE: u64 = u64::MAX;
const NONE: usize = usize::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Unused,
    /// Создан, но ещё не опубликован (ядро дописывает UInit и т.п.).
    Starting,
    Ready,
    Running,
    /// SP ещё сохраняется в context_switch; другой CPU не должен его выбирать.
    Switching,
    Sleeping,
    Blocked, // ждёт события (IPC, futex), возможно с дедлайном
    /// Поток завершается и ждёт, пока его контекст будет сохранён.
    Exiting,
    Dead,
}

#[derive(Clone, Copy)]
struct Thread {
    state: State,
    sp: usize,
    wake: u64,
    deadline: u64,
    slice: u32,
    #[allow(dead_code)]
    name: &'static str,
    ttbr0: usize, // 0 = адресное пространство ядра
    asid: usize,  // ASID для пользовательского адресного пространства (0 = ядро)
    sp_el0: u64,
    pid: usize, // 0 = поток ядра
    prio: u8,      // текущий (с учётом наследования)
    base_prio: u8, // собственный
    idle: bool,
    cpu: usize,
    kill_pending: bool,
    /// `wake()` пришёл раньше, чем поток успел заблокироваться. Следующий
    /// `block_current_until`/`sleep_ticks` должен немедленно вернуться.
    wake_pending: bool,
    /// Поток убит, но его IPC-состояние ещё не вычищено. Слот нельзя
    /// переиспользовать, иначе очистка затронет чужой новый поток.
    pinned: bool,
}

const EMPTY: Thread = Thread {
    state: State::Unused,
    sp: 0,
    wake: 0,
    deadline: NO_DEADLINE,
    slice: 0,
    name: "",
    ttbr0: 0,
    asid: 0,
    sp_el0: 0,
    pid: 0,
    prio: PRIO_DEFAULT,
    base_prio: PRIO_DEFAULT,
    idle: false,
    cpu: 0,
    kill_pending: false,
    wake_pending: false,
    pinned: false,
};

struct Sched {
    threads: [Thread; MAX_THREADS],
}

#[repr(C, align(16))]
struct Stacks([[u8; STACK_SIZE]; MAX_THREADS]);

static STACKS: crate::sync::Global<Stacks> =
    crate::sync::Global::new(Stacks([[0; STACK_SIZE]; MAX_THREADS]));
static SCHED: SpinLock<Sched> = SpinLock::new(Sched { threads: [EMPTY; MAX_THREADS] });
static STARTED: AtomicBool = AtomicBool::new(false);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
/// Поток, который данное ядро только что сняло с выполнения. Новый контекст
/// (или trampoline) превратит его в Ready/Dead в `complete_switch`.
static PENDING_PREV: [AtomicUsize; smp::MAX_CPUS] =
    [const { AtomicUsize::new(NONE) }; smp::MAX_CPUS];

extern "C" {
    fn context_switch(old_sp: *mut usize, new_sp: usize);
    fn thread_trampoline();
}

global_asm!(
    r#"
.section .text
.global context_switch
context_switch:
    sub     sp, sp, #0x60
    stp     x19, x20, [sp, #0x00]
    stp     x21, x22, [sp, #0x10]
    stp     x23, x24, [sp, #0x20]
    stp     x25, x26, [sp, #0x30]
    stp     x27, x28, [sp, #0x40]
    stp     x29, x30, [sp, #0x50]
    mov     x9, sp
    str     x9, [x0]
    mov     sp, x1
    ldp     x19, x20, [sp, #0x00]
    ldp     x21, x22, [sp, #0x10]
    ldp     x23, x24, [sp, #0x20]
    ldp     x25, x26, [sp, #0x30]
    ldp     x27, x28, [sp, #0x40]
    ldp     x29, x30, [sp, #0x50]
    add     sp, sp, #0x60
    ret

// Первый вход нового потока: x19 = функция, x20 = аргумент.
// Сначала (при замаскированных IRQ) сообщаем планировщику, что предыдущий
// контекст уже сохранён, и только потом разрешаем прерывания: иначе IRQ может
// вклиниться до завершения переключения и потерять PENDING_PREV.
.global thread_trampoline
thread_trampoline:
    bl      sched_complete_switch
    msr     daifclr, #2
    mov     x0, x20
    blr     x19
    bl      thread_exit
1:
    wfe
    b       1b
"#
);

/// Завершает переключение: предыдущий поток этого CPU уже сохранён.
fn complete_switch() {
    let cpu = smp::current_cpu_index();
    let prev = PENDING_PREV[cpu].swap(NONE, Ordering::AcqRel);
    if prev == NONE || prev >= MAX_THREADS {
        return;
    }
    let mut s = SCHED.lock();
    let t = &mut s.threads[prev];
    match t.state {
        State::Switching => {
            if t.kill_pending {
                t.state = State::Dead;
                t.deadline = NO_DEADLINE;
            } else {
                t.state = State::Ready;
            }
        }
        State::Exiting => {
            t.state = State::Dead;
            t.deadline = NO_DEADLINE;
        }
        _ => {}
    }
}

/// Вызывается из `thread_trampoline` (asm).
#[no_mangle]
pub extern "C" fn sched_complete_switch() {
    complete_switch();
}

/// Idle-цикл ядра. Поток 0 — boot CPU, потоки 1..MAX_CPUS — вторичные ядра.
pub extern "C" fn idle_loop(cpu: usize) -> ! {
    let _ = cpu;
    loop {
        smp::clear_need_resched();
        schedule();
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}

/// Инициализирует idle-поток boot CPU. Вызывать до `smp::init`.
pub fn init() {
    let mut s = SCHED.lock();
    s.threads[0] = Thread {
        state: State::Running,
        sp: 0, // сохранится при первом context_switch
        name: "idle0",
        prio: 0,
        base_prio: 0,
        idle: true,
        cpu: 0,
        ..EMPTY
    };
    smp::set_current_tid(0);
    smp::set_current_pid(0);
    STARTED.store(true, Ordering::Release);
}

/// Регистрирует idle-поток вторичного CPU. Вызывается из `secondary_main`.
pub fn init_secondary(cpu: usize) {
    while !STARTED.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    if cpu == 0 || cpu >= IDLE_THREADS {
        return;
    }
    let mut s = SCHED.lock();
    s.threads[cpu] = Thread {
        state: State::Running,
        sp: 0, // сохранится при первом context_switch
        name: "idle",
        prio: 0,
        base_prio: 0,
        idle: true,
        cpu,
        ..EMPTY
    };
    drop(s);
    smp::set_current_tid(cpu);
    smp::set_current_pid(0);
}

/// Создаёт поток ядра. Возвращает его номер или None, если слотов нет.
pub fn spawn(name: &'static str, entry: extern "C" fn(usize), arg: usize) -> Option<usize> {
    let tid = spawn_in(name, entry, arg, 0, 0, 0, UInit::new())?;
    Some(tid)
}

/// Как `spawn`, но поток принадлежит процессу `pid` и работает в адресном пространстве `ttbr0` с ASID `asid`.
/// `uinit` — параметры первого входа в EL0 (entry, sp, a0, a1). Записываются атомарно с созданием потока.
/// Поток создаётся сразу в состоянии `Ready` (нет отдельного `start_thread`).
pub fn spawn_in(
    name: &'static str,
    entry: extern "C" fn(usize),
    arg: usize,
    ttbr0: usize,
    asid: usize,
    pid: usize,
    uinit: UInit,
) -> Option<usize> {
    let _g = IrqGuard::new();
    let mut s = SCHED.lock();
    let i = (IDLE_THREADS..MAX_THREADS)
        .find(|&i| matches!(s.threads[i].state, State::Unused | State::Dead) && !s.threads[i].pinned)?;
    let top = unsafe { STACKS.get().0[i].as_mut_ptr().add(STACK_SIZE) } as usize;
    let frame = top - 0x60;
    unsafe {
        let f = frame as *mut u64;
        for k in 0..12 {
            f.add(k).write(0);
        }
        f.add(0).write(entry as usize as u64); // x19
        f.add(1).write(arg as u64); // x20
        f.add(11).write(thread_trampoline as *const () as usize as u64); // x30
    }
    // Пишем UINIT ПЕРЕД переводом в Ready — под той же блокировкой SCHED.
    UINIT.get()[i] = uinit;
    s.threads[i] = Thread {
        state: State::Ready,
        sp: frame,
        slice: SLICE_TICKS,
        name,
        ttbr0,
        asid,
        pid,
        cpu: NONE,
        ..EMPTY
    };
    smp::broadcast_resched();
    Some(i)
}

/// Выбор следующего потока для текущего CPU (только чтение состояния).
/// `strict` — только строго более приоритетный, чем текущий.
fn pick_next(s: &Sched, cpu: usize, strict: bool) -> usize {
    let cur = smp::current_tid();
    let cur_valid = cur < MAX_THREADS;
    let cur_running = cur_valid && s.threads[cur].state == State::Running;
    let cur_is_idle = cur_valid && s.threads[cur].idle;
    let threshold = if strict && cur_running && !cur_is_idle {
        s.threads[cur].prio as i32
    } else {
        -1
    };

    let mut best = NONE;
    let mut best_prio = -1i32;
    for i in IDLE_THREADS..MAX_THREADS {
        if s.threads[i].state != State::Ready {
            continue;
        }
        let p = s.threads[i].prio as i32;
        if p > threshold && p > best_prio {
            best = i;
            best_prio = p;
        }
    }
    if best != NONE {
        return best;
    }
    if cur_running {
        return cur;
    }
    // Нет готовых и текущий не Running — уходим в idle-поток этого ядра.
    cpu
}

/// То же, что `pick_next`, но выбранный поток СРАЗУ переводится в Running под
/// тем же `SCHED`. Это резервирование: иначе два ядра могли бы выбрать один и
/// тот же Ready-поток в окне между выбором и переключением, и он выполнялся бы
/// одновременно на двух CPU с общим стеком.
///
/// Вызывающий обязан сразу выполнить `do_switch(cur, next)` (кроме случая
/// next == cur). Поток, помеченный Running, останется таковым и после
/// переключения — повторно его никто не выберет.
fn take_next(s: &mut Sched, cpu: usize, strict: bool) -> usize {
    let next = pick_next(s, cpu, strict);
    if next != smp::current_tid() && next < MAX_THREADS
        && s.threads[next].state == State::Ready
    {
        s.threads[next].state = State::Running;
    }
    next
}

/// Переключение на выбранный поток. IRQ остаются замаскированными до фактического
/// переключения; восстанавливаются уже в контексте нового потока.
fn do_switch(cur: usize, next: usize) {
    if cur == next || cur >= MAX_THREADS || next >= MAX_THREADS {
        return;
    }
    let cpu = smp::current_cpu_index();
    let daif = irq_save();
    let (cur_sp_ptr, next_sp) = {
        let mut s = SCHED.lock();
        if s.threads[cur].state == State::Running {
            s.threads[cur].state = State::Switching;
        }
        s.threads[next].state = State::Running;
        s.threads[next].slice = SLICE_TICKS;
        s.threads[next].cpu = cpu;
        SWITCHES.fetch_add(1, Ordering::Relaxed);

        let sp0: u64;
        unsafe { asm!("mrs {}, sp_el0", out(reg) sp0, options(nomem, nostack, preserves_flags)) };
        s.threads[cur].sp_el0 = sp0;
        unsafe { asm!("msr sp_el0, {}", in(reg) s.threads[next].sp_el0, options(nomem, nostack, preserves_flags)) };

        let root = if s.threads[next].ttbr0 != 0 {
            s.threads[next].ttbr0
        } else {
            mmu::kernel_root()
        };
        let asid = if s.threads[next].ttbr0 != 0 { s.threads[next].asid } else { 0 };
        mmu::switch_ttbr0(root, asid);

        smp::set_current_tid(next);
        smp::set_current_pid(s.threads[next].pid);
        let cur_sp_ptr = &mut s.threads[cur].sp as *mut usize;
        let next_sp = s.threads[next].sp;
        (cur_sp_ptr, next_sp)
    };
    PENDING_PREV[cpu].store(cur, Ordering::Release);
    unsafe { context_switch(cur_sp_ptr, next_sp) };
    // Завершаем переключение ДО разрешения IRQ: см. комментарий в thread_trampoline.
    complete_switch();
    irq_restore(daif);
}

/// Из обработчика тика: будит спящих, снимает просроченные ожидания,
/// обновляет квант текущего потока этого CPU.
pub fn on_tick() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    let cpu = smp::current_cpu_index();
    let now = timer::ticks();
    let mut expired = [0usize; MAX_THREADS];
    let mut ne = 0;
    let mut need = false;
    {
        let mut s = SCHED.lock();
        let cur = smp::current_tid();
        for i in IDLE_THREADS..MAX_THREADS {
            match s.threads[i].state {
                State::Sleeping if s.threads[i].wake <= now => {
                    s.threads[i].state = State::Ready;
                    if i != cur && s.threads[i].prio >= s.threads[cur].prio {
                        need = true;
                    }
                }
                State::Blocked if s.threads[i].deadline <= now => {
                    s.threads[i].deadline = NO_DEADLINE;
                    expired[ne] = i;
                    ne += 1;
                }
                _ => {}
            }
        }
        if cur < MAX_THREADS {
            if s.threads[cur].slice > 0 {
                s.threads[cur].slice -= 1;
            }
            if s.threads[cur].slice == 0 {
                s.threads[cur].slice = SLICE_TICKS;
                if !need {
                    need = true; // истёк квант: уступаем равным
                }
            } else if !s.threads[cur].idle {
                let next = pick_next(&s, cpu, true);
                if next != cur {
                    need = true;
                }
            } else {
                let next = pick_next(&s, cpu, false);
                if next != cur {
                    need = true;
                }
            }
        } else {
            need = true;
        }
    }
    // abort_wait/wake берут IPC/SCHED — вызывать их нельзя, удерживая SCHED.
    for &tid in &expired[..ne] {
        if !ipc::abort_wait(tid, ipc::ERR_TIMEOUT) {
            wake(tid);
        }
    }
    if need {
        smp::set_need_resched();
    }
}

/// Возвращает true, если текущему CPU нужно перепланирование.
pub fn need_resched() -> bool {
    smp::need_resched()
}

/// Сбрасывает флаг необходимости перепланирования на текущем CPU.
pub fn clear_resched() {
    smp::clear_need_resched();
}

/// IPI_RESCHED: отметить необходимость перепланирования.
pub fn on_ipi() {
    smp::set_need_resched();
}

/// Если появился готовый поток с более высоким приоритетом — переключиться на него сразу.
pub fn preempt_check() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    let cpu = smp::current_cpu_index();
    let cur = smp::current_tid();
    let next = {
        let mut s = SCHED.lock();
        take_next(&mut s, cpu, true)
    };
    if next != cur {
        do_switch(cur, next);
    }
}

/// Перепланирование по запросу: уступить любому подходящему потоку.
pub fn schedule() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    let cpu = smp::current_cpu_index();
    let cur = smp::current_tid();
    let next = {
        let mut s = SCHED.lock();
        take_next(&mut s, cpu, false)
    };
    if next != cur {
        do_switch(cur, next);
    }
}

/// Добровольно уступить процессор (равным по приоритету тоже).
pub fn yield_now() {
    schedule();
}

/// Уснуть на `n` тиков (idle-потоку спать нельзя).
pub fn sleep_ticks(n: u64) {
    let cpu = smp::current_cpu_index();
    let cur = smp::current_tid();
    let next = {
        let _g = IrqGuard::new();
        let mut s = SCHED.lock();
        if cur >= MAX_THREADS || s.threads[cur].idle {
            return;
        }
        // wake() мог прийти до того, как мы успели заблокироваться (SMP):
        // не теряем его. kill_pending тоже нельзя усыплять — поток должен умереть.
        if s.threads[cur].wake_pending {
            s.threads[cur].wake_pending = false;
            return;
        }
        if s.threads[cur].kill_pending {
            return;
        }
        s.threads[cur].state = State::Sleeping;
        s.threads[cur].wake = timer::ticks() + n;
        let next = take_next(&mut s, cpu, false);
        if next == cur {
            s.threads[cur].state = State::Running;
            return;
        }
        next
    };
    do_switch(cur, next);
}

/// Блокирует текущий поток до `wake` или до дедлайна (в тиках; u64::MAX — без дедлайна).
pub fn block_current_until(deadline: u64) {
    let cpu = smp::current_cpu_index();
    let cur = smp::current_tid();
    let next = {
        let _g = IrqGuard::new();
        let mut s = SCHED.lock();
        if cur >= MAX_THREADS || s.threads[cur].idle {
            return;
        }
        // См. комментарий в sleep_ticks: закрываем гонку потерянного пробуждения.
        if s.threads[cur].wake_pending {
            s.threads[cur].wake_pending = false;
            return;
        }
        if s.threads[cur].kill_pending {
            return;
        }
        s.threads[cur].state = State::Blocked;
        s.threads[cur].deadline = deadline;
        let next = take_next(&mut s, cpu, false);
        if next == cur {
            s.threads[cur].state = State::Running;
            return;
        }
        next
    };
    do_switch(cur, next);
}

/// Делает заблокированный поток готовым к запуску.
pub fn wake(tid: usize) {
    let changed = {
        let _g = IrqGuard::new();
        let mut s = SCHED.lock();
        if tid >= MAX_THREADS {
            false
        } else {
            let t = &mut s.threads[tid];
            match t.state {
                State::Blocked | State::Sleeping => {
                    t.state = State::Ready;
                    t.deadline = NO_DEADLINE;
                    t.wake_pending = false;
                    true
                }
                // Поток ещё не дошёл до block_current_until: запоминаем
                // пробуждение, иначе на SMP оно потеряется и поток уснёт навсегда.
                State::Running | State::Switching | State::Starting | State::Ready => {
                    t.wake_pending = true;
                    false
                }
                _ => false,
            }
        }
    };
    if changed {
        smp::broadcast_resched();
    }
}

/// Завершение потока (вызывается и из trampoline, когда функция потока вернулась).
#[no_mangle]
pub extern "C" fn thread_exit() -> ! {
    let cpu = smp::current_cpu_index();
    let cur = smp::current_tid();
    let next = {
        let _g = IrqGuard::new();
        let mut s = SCHED.lock();
        if cur < MAX_THREADS {
            s.threads[cur].state = State::Exiting;
            s.threads[cur].deadline = NO_DEADLINE;
        }
        let next = take_next(&mut s, cpu, false);
        if next == cur {
            drop(s);
            loop {
                unsafe { asm!("wfe", options(nomem, nostack)) };
            }
        }
        next
    };
    do_switch(cur, next);
    loop {
        unsafe { asm!("wfe", options(nomem, nostack)) };
    }
}

/// Принудительно завершает другой поток (не текущий).
/// Если поток сейчас выполняется на другом CPU, он получит kill_pending и
/// умрёт после ближайшего переключения контекста.
pub fn kill_thread(tid: usize) {
    if tid < IDLE_THREADS || tid >= MAX_THREADS {
        return;
    }
    let _g = IrqGuard::new();
    let mut s = SCHED.lock();
    match s.threads[tid].state {
        State::Running | State::Switching => {
            s.threads[tid].kill_pending = true;
            s.threads[tid].pinned = true;
        }
        State::Starting | State::Ready | State::Sleeping | State::Blocked => {
            s.threads[tid].state = State::Dead;
            s.threads[tid].deadline = NO_DEADLINE;
            s.threads[tid].pinned = true;
        }
        _ => {}
    }
    drop(s);
    smp::broadcast_resched();
}

/// Ждёт, пока контекст потока будет снят с CPU. Нужно перед освобождением
/// адресного пространства процесса: ни одно ядро не должно ещё работать в нём.
pub fn wait_thread_dead(tid: usize) {
    if tid < IDLE_THREADS || tid >= MAX_THREADS {
        return;
    }
    loop {
        let dead = {
            let _g = IrqGuard::new();
            let s = SCHED.lock();
            matches!(s.threads[tid].state, State::Dead | State::Unused)
        };
        if dead {
            return;
        }
        core::hint::spin_loop();
    }
}

/// Слот убитого потока после `wait_thread_dead` и очистки IPC снова доступен.
pub fn release_thread(tid: usize) {
    if tid < IDLE_THREADS || tid >= MAX_THREADS {
        return;
    }
    let _g = IrqGuard::new();
    let mut s = SCHED.lock();
    if s.threads[tid].pinned && !matches!(s.threads[tid].state, State::Running | State::Switching) {
        s.threads[tid] = EMPTY;
    }
}

/// Битовая маска живых потоков процесса `pid`.
pub fn threads_mask(pid: usize) -> u64 {
    let _g = IrqGuard::new();
    let s = SCHED.lock();
    let mut m = 0u64;
    for i in IDLE_THREADS..MAX_THREADS {
        if s.threads[i].pid == pid && !matches!(s.threads[i].state, State::Unused | State::Dead) {
            m |= 1 << i;
        }
    }
    m
}

pub fn current_tid() -> usize {
    smp::current_tid()
}

pub fn pid_of(tid: usize) -> usize {
    if tid >= MAX_THREADS {
        return 0;
    }
    let _g = IrqGuard::new();
    let s = SCHED.lock();
    s.threads[tid].pid
}

/// pid процесса, которому принадлежит текущий поток (0 — поток ядра).
pub fn current_pid() -> usize {
    smp::current_pid()
}

pub fn prio_of(tid: usize) -> u8 {
    if tid >= MAX_THREADS {
        return 0;
    }
    let _g = IrqGuard::new();
    let s = SCHED.lock();
    s.threads[tid].prio
}

/// Задаёт собственный приоритет потока (и текущий).
pub fn set_prio(tid: usize, prio: u8) {
    if tid < IDLE_THREADS || tid >= MAX_THREADS {
        return;
    }
    let _g = IrqGuard::new();
    let mut s = SCHED.lock();
    if prio <= PRIO_MAX {
        s.threads[tid].base_prio = prio;
        s.threads[tid].prio = prio;
    }
}

/// Наследование приоритета: сервер временно получает приоритет клиента.
pub fn boost(tid: usize, prio: u8) {
    if tid >= MAX_THREADS {
        return;
    }
    let _g = IrqGuard::new();
    let mut s = SCHED.lock();
    if s.threads[tid].prio < prio {
        s.threads[tid].prio = prio;
    }
}

pub fn restore_prio(tid: usize) {
    if tid >= MAX_THREADS {
        return;
    }
    let _g = IrqGuard::new();
    let mut s = SCHED.lock();
    s.threads[tid].prio = s.threads[tid].base_prio;
}

pub fn switches() -> u64 {
    SWITCHES.load(Ordering::Relaxed)
}

/// Переключиться на первый готовый поток и начать его выполнение.
pub fn start_first() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    let cpu = smp::current_cpu_index();
    let cur = smp::current_tid();
    let next = {
        let mut s = SCHED.lock();
        take_next(&mut s, cpu, false)
    };
    if next != cur {
        do_switch(cur, next);
    }
}
