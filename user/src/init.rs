//! PID 1 — первая пользовательская программа.
//!
//! Ядро поднимает железо и передаёт управление сюда; всё остальное — забота ОС:
//! init запускает сервисы/тесты/оболочку через `sys_spawn` и забирает их коды
//! выхода через `sys_wait`. Это и есть «ядро запускает саму ОС».

use crate::sys;

const ROLE_SELFTEST: u64 = 3;
const ROLE_LEAKER: u64 = 4;
const ROLE_SHELL: u64 = 5;

pub fn run() -> ! {
    uprintln!("init: PID {} started", sys::getpid());

    stage8();

    let Some(sh) = sys::spawn("shell", ROLE_SHELL, 0, 0) else {
        uprintln!("init: shell spawn failed");
        sys::exit(1);
    };
    uprintln!("init: shell pid={}", sh);
    let code = sys::wait(sh, sys::INF).unwrap_or(u64::MAX);
    uprintln!("init: shell exited with code {}", code);
    uprintln!("init: system halted");
    loop {
        sys::sleep(1000);
    }
}

/// Приёмочный прогон bring-up: selftest ядра + три раунда проверки утечек.
fn stage8() {
    let Some(st) = sys::spawn("selftest", ROLE_SELFTEST, 0, 5) else {
        uprintln!("stage 8 FAIL: selftest spawn failed");
        return;
    };
    let st_code = sys::wait(st, 120_000);
    let procs_ok = st_code == Some(0);

    // Базовая линия — после завершения selftest (он не должен оставить страниц).
    let (base, _) = sys::mem_stats();
    let mut mem_ok = true;
    for round in 1..=3 {
        let Some(pid) = sys::spawn("leaker", ROLE_LEAKER, 0, 0) else {
            uprintln!("[init] leaker spawn failed");
            mem_ok = false;
            break;
        };
        let code = sys::wait(pid, 20_000);
        let (now, _) = sys::mem_stats();
        uprintln!(
            "[init] leaker round {}: status={:?} pages in use={} (baseline {})",
            round,
            code,
            now,
            base
        );
        if now != base || code != Some(0) {
            mem_ok = false;
        }
    }

    let (calls, replies, switches) = sys::kstats();
    uprintln!(
        "[init] IPC calls={} replies={} context switches={}",
        calls,
        replies,
        switches
    );

    if procs_ok && mem_ok {
        uprintln!(
            "stage 8 OK: init + spawn/wait, threads, big IPC + cap transfer, timeouts/cancel, priorities, memory reclaim"
        );
    } else {
        uprintln!(
            "stage 8 FAIL (processes ok: {}, memory ok: {})",
            procs_ok,
            mem_ok
        );
    }
}
