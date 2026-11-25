//! Частота записи памяти белой: не чаще одной записи за интервал, последнее значение не теряется и ложится
//! вовремя, `flush` и закрытие пишут сразу. Часть тестов — на заглушке с отметками времени, часть — на настоящем
//! файле («протянул шкалу, процесс убили, загрузили»).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use camera_core::white::{RememberedOrigin, WhiteMemory, WhiteMemoryLoad, WhiteMemoryRecord};

use super::tests::{app_directory, record, wait_until};
use super::*;

/// Одна запись, дошедшая до «диска» заглушки: когда и что.
type WriteLog = Arc<Mutex<Vec<(Instant, WhiteMemoryRecord)>>>;

/// Приёмник-заглушка: запоминает момент каждой записи.
struct TimedSink {
    writes: WriteLog,
}

impl WhiteRecordSink for TimedSink {
    fn write_record(&mut self, record: &WhiteMemoryRecord) -> Result<WriteCommit, WhiteMemoryError> {
        self.writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Instant::now(), *record));
        Ok(WriteCommit::Durable)
    }
}

fn writer_with_interval(interval: Duration) -> (WhiteMemoryWriter, WriteLog) {
    let writes = WriteLog::default();
    let writer = WhiteMemoryWriter::spawn(
        TimedSink {
            writes: Arc::clone(&writes),
        },
        WhiteMemoryDiagnostics::default(),
        interval,
    )
    .expect("поток");
    (writer, writes)
}

fn written_records(writes: &WriteLog) -> Vec<WhiteMemoryRecord> {
    writes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .map(|(_, record)| *record)
        .collect()
}

#[test]
fn a_hundred_saves_in_a_row_leave_the_last_value_on_disk_with_few_writes_spaced_by_the_interval() {
    let interval = Duration::from_millis(50);
    let (mut writer, writes) = writer_with_interval(interval);
    let started = Instant::now();
    let mut last = record(3_000.0, 0.0, RememberedOrigin::Manual);
    for step in 0..100_u32 {
        last = record(3_000.0 + f64::from(step) * 10.0, 0.0, RememberedOrigin::Manual);
        writer.save(last);
        // Палец движется не мгновенно: без пауз поток записи мог бы вовсе не успеть проснуться, и тест ничего не доказал бы.
        std::thread::sleep(Duration::from_millis(1));
    }
    let drag_took = started.elapsed();

    wait_until("последнее значение на диске", || {
        written_records(&writes).last() == Some(&last)
    });

    let log = writes.lock().unwrap_or_else(PoisonError::into_inner).clone();
    // Первая запись — сразу, остальные — не чаще интервала: за время серии их не больше «1 + интервалов + хвост».
    let allowed = 2 + usize::try_from(drag_took.as_millis() / interval.as_millis()).expect("влезает");
    assert!(
        log.len() <= allowed,
        "записей {} при {} допустимых за {drag_took:?}",
        log.len(),
        allowed
    );
    assert!(
        log.len() < 20,
        "без ограничения частоты было бы десятки записей: {}",
        log.len()
    );
    for pair in log.windows(2) {
        assert!(
            pair[1].0.duration_since(pair[0].0) >= interval,
            "две записи ближе интервала"
        );
    }
    assert_eq!(
        log.last().map(|(_, record)| record.origin),
        Some(RememberedOrigin::Manual)
    );
}

#[test]
fn the_last_value_lands_after_the_interval_even_without_new_events() {
    let interval = Duration::from_millis(150);
    let (mut writer, writes) = writer_with_interval(interval);
    let first = record(3_000.0, 0.0, RememberedOrigin::Manual);
    let second = record(4_000.0, 0.0, RememberedOrigin::Manual);

    writer.save(first);
    wait_until("первая запись сразу", || {
        written_records(&writes).len() == 1
    });
    writer.save(second);
    let saved_second_at = Instant::now();
    // Новых событий нет: вторая должна лечь сама.
    wait_until("вторая запись по сроку", || {
        written_records(&writes).len() == 2
    });

    let log = writes.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(written_records(&writes), vec![first, second]);
    assert!(
        log[1].0.duration_since(log[0].0) >= interval,
        "раньше срока писать нельзя"
    );
    assert!(
        log[1].0.duration_since(saved_second_at) < Duration::from_secs(2),
        "и позже срока (с запасом на планировщик) тоже"
    );
}

#[test]
fn flush_writes_the_pending_value_without_waiting_for_the_interval_and_is_not_sticky() {
    let (mut writer, writes) = writer_with_interval(Duration::from_secs(60));
    let first = record(3_000.0, 0.0, RememberedOrigin::Manual);
    let pending = record(4_000.0, 0.0, RememberedOrigin::Manual);
    let after_flush = record(5_000.0, 0.0, RememberedOrigin::Manual);

    writer.save(first);
    wait_until("первая запись сразу", || {
        written_records(&writes).len() == 1
    });
    writer.save(pending);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(written_records(&writes), vec![first], "до интервала вторая ждёт");

    writer.flush();
    wait_until("flush положил значение", || {
        written_records(&writes).len() == 2
    });
    assert_eq!(written_records(&writes), vec![first, pending]);

    // flush без принятого значения ничего не взводит: следующая запись снова ждёт интервал.
    writer.flush();
    writer.save(after_flush);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(written_records(&writes), vec![first, pending], "флаг «сразу» не залип");
}

#[test]
fn closing_the_writer_writes_the_pending_value_at_once() {
    let (mut writer, writes) = writer_with_interval(Duration::from_secs(60));
    let first = record(3_000.0, 0.0, RememberedOrigin::Manual);
    let pending = record(4_000.0, 0.0, RememberedOrigin::Source);
    writer.save(first);
    wait_until("первая запись сразу", || {
        written_records(&writes).len() == 1
    });
    writer.save(pending);

    let closing_started = Instant::now();
    drop(writer);

    assert_eq!(written_records(&writes), vec![first, pending]);
    assert!(
        closing_started.elapsed() < Duration::from_secs(5),
        "закрытие не ждёт интервала (он 60 с)"
    );
}

#[test]
fn a_scale_drag_followed_by_flush_survives_a_kill_and_without_flush_only_a_drop_saves_it() {
    let (directory, path) = app_directory();
    let config = WhiteMemoryConfig {
        min_write_interval_ms: 5_000,
        ..WhiteMemoryConfig::default()
    };
    let mut last = record(3_000.0, 0.0, RememberedOrigin::Manual);

    // Протянули шкалу, приложение ушло в фон (flush), процесс убили: Drop не выполнился (`forget`).
    let mut opened = open_white_memory(&directory, &config);
    for step in 0..100_u32 {
        last = record(3_000.0 + f64::from(step) * 10.0, -0.004, RememberedOrigin::Manual);
        opened.parts.memory.save(last);
    }
    opened.parts.memory.flush();
    wait_until(
        "flush положил последнее значение на диск",
        || WhiteMemoryFile::open(&path, &config).load == WhiteMemoryLoad::Remembered(last),
    );
    std::mem::forget(opened.parts.memory);
    assert_eq!(
        open_white_memory(&directory, &config).parts.load,
        WhiteMemoryLoad::Remembered(last),
        "после «убийства» на диске последнее значение с происхождением manual"
    );

    // Без flush: пока процесс жив, диск отстаёт на интервал; чистая остановка (Drop) всё равно дописывает.
    let newer = record(7_000.0, 0.01, RememberedOrigin::Manual);
    let mut second_run = open_white_memory(&directory, &config);
    second_run
        .parts
        .memory
        .save(record(6_000.0, 0.0, RememberedOrigin::Manual));
    let second_diagnostics = second_run.diagnostics.clone();
    wait_until("первая запись второго запуска", || {
        second_diagnostics.snapshot().saved == 1
    });
    second_run.parts.memory.save(newer);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        second_diagnostics.snapshot().saved,
        1,
        "до интервала вторая запись не пишется"
    );
    drop(second_run);
    assert_eq!(
        open_white_memory(&directory, &config).parts.load,
        WhiteMemoryLoad::Remembered(newer)
    );
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn the_interval_is_validated_and_comes_from_json() {
    assert_eq!(WhiteMemoryConfig::default().min_write_interval_ms, 250);
    assert_eq!(
        WhiteMemoryConfig::default().min_write_interval(),
        Duration::from_millis(250)
    );
    for bad in [0, WhiteMemoryConfig::MAX_WRITE_INTERVAL_MS + 1] {
        let config = WhiteMemoryConfig {
            min_write_interval_ms: bad,
            ..WhiteMemoryConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(WhiteMemoryConfigError::InvalidWriteInterval { value: bad })
        );
    }
    let parsed: WhiteMemoryConfig = serde_json::from_str(r#"{"min_write_interval_ms":400}"#).expect("разбор");
    assert_eq!(parsed.min_write_interval(), Duration::from_millis(400));
    assert_eq!(parsed.validate(), Ok(()));
}

/// Перехватчик событий `tracing`: складывает уровень и «поле=значение» каждого события в одну строку.
struct CaptureEvents {
    lines: Arc<Mutex<Vec<String>>>,
}

/// Собирает поля события в строку.
struct FieldText(String);

impl tracing::field::Visit for FieldText {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl tracing::Subscriber for CaptureEvents {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = FieldText(format!("{} ", event.metadata().level()));
        event.record(&mut fields);
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).push(fields.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[test]
fn a_disk_write_logs_the_value_and_origin_at_info() {
    let lines = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(
        CaptureEvents {
            lines: Arc::clone(&lines),
        },
        || super::writer::log_written(&record(3_150.0, -0.012, RememberedOrigin::Manual)),
    );
    let lines = lines.lock().unwrap_or_else(PoisonError::into_inner).clone();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(line.starts_with("INFO "), "{line}");
    assert!(line.contains("память белой записана"), "{line}");
    assert!(line.contains("kelvin=3150.0"), "{line}");
    assert!(line.contains("duv=-0.012"), "{line}");
    assert!(line.contains("origin=Manual"), "{line}");
}
