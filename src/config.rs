//! Центральная конфигурация лимитов ядра.
//! Регулируй под целевую платформу; значения взяты с запасом по сравнению с исходными 8/16.

pub mod limits {
    pub const MAX_PROCS: usize = 64;          // было 8
    pub const MAX_THREADS: usize = 128;       // было 16
    pub const MAX_CAPS_PER_PROC: usize = 16;  // было 8
    pub const MAX_IPC_ENDPOINTS: usize = 64;  // было 8
    pub const MAX_IPC_SENDQ: usize = 32;      // было 8
    pub const MAX_PMM_RESERVES: usize = 256;  // было 80
    pub const MMU_TABLE_POOL: usize = 256;    // было 96
    pub const USER_STACK_PAGES: usize = 16;
    pub const KERNEL_STACK_PAGES: usize = 32;
    pub const SLICE_TICKS: u32 = 5;
    pub const IPC_DEFAULT_TIMEOUT_TICKS: u64 = 1000;
}
