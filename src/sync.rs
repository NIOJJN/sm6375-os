//! Примитивы синхронизации.
//!
//! * `Global<T>` — раннее одноядерное состояние без блокировок (оставлено для
//!   структур, к которым обращается только CPU0).
//! * `IrqGuard` — критическая секция на одном ядре (маскирование IRQ).
//! * `SpinLock<T>` — межъядерный спинлок с одновременным маскированием IRQ на
//!   текущем CPU. Порядок в `Drop`: сначала отпускаем блокировку, затем
//!   восстанавливаем DAIF, чтобы не открыть окно для IRQ под локом.
use core::arch::asm;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// SMP-safe global: wraps a SpinLock. `lock()` returns a guard.
pub struct Global<T>(SpinLock<T>);

unsafe impl<T: Send> Sync for Global<T> {}

impl<T> Global<T> {
    pub const fn new(v: T) -> Self {
        Self(SpinLock::new(v))
    }

    #[inline]
    pub fn lock(&self) -> SpinGuard<'_, T> {
        self.0.lock()
    }

    /// Backward-compatible name; returns a lock guard.
    #[inline]
    pub fn get(&self) -> SpinGuard<'_, T> {
        self.0.lock()
    }
}

/// Сохраняет DAIF и маскирует IRQ. Парный вызов — [`irq_restore`].
#[inline]
pub fn irq_save() -> u64 {
    let d: u64;
    unsafe {
        asm!("mrs {}, daif", out(reg) d, options(nomem, nostack, preserves_flags));
        asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
    }
    d
}

/// Восстанавливает DAIF, сохранённый [`irq_save`].
#[inline]
pub fn irq_restore(state: u64) {
    // Бит 7 (0x80) — PSTATE.I. Если IRQ были разрешены до входа — разрешаем снова.
    if state & (1 << 7) == 0 {
        unsafe { asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags)) };
    }
}

/// Критическая секция на одном ядре: маскирует IRQ, при выходе восстанавливает прежнее состояние.
pub struct IrqGuard(u64);

impl IrqGuard {
    pub fn new() -> Self {
        Self(irq_save())
    }
}

impl Default for IrqGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        irq_restore(self.0);
    }
}

/// Межъядерный спинлок (test-and-set с backoff).
///
/// `lock()` маскирует IRQ на текущем CPU на всё время критической секции: это
/// защищает от реентерабельного входа из обработчика прерывания. Для структур,
/// которые может трогать только CPU0, по-прежнему достаточно `IrqGuard`.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// Спинлок можно разделять между потоками/ядрами, если защищаемые данные Send.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(v: T) -> Self {
        Self { locked: AtomicBool::new(false), data: UnsafeCell::new(v) }
    }

    #[inline]
    pub fn lock(&self) -> SpinGuard<'_, T> {
        let daif = irq_save();
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinGuard { lock: self, daif }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<SpinGuard<'_, T>> {
        let daif = irq_save();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinGuard { lock: self, daif })
        } else {
            irq_restore(daif);
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

/// RAII-защита для [`SpinLock`].
pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
    daif: u64,
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        irq_restore(self.daif);
    }
}
