//! Объекты разделяемой памяти: набор физических страниц, которые можно отобразить в несколько процессов.
//! Время жизни контролируется refcountом в единой таблице obj::OBJS; таблица SHMS хранит только
//! физические страницы и их число (индексируется по obj_payload(obj_id)).
//! Когда refs объекта доходит до 0, obj::obj_release вызывает shm_cleanup, который освобождает страницы.
use crate::{
    obj,
    pmm,
    sync::{IrqGuard, SpinLock},
};

pub const MAX_SHM: usize = 64;
pub const MAX_PAGES: usize = 64;

#[derive(Clone, Copy)]
struct Shm {
    used: bool,
    n: usize,
    pages: [usize; MAX_PAGES],
}

const EMPTY: Shm = Shm { used: false, n: 0, pages: [0; MAX_PAGES] };
static SHMS: SpinLock<[Shm; MAX_SHM]> = SpinLock::new([EMPTY; MAX_SHM]);

/// Освобождает страницы и помечает слот SHMS свободным.
fn shm_cleanup(shm_index: usize) {
    if shm_index >= MAX_SHM {
        return;
    }
    let _g = IrqGuard::new();
    let mut shms = SHMS.lock();
    let s = &mut shms[shm_index];
    if s.used {
        for &p in s.pages.iter().take(s.n) {
            pmm::free_page(p);
        }
        *s = EMPTY;
    }
}

/// Инициализация: регистрирует коллбэк очистки для объектов Shm.
pub fn init() {
    obj::obj_register_cleanup(obj::ObjType::Shm, shm_cleanup);
}

/// Создаёт объект из `n` (1..=MAX_PAGES) обнулённых страниц.
/// obj_alloc выделяет объект с refs=1 (ссылка создателя); после `cap_install`
/// создатель обязан освободить её через `obj::obj_release`.
/// owner_pid — процесс, создавший объект (этот же процесс и владелец).
pub fn create(owner_pid: usize, n: usize) -> Option<usize> {
    if n == 0 || n > MAX_PAGES {
        return None;
    }
    // Слот резервируется под локом ДО выделения страниц: иначе на SMP два ядра
    // могут выбрать один и тот же индекс и затереть данные друг друга.
    let shm_index = {
        let mut shms = SHMS.lock();
        let i = (0..MAX_SHM).find(|&i| !shms[i].used)?;
        shms[i] = Shm { used: true, n: 0, pages: [0; MAX_PAGES] };
        i
    };
    let mut pages = [0usize; MAX_PAGES];
    for k in 0..n {
        match pmm::alloc_page() {
            Some(p) => pages[k] = p,
            None => {
                for &p in pages.iter().take(k) {
                    pmm::free_page(p);
                }
                SHMS.lock()[shm_index] = EMPTY;
                return None;
            }
        }
    }
    {
        let mut shms = SHMS.lock();
        shms[shm_index] = Shm { used: true, n, pages };
    }
    match obj::obj_alloc(obj::ObjType::Shm, owner_pid, shm_index) {
        Some(obj_id) => Some(obj_id),
        None => {
            // Объект не создан — освобождаем зарезервированные страницы и слот.
            let mut shms = SHMS.lock();
            let s = &mut shms[shm_index];
            for &p in s.pages.iter().take(s.n) {
                pmm::free_page(p);
            }
            *s = EMPTY;
            None
        }
    }
}

pub fn get(obj_id: usize) -> Option<([usize; MAX_PAGES], usize)> {
    let shm_index = obj::obj_payload(obj_id)?;
    if shm_index >= MAX_SHM {
        return None;
    }
    let _g = IrqGuard::new();
    let shms = SHMS.lock();
    let s = shms[shm_index];
    if s.used {
        Some((s.pages, s.n))
    } else {
        None
    }
}
