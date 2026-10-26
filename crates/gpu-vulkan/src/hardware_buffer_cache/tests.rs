//! Тесты кэша на fake-бэкенде, который пишет порядок создания и уничтожения импортов.
//! Они проверяют именно результат — что и когда реально уничтожено, — а не вызовы методов.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Event {
    Created(HardwareBufferId, ImportLayout),
    Destroyed(HardwareBufferId, ImportLayout),
}

type EventLog = Rc<RefCell<Vec<Event>>>;

/// «Импорт» fake-бэкенда: при `Drop` записывает уничтожение.
struct FakeImport {
    id: HardwareBufferId,
    layout: ImportLayout,
    log: EventLog,
}

impl FakeImport {
    fn new(log: &EventLog, id: HardwareBufferId, layout: ImportLayout) -> Self {
        log.borrow_mut().push(Event::Created(id, layout));
        Self {
            id,
            layout,
            log: Rc::clone(log),
        }
    }
}

impl Drop for FakeImport {
    fn drop(&mut self) {
        self.log.borrow_mut().push(Event::Destroyed(self.id, self.layout));
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FakeImportError(HardwareBufferId);

struct FakeBackend {
    log: EventLog,
    failing_ids: HashSet<HardwareBufferId>,
    import_calls: usize,
}

impl FakeBackend {
    fn new(log: &EventLog) -> Self {
        Self {
            log: Rc::clone(log),
            failing_ids: HashSet::new(),
            import_calls: 0,
        }
    }
}

impl ImportBackend for FakeBackend {
    type Source = ();
    type Import = FakeImport;
    type Error = FakeImportError;

    fn import(
        &mut self,
        id: HardwareBufferId,
        _source: &(),
        layout: ImportLayout,
    ) -> Result<FakeImport, FakeImportError> {
        self.import_calls += 1;
        if self.failing_ids.contains(&id) {
            return Err(FakeImportError(id));
        }
        Ok(FakeImport::new(&self.log, id, layout))
    }
}

const A: HardwareBufferId = HardwareBufferId(1);
const B: HardwareBufferId = HardwareBufferId(2);
const C: HardwareBufferId = HardwareBufferId(3);
const IMAGE: ImportLayout = ImportLayout::SampledImageR16Uint { width: 64, height: 32 };
const OTHER_IMAGE: ImportLayout = ImportLayout::SampledImageR16Uint { width: 128, height: 32 };
const PACKED: ImportLayout = ImportLayout::PackedBuffer {
    row_stride_bytes: 80,
    rows: 32,
};

fn serial(value: u64) -> SubmissionSerial {
    SubmissionSerial(value)
}

fn cache_with_capacity(capacity: usize) -> (HardwareBufferImportCache<FakeBackend>, EventLog) {
    let log: EventLog = Rc::default();
    let capacity = NonZeroUsize::new(capacity).expect("ёмкость теста ненулевая");
    (HardwareBufferImportCache::new(FakeBackend::new(&log), capacity), log)
}

fn outcome_of(
    cache: &mut HardwareBufferImportCache<FakeBackend>,
    id: HardwareBufferId,
    layout: ImportLayout,
    submission: u64,
) -> AcquireOutcome {
    cache
        .acquire(id, &(), layout, serial(submission))
        .expect("импорт fake-бэкенда успешен")
        .outcome
}

#[test]
fn second_request_for_same_buffer_is_a_hit_that_creates_nothing() {
    let (mut cache, log) = cache_with_capacity(4);
    assert_eq!(outcome_of(&mut cache, A, IMAGE, 1), AcquireOutcome::Miss);
    assert_eq!(outcome_of(&mut cache, A, IMAGE, 2), AcquireOutcome::Hit);
    assert_eq!(*log.borrow(), vec![Event::Created(A, IMAGE)]);
    assert_eq!(cache.backend().import_calls, 1, "хит не должен звать бэкенд");
    let counters = cache.counters();
    assert_eq!((counters.hits, counters.misses), (1, 1));
    assert_eq!(cache.len(), 1);
}

#[test]
fn acquire_returns_the_import_of_the_requested_buffer() {
    let (mut cache, _log) = cache_with_capacity(4);
    cache.acquire(A, &(), IMAGE, serial(1)).expect("импорт A");
    let acquired = cache.acquire(B, &(), PACKED, serial(2)).expect("импорт B");
    assert_eq!((acquired.import.id, acquired.import.layout), (B, PACKED));
}

#[test]
fn capacity_eviction_drops_least_recently_used_after_gpu_finished() {
    let (mut cache, log) = cache_with_capacity(2);
    outcome_of(&mut cache, A, IMAGE, 1);
    outcome_of(&mut cache, B, IMAGE, 2);
    // Хит обновляет давность: теперь самый давний — B.
    assert_eq!(outcome_of(&mut cache, A, IMAGE, 3), AcquireOutcome::Hit);
    cache.retire_completed(serial(3));
    outcome_of(&mut cache, C, IMAGE, 4);

    assert_eq!(
        *log.borrow(),
        vec![
            Event::Created(A, IMAGE),
            Event::Created(B, IMAGE),
            Event::Created(C, IMAGE),
            Event::Destroyed(B, IMAGE),
        ]
    );
    assert!(cache.contains(A) && cache.contains(C) && !cache.contains(B));
    let counters = cache.counters();
    assert_eq!(
        (counters.evictions, counters.deferred_evictions, counters.destroyed),
        (1, 0, 1)
    );
    assert_eq!(cache.len(), 2, "ёмкость не превышена");
}

#[test]
fn eviction_is_deferred_until_gpu_completes_the_last_use() {
    let (mut cache, log) = cache_with_capacity(1);
    outcome_of(&mut cache, A, IMAGE, 5);
    outcome_of(&mut cache, B, IMAGE, 6);

    // A вытеснен, но GPU мог ещё читать его (серия 5 не завершена): импорт жив.
    assert_eq!(*log.borrow(), vec![Event::Created(A, IMAGE), Event::Created(B, IMAGE)]);
    assert_eq!(cache.pending_destroy_count(), 1);
    assert_eq!(cache.counters().deferred_evictions, 1);

    assert_eq!(cache.retire_completed(serial(4)), 0, "серия 4 < 5: рано");
    assert_eq!(cache.pending_destroy_count(), 1);
    assert_eq!(log.borrow().len(), 2);

    assert_eq!(cache.retire_completed(serial(5)), 1);
    assert_eq!(log.borrow().last(), Some(&Event::Destroyed(A, IMAGE)));
    assert_eq!(cache.pending_destroy_count(), 0);
    assert_eq!(cache.counters().destroyed, 1);
}

#[test]
fn stale_completion_report_does_not_move_completed_serial_back() {
    let (mut cache, log) = cache_with_capacity(1);
    outcome_of(&mut cache, A, IMAGE, 3);
    cache.retire_completed(serial(10));
    cache.retire_completed(serial(1)); // запоздавшее сообщение
    outcome_of(&mut cache, B, IMAGE, 11);
    // GPU уже дошёл до 10 ≥ 3, поэтому A уничтожен сразу, а не отложен.
    assert_eq!(log.borrow().last(), Some(&Event::Destroyed(A, IMAGE)));
    assert_eq!(cache.counters().deferred_evictions, 0);
}

#[test]
fn last_use_never_moves_backwards() {
    let (mut cache, log) = cache_with_capacity(1);
    outcome_of(&mut cache, A, IMAGE, 7);
    outcome_of(&mut cache, A, IMAGE, 3); // поздний хит со старой серией не должен «омолодить» защиту
    outcome_of(&mut cache, B, IMAGE, 8);
    cache.retire_completed(serial(5));
    assert!(
        !log.borrow().contains(&Event::Destroyed(A, IMAGE)),
        "A использовался в серии 7"
    );
    cache.retire_completed(serial(7));
    assert!(log.borrow().contains(&Event::Destroyed(A, IMAGE)));
}

#[test]
fn layout_change_reimports_and_defers_destroying_the_old_import() {
    let (mut cache, log) = cache_with_capacity(4);
    outcome_of(&mut cache, A, IMAGE, 1);
    assert_eq!(outcome_of(&mut cache, A, OTHER_IMAGE, 2), AcquireOutcome::LayoutChanged);

    assert_eq!(
        *log.borrow(),
        vec![Event::Created(A, IMAGE), Event::Created(A, OTHER_IMAGE)],
        "старый импорт жив: GPU мог читать его в серии 1"
    );
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.pending_destroy_count(), 1);
    let counters = cache.counters();
    assert_eq!(
        (counters.misses, counters.layout_reimports, counters.evictions),
        (2, 1, 1)
    );

    cache.retire_completed(serial(1));
    assert_eq!(log.borrow().last(), Some(&Event::Destroyed(A, IMAGE)));
    // Новая раскладка теперь — хит.
    assert_eq!(outcome_of(&mut cache, A, OTHER_IMAGE, 3), AcquireOutcome::Hit);
}

#[test]
fn failed_import_is_not_cached_and_disturbs_nobody() {
    let (mut cache, log) = cache_with_capacity(1);
    outcome_of(&mut cache, A, IMAGE, 1);
    cache.retire_completed(serial(1));
    cache.backend_mut_for_test().failing_ids.insert(B);

    let error = cache.acquire(B, &(), IMAGE, serial(2)).map(|_| ()).unwrap_err();
    assert_eq!(error, FakeImportError(B));
    assert!(cache.contains(A), "соседа не вытеснили ради неудачного импорта");
    assert!(!cache.contains(B));
    assert_eq!(*log.borrow(), vec![Event::Created(A, IMAGE)]);
    let counters = cache.counters();
    // misses = 1 — это первый успешный импорт A; неудачный импорт B промахом не считается.
    assert_eq!(
        (counters.import_failures, counters.misses, counters.evictions),
        (1, 1, 0)
    );

    // Повтор снова идёт в бэкенд (отказ не запомнен), а когда бэкенд починился — успех.
    cache.backend_mut_for_test().failing_ids.clear();
    assert_eq!(outcome_of(&mut cache, B, IMAGE, 3), AcquireOutcome::Miss);
}

#[test]
fn failed_reimport_keeps_the_previous_import_of_that_buffer() {
    let (mut cache, log) = cache_with_capacity(2);
    outcome_of(&mut cache, A, IMAGE, 1);
    cache.backend_mut_for_test().failing_ids.insert(A);
    assert!(cache.acquire(A, &(), OTHER_IMAGE, serial(2)).is_err());
    assert!(cache.contains(A));
    assert_eq!(*log.borrow(), vec![Event::Created(A, IMAGE)]);
    assert_eq!(cache.counters().layout_reimports, 0);
}

#[test]
fn clear_refuses_while_gpu_may_still_use_imports_and_changes_nothing() {
    let (mut cache, log) = cache_with_capacity(2);
    outcome_of(&mut cache, A, IMAGE, 4);
    outcome_of(&mut cache, B, PACKED, 6);

    let error = cache.clear(serial(5)).unwrap_err();
    assert_eq!(
        error,
        ClearError::GpuStillUsing {
            newest_use: serial(6),
            completed: serial(5)
        }
    );
    assert_eq!(cache.len(), 2, "отказ не должен ничего менять");
    assert_eq!(log.borrow().len(), 2, "и ничего не уничтожать");

    assert_eq!(cache.clear(serial(6)), Ok(2));
    assert!(cache.is_empty());
    assert_eq!(
        log.borrow()[2..].iter().copied().collect::<HashSet<_>>().len(),
        2,
        "уничтожены оба импорта"
    );
    assert_eq!(cache.counters().destroyed, 2);
}

#[test]
fn clear_also_waits_for_pending_evictions() {
    let (mut cache, log) = cache_with_capacity(1);
    outcome_of(&mut cache, A, IMAGE, 9);
    outcome_of(&mut cache, B, IMAGE, 2); // A ждёт уничтожения (серия 9)
    assert!(matches!(cache.clear(serial(5)), Err(ClearError::GpuStillUsing { .. })));
    assert_eq!(cache.clear(serial(9)), Ok(2));
    assert!(log.borrow().contains(&Event::Destroyed(A, IMAGE)));
    assert!(log.borrow().contains(&Event::Destroyed(B, IMAGE)));
}

#[test]
fn clear_on_empty_cache_is_a_noop_even_with_zero_completed() {
    let (mut cache, log) = cache_with_capacity(2);
    assert_eq!(cache.clear(serial(0)), Ok(0));
    assert!(log.borrow().is_empty());
    assert_eq!(cache.counters(), ImportCacheCounters::default());
}

#[test]
fn cache_never_destroys_imports_it_does_not_own() {
    let (mut cache, log) = cache_with_capacity(1);
    // «Чужой» импорт того же бэкенда живёт отдельно от кэша.
    let outsider = FakeImport::new(&log, HardwareBufferId(999), IMAGE);

    outcome_of(&mut cache, A, IMAGE, 1);
    outcome_of(&mut cache, B, IMAGE, 2);
    cache.retire_completed(serial(2));
    cache.clear(serial(2)).expect("GPU закончил");

    let destroyed_outsider = log.borrow().contains(&Event::Destroyed(HardwareBufferId(999), IMAGE));
    assert!(!destroyed_outsider, "кэш уничтожил чужой импорт");
    drop(outsider);
    assert_eq!(
        log.borrow().last(),
        Some(&Event::Destroyed(HardwareBufferId(999), IMAGE))
    );
    // Бэкенд вызывался ровно по числу промахов: кэш не импортирует «на всякий случай».
    assert_eq!(cache.backend().import_calls as u64, cache.counters().misses);
}

#[test]
fn dropping_the_cache_releases_every_import() {
    let (cache, log) = {
        let (mut cache, log) = cache_with_capacity(2);
        outcome_of(&mut cache, A, IMAGE, 1);
        outcome_of(&mut cache, B, PACKED, 2);
        (cache, log)
    };
    drop(cache);
    let destroyed = log
        .borrow()
        .iter()
        .filter(|event| matches!(event, Event::Destroyed(..)))
        .count();
    assert_eq!(destroyed, 2);
}

impl HardwareBufferImportCache<FakeBackend> {
    /// Изменяемый доступ к fake-бэкенду только для тестов (в продуктовом API его нет).
    fn backend_mut_for_test(&mut self) -> &mut FakeBackend {
        &mut self.backend
    }
}
