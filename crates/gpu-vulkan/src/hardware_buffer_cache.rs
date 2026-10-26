//! Кэш импортов `AHardwareBuffer` в Vulkan — нейтральная логика (тестируется на ПК).
//!
//! Зачем кэш: импорт AHB (image/buffer + `vkAllocateMemory` с импортом) стоит дорого, а камера
//! гоняет по кругу небольшой набор одних и тех же буферов. Ключ — `AHardwareBuffer_getId` (API 31):
//! тот же буфер → тот же импорт.
//!
//! Кэш не знает, что именно импортируется: это трейт [`ImportBackend`] (на Android — Vulkan-импорт,
//! в тестах — fake). Это отделяет политику (LRU, отложенное уничтожение, счётчики) от Vulkan.
//!
//! Главный инвариант: **импорт, который GPU ещё может читать, не уничтожается никогда.**
//! Каждая запись помнит серийный номер последней отправки на GPU, где её использовали
//! ([`SubmissionSerial`]); вытесненная запись уходит в очередь отложенного уничтожения и
//! освобождается только когда `completed_serial >= last_use`. Освобождение делает сам владелец
//! кэша вызовом [`HardwareBufferImportCache::retire_completed`], когда узнал, что GPU дошёл до серии.
//!
//! Модули:
//! * [`layout`] — раскладки импорта и наши проверки размеров против `allocationSize`;
//! * [`raw_reference`] — образец RAW-кадра и CPU-эталон контрольных сумм;
//! * [`checksum_gpu`] — compute-запуск контрольных сумм над image/buffer (общий для ПК и телефона);
//! * [`queue_handoff`] — барьеры передачи владения очередью FOREIGN ↔ наша (sync2).

use std::collections::HashMap;
use std::num::NonZeroUsize;

pub mod checksum_gpu;
pub mod layout;
pub mod queue_handoff;
pub mod raw_reference;

pub use self::layout::ImportLayout;

/// Стабильный идентификатор `AHardwareBuffer` (`AHardwareBuffer_getId`), ключ кэша.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HardwareBufferId(pub u64);

/// Серийный номер отправки на GPU. Растёт монотонно; чем больше, тем позже отправлено.
/// Владелец очереди знает, до какой серии GPU уже дошёл (`completed`), и сообщает кэшу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SubmissionSerial(pub u64);

/// Бэкенд импорта: создаёт «импорт» (то, что освобождается в `Drop`) для буфера.
pub trait ImportBackend {
    /// Источник данных для импорта (на Android — ссылка на AHB; в тестах — `()`).
    type Source;
    /// Готовый импорт. Освобождение — в его `Drop`; кэш гарантирует, что `Drop` вызывается
    /// только когда GPU закончил с импортом.
    type Import;
    /// Ошибка импорта (на Android — типизированная `HardwareBufferImportError`).
    type Error;

    /// Создаёт импорт `source` с раскладкой `layout`.
    fn import(
        &mut self,
        id: HardwareBufferId,
        source: &Self::Source,
        layout: ImportLayout,
    ) -> Result<Self::Import, Self::Error>;
}

/// Счётчики кэша (для диагностики и тестов).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportCacheCounters {
    /// Запрос нашёл импорт с той же раскладкой.
    pub hits: u64,
    /// Запрос потребовал нового импорта (включая переимпорт при смене раскладки).
    pub misses: u64,
    /// Записей вытеснено (по ёмкости или из-за смены раскладки), всего.
    pub evictions: u64,
    /// Из них уничтожение пришлось отложить: GPU ещё мог читать импорт.
    pub deferred_evictions: u64,
    /// Импорт не удался (в кэш не попадает).
    pub import_failures: u64,
    /// Переимпортов из-за смены раскладки для того же id (подмножество `misses`).
    pub layout_reimports: u64,
    /// Импортов, реально уничтоженных (drop) кэшем.
    pub destroyed: u64,
}

/// Чем закончился запрос.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireOutcome {
    /// Взят готовый импорт.
    Hit,
    /// Создан новый импорт.
    Miss,
    /// Раскладка того же буфера сменилась: старый импорт вытеснен, создан новый.
    LayoutChanged,
}

/// Ответ на запрос: ссылка на импорт и как он получен.
#[derive(Debug)]
pub struct Acquired<'cache, Import> {
    /// Импорт; жив, пока кэш не изменён (заимствование).
    pub import: &'cache Import,
    /// Хит / промах / смена раскладки.
    pub outcome: AcquireOutcome,
}

/// Ошибка [`HardwareBufferImportCache::clear`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearError {
    /// GPU ещё может использовать импорты (последняя отправка новее `completed`). Кэш не изменён.
    GpuStillUsing {
        /// Самая поздняя серия, где использовался какой-либо импорт.
        newest_use: SubmissionSerial,
        /// Серия, до которой GPU дошёл по сообщению вызывающего.
        completed: SubmissionSerial,
    },
}

/// Запись кэша.
struct Entry<Import> {
    import: Import,
    layout: ImportLayout,
    /// Серия последней отправки, использовавшей импорт.
    last_use: SubmissionSerial,
    /// Тик последнего обращения (для LRU).
    recency: u64,
}

/// Вытесненный импорт, ждущий конца работы GPU.
struct PendingDestroy<Import> {
    /// Хранится ради `Drop`: освобождение импорта и есть его уничтожение.
    _import: Import,
    last_use: SubmissionSerial,
}

/// LRU-кэш импортов с отложенным уничтожением.
pub struct HardwareBufferImportCache<Backend: ImportBackend> {
    backend: Backend,
    capacity: NonZeroUsize,
    entries: HashMap<HardwareBufferId, Entry<Backend::Import>>,
    pending: Vec<PendingDestroy<Backend::Import>>,
    /// Серия, до которой GPU дошёл (по последнему сообщению владельца).
    completed: SubmissionSerial,
    recency_clock: u64,
    counters: ImportCacheCounters,
}

impl<Backend: ImportBackend> HardwareBufferImportCache<Backend> {
    /// Создаёт пустой кэш на `capacity` записей (живых, не считая ожидающих уничтожения).
    pub fn new(backend: Backend, capacity: NonZeroUsize) -> Self {
        Self {
            backend,
            capacity,
            entries: HashMap::new(),
            pending: Vec::new(),
            completed: SubmissionSerial::default(),
            recency_clock: 0,
            counters: ImportCacheCounters::default(),
        }
    }

    /// Возвращает импорт буфера `id` для использования в отправке с серией `submission`.
    ///
    /// * хит с той же раскладкой — отдаёт готовый импорт;
    /// * промах — импортирует через бэкенд; при ошибке в кэш ничего не попадает и никого не
    ///   вытесняют (сначала импорт, потом вытеснение);
    /// * другая раскладка того же `id` — старый импорт вытесняется (отложенно, если нужно),
    ///   создаётся новый.
    ///
    /// `submission` записывается как «последнее использование»: вытеснение этой записи будет отложено
    /// до `completed >= submission`. Вызывать нужно ДО отправки, с серией, которую получит эта отправка.
    pub fn acquire(
        &mut self,
        id: HardwareBufferId,
        source: &Backend::Source,
        layout: ImportLayout,
        submission: SubmissionSerial,
    ) -> Result<Acquired<'_, Backend::Import>, Backend::Error> {
        self.recency_clock += 1;
        let recency = self.recency_clock;

        let outcome = match self.entries.get(&id).map(|entry| entry.layout == layout) {
            Some(true) => AcquireOutcome::Hit,
            Some(false) => AcquireOutcome::LayoutChanged,
            None => AcquireOutcome::Miss,
        };

        if outcome == AcquireOutcome::Hit {
            self.counters.hits += 1;
        } else {
            // Сначала новый импорт: если он упадёт, старая запись и соседи остаются нетронутыми.
            let import = match self.backend.import(id, source, layout) {
                Ok(import) => import,
                Err(error) => {
                    self.counters.import_failures += 1;
                    return Err(error);
                }
            };
            self.counters.misses += 1;
            if outcome == AcquireOutcome::LayoutChanged {
                self.counters.layout_reimports += 1;
                if let Some(stale) = self.entries.remove(&id) {
                    self.evict(stale);
                }
            } else {
                self.make_room_for_one_more();
            }
            self.entries.insert(
                id,
                Entry {
                    import,
                    layout,
                    last_use: submission,
                    recency,
                },
            );
        }

        // Запись гарантированно есть: либо была (хит), либо только что вставлена.
        let Some(entry) = self.entries.get_mut(&id) else {
            unreachable_entry_missing()
        };
        entry.recency = recency;
        entry.last_use = entry.last_use.max(submission);
        Ok(Acquired {
            import: &entry.import,
            outcome,
        })
    }

    /// Сообщает кэшу, что GPU закончил все отправки до серии `completed` включительно, и
    /// уничтожает вытесненные импорты, которые больше не нужны GPU. Возвращает число уничтоженных.
    ///
    /// Серия не может уменьшаться: меньшее значение игнорируется (устаревшее сообщение).
    pub fn retire_completed(&mut self, completed: SubmissionSerial) -> usize {
        self.completed = self.completed.max(completed);
        let completed = self.completed;
        let before = self.pending.len();
        // `retain` роняет (drop) удаляемые импорты — это и есть уничтожение Vulkan-объектов.
        self.pending.retain(|pending| pending.last_use > completed);
        let destroyed = before - self.pending.len();
        self.counters.destroyed += destroyed as u64;
        destroyed
    }

    /// Уничтожает ВСЕ импорты (живые и ожидающие). Допустимо, только если GPU закончил со всем:
    /// иначе — [`ClearError::GpuStillUsing`], кэш остаётся нетронутым.
    pub fn clear(&mut self, completed: SubmissionSerial) -> Result<usize, ClearError> {
        let completed = self.completed.max(completed);
        let newest_use = self
            .entries
            .values()
            .map(|entry| entry.last_use)
            .chain(self.pending.iter().map(|pending| pending.last_use))
            .max();
        if let Some(newest_use) = newest_use
            && newest_use > completed
        {
            return Err(ClearError::GpuStillUsing { newest_use, completed });
        }
        self.completed = completed;
        let destroyed = self.entries.len() + self.pending.len();
        self.entries.clear();
        self.pending.clear();
        self.counters.destroyed += destroyed as u64;
        Ok(destroyed)
    }

    /// Живых записей.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Нет живых записей.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Вытесненных импортов, ожидающих конца работы GPU.
    pub fn pending_destroy_count(&self) -> usize {
        self.pending.len()
    }

    /// Ёмкость (живых записей).
    pub fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    /// Есть ли живая запись для буфера (не меняет LRU и счётчики).
    pub fn contains(&self, id: HardwareBufferId) -> bool {
        self.entries.contains_key(&id)
    }

    /// Счётчики.
    pub fn counters(&self) -> ImportCacheCounters {
        self.counters
    }

    /// Бэкенд (для диагностики/тестов; кэш им владеет).
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// Освобождает место под одну новую запись: пока живых ≥ ёмкости, вытесняет самую давнюю.
    fn make_room_for_one_more(&mut self) {
        while self.entries.len() >= self.capacity.get() {
            let Some(oldest_id) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.recency)
                .map(|(id, _)| *id)
            else {
                return;
            };
            if let Some(victim) = self.entries.remove(&oldest_id) {
                self.evict(victim);
            }
        }
    }

    /// Вытесняет запись: уничтожает сразу, если GPU уже закончил, иначе откладывает.
    fn evict(&mut self, victim: Entry<Backend::Import>) {
        self.counters.evictions += 1;
        if victim.last_use <= self.completed {
            self.counters.destroyed += 1;
            drop(victim.import);
        } else {
            self.counters.deferred_evictions += 1;
            self.pending.push(PendingDestroy {
                _import: victim.import,
                last_use: victim.last_use,
            });
        }
    }
}

/// Недостижимая ветка: запись только что вставлена или найдена. Вынесено в функцию, чтобы
/// паника (нарушение внутреннего инварианта) была одна и объяснённая.
#[expect(clippy::panic, reason = "внутренний инвариант: запись есть сразу после вставки/хита")]
fn unreachable_entry_missing() -> ! {
    panic!("кэш импортов: запись пропала сразу после вставки — нарушен внутренний инвариант")
}

#[cfg(test)]
mod tests;
