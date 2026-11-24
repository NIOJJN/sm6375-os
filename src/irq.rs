//! Диспетчер IRQ. Вызывается из boot.S (irq_entry). Здесь нельзя печатать: консоль не реентерабельна.
use crate::{gic, sched, smp, timer};
use core::sync::atomic::{AtomicU32, Ordering};

pub static SGI_COUNT: AtomicU32 = AtomicU32::new(0);
pub static UNKNOWN_COUNT: AtomicU32 = AtomicU32::new(0);
pub static LAST_UNKNOWN: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
pub extern "C" fn irq_handler(from_user: u64) {
    let id = gic::ack();
    if id >= 1020 {
        return; // ложное прерывание
    }
    let mut tick = false;
    if id == smp::IPI_RESCHED {
        smp::on_ipi(id);
        sched::on_ipi();
    } else if id < 16 {
        SGI_COUNT.fetch_add(1, Ordering::Relaxed);
    } else if id == timer::intid() {
        timer::on_irq(smp::current_cpu_index() == 0);
        tick = true;
    } else {
        UNKNOWN_COUNT.fetch_add(1, Ordering::Relaxed);
        LAST_UNKNOWN.store(id, Ordering::Relaxed);
    }
    // EOI строго ДО планировщика: после переключения потока этот обработчик может долго не продолжиться.
    gic::eoi(id);
    if tick {
        sched::on_tick();
    }
    // Вытесняем только возврат в EL0: в EL1 ядро может находиться внутри
    // критической секции или посреди переключения контекста. Точки
    // планирования в ядре (idle_loop, sleep, IPC) вызывают schedule сами.
    if from_user != 0 && sched::need_resched() {
        sched::clear_resched();
        sched::schedule();
    }
}
