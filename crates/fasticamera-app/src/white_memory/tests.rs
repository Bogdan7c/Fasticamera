//! Память белой на настоящих файлах во временной папке и на заглушках приёмника: перезапись, склейка, падение
//! записи, дописывание при закрытии, чужой/новый/битый/пустой файл, отсутствие каталога, режим `none`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_core::white::{RememberedOrigin, WhiteMemory, WhiteMemoryLoad, WhiteMemoryRecord};

use super::*;

static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

/// Новая пустая папка приложения (каждый тест — своя).
pub(super) fn app_directory() -> (AppDirectory, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "fasticamera-white-memory-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("папка создана");
    let directory = AppDirectory::new(path.to_string_lossy().into_owned()).expect("папка годится");
    (directory, path)
}

pub(super) fn record(kelvin: f64, duv: f64, origin: RememberedOrigin) -> WhiteMemoryRecord {
    WhiteMemoryRecord {
        setting: WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
            tint: Tint::from_duv(duv).expect("оттенок"),
        },
        origin,
    }
}

/// Ждёт условия до 5 с (поток записи работает сам, ждать приходится его).
pub(super) fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "не дождались: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Файлы-карантины в папке: `<имя>.corrupt-<мс>`.
fn quarantined_files(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .expect("папка читается")
        .map(|entry| entry.expect("запись").path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains(QUARANTINE_FILE_MARKER))
        })
        .collect()
}

fn open(directory: &AppDirectory) -> OpenedWhiteMemory {
    open_white_memory(directory, &WhiteMemoryConfig::default())
}

// --- Файл: запись, перезапуск ---

#[test]
fn a_first_launch_remembers_nothing_and_creates_no_file() {
    let (directory, path) = app_directory();
    let opened = open(&directory);
    assert_eq!(opened.parts.load, WhiteMemoryLoad::NothingRemembered);
    assert!(!path.join("white.json").exists());
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_saved_record_survives_the_restart_with_the_same_value_and_origin() {
    let (directory, path) = app_directory();
    let manual = record(3_150.0, -0.012, RememberedOrigin::Manual);
    {
        let mut opened = open(&directory);
        opened.parts.memory.save(manual);
        // Явного закрытия нет: дописывает Drop (так же, как при остановке движка).
    }

    let reopened = open(&directory);

    assert_eq!(reopened.parts.load, WhiteMemoryLoad::Remembered(manual));
    let text = std::fs::read_to_string(path.join("white.json")).expect("файл есть");
    assert!(text.contains("\"schema_version\": 1"), "{text}");
    assert!(text.contains("\"origin\": \"manual\""), "{text}");
    assert!(
        !path.join("white.json.tmp").exists(),
        "временный файл переименован, а не оставлен"
    );
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn overwriting_keeps_only_the_latest_record_and_both_origins_round_trip() {
    let (directory, path) = app_directory();
    let from_source = record(2_800.0, 0.004, RememberedOrigin::Source);
    {
        let mut opened = open(&directory);
        opened.parts.memory.save(record(6_500.0, 0.0, RememberedOrigin::Manual));
        let diagnostics = opened.diagnostics.clone();
        wait_until("первая запись на диске", || {
            diagnostics.snapshot().saved == 1
        });
        opened.parts.memory.save(from_source);
    }

    assert_eq!(open(&directory).parts.load, WhiteMemoryLoad::Remembered(from_source));
    std::fs::remove_dir_all(path).expect("уборка");
}

// --- Поток записи на заглушке: склейка, падение, Drop ---

/// Приёмник-заглушка: первая запись ждёт «ворота», все записи копятся в списке, ошибки задаются заранее.
struct ScriptedSink {
    written: Arc<Mutex<Vec<WhiteMemoryRecord>>>,
    first_write_started: Option<Sender<()>>,
    gate: Option<Receiver<()>>,
    failures_left: u32,
    panic_on_write: bool,
}

impl ScriptedSink {
    fn new(written: Arc<Mutex<Vec<WhiteMemoryRecord>>>) -> Self {
        Self {
            written,
            first_write_started: None,
            gate: None,
            failures_left: 0,
            panic_on_write: false,
        }
    }
}

impl WhiteRecordSink for ScriptedSink {
    fn write(&mut self, update: &WhiteMemoryUpdate) -> Result<WriteCommit, WhiteMemoryError> {
        // Эти тесты про белую; правило и слежение проверяют `rule_tests` и `follow_tests` на своих приёмниках.
        let Some(record) = &update.record else {
            return Ok(WriteCommit::Durable);
        };
        if self.panic_on_write {
            panic!("приёмник сломан (проверка стража потока)");
        }
        if let Some(started) = self.first_write_started.take() {
            started.send(()).expect("тест ждёт");
        }
        if let Some(gate) = self.gate.take() {
            gate.recv().expect("ворота открыты");
        }
        if self.failures_left > 0 {
            self.failures_left -= 1;
            return Err(WhiteMemoryError::Write("диск полон".to_owned()));
        }
        self.written.lock().expect("замок").push(*record);
        Ok(WriteCommit::Durable)
    }
}

#[test]
fn records_arriving_during_a_slow_write_are_coalesced_and_the_last_one_wins() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let (started_sender, started) = channel();
    let (open_gate, gate) = channel();
    let mut sink = ScriptedSink::new(Arc::clone(&written));
    sink.first_write_started = Some(started_sender);
    sink.gate = Some(gate);
    let diagnostics = WhiteMemoryDiagnostics::default();
    let mut writer = WhiteMemoryWriter::spawn(sink, diagnostics.clone(), Duration::ZERO).expect("поток");
    let first = record(3_000.0, 0.0, RememberedOrigin::Manual);
    let middle = record(4_000.0, 0.0, RememberedOrigin::Manual);
    let last = record(5_000.0, 0.01, RememberedOrigin::Source);

    writer.save(first);
    started
        .recv_timeout(Duration::from_secs(5))
        .expect("поток начал писать первую");
    // Пока диск «занят», приходят ещё две записи: средняя вытесняется последней.
    writer.save(middle);
    writer.save(last);
    open_gate.send(()).expect("ворота");
    let stats = writer.close();

    assert_eq!(*written.lock().expect("замок"), vec![first, last]);
    assert_eq!(stats.requested, 3);
    assert_eq!(stats.coalesced, 1, "средняя запись склеена");
    assert_eq!(stats.saved, 2);
    assert_eq!(stats.failed, 0);
}

#[test]
fn a_failed_write_is_counted_and_reported_and_the_next_successful_write_clears_the_error() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let mut sink = ScriptedSink::new(Arc::clone(&written));
    sink.failures_left = 1;
    let diagnostics = WhiteMemoryDiagnostics::default();
    let mut writer = WhiteMemoryWriter::spawn(sink, diagnostics.clone(), Duration::ZERO).expect("поток");

    writer.save(record(3_000.0, 0.0, RememberedOrigin::Manual));
    wait_until("ошибка записи видна", || {
        diagnostics.snapshot().failed == 1
    });
    let after_failure = diagnostics.snapshot();
    assert_eq!(after_failure.saved, 0);
    assert!(
        after_failure
            .last_error
            .as_deref()
            .is_some_and(|text| text.contains("диск полон")),
        "{after_failure:?}"
    );

    let retry = record(3_100.0, 0.0, RememberedOrigin::Manual);
    writer.save(retry);
    wait_until("вторая запись на диске", || {
        diagnostics.snapshot().saved == 1
    });
    assert_eq!(diagnostics.snapshot().last_error, None, "успех снимает ошибку");
    assert_eq!(*written.lock().expect("замок"), vec![retry]);
}

#[test]
fn closing_writes_the_last_accepted_record_and_later_saves_are_rejected_not_lost_silently() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let mut writer = WhiteMemoryWriter::spawn(
        ScriptedSink::new(Arc::clone(&written)),
        WhiteMemoryDiagnostics::default(),
        Duration::ZERO, // min_write_interval: эти тесты не про частоту записи
    )
    .expect("поток");
    let last = record(3_000.0, 0.0, RememberedOrigin::Manual);
    writer.save(last);
    let stats = writer.close();
    assert_eq!(
        *written.lock().expect("замок"),
        vec![last],
        "закрытие дописало принятое"
    );
    assert_eq!(stats.saved, 1);

    writer.save(record(9_000.0, 0.0, RememberedOrigin::Manual));
    let after = writer.close();
    assert_eq!(after.rejected, 1, "запись после закрытия отброшена и посчитана");
    assert_eq!(written.lock().expect("замок").len(), 1);
}

#[test]
fn a_panicking_sink_kills_only_the_writer_and_later_saves_are_counted_as_rejected() {
    let mut sink = ScriptedSink::new(Arc::default());
    sink.panic_on_write = true;
    let diagnostics = WhiteMemoryDiagnostics::default();
    let mut writer = WhiteMemoryWriter::spawn(sink, diagnostics.clone(), Duration::ZERO).expect("поток");

    writer.save(record(3_000.0, 0.0, RememberedOrigin::Manual));
    wait_until("поток записи умер", || {
        diagnostics.snapshot().writer_died
    });
    writer.save(record(3_100.0, 0.0, RememberedOrigin::Manual));

    assert_eq!(diagnostics.snapshot().rejected, 1);
}

// --- Настоящий файл: отказы записи ---

#[test]
fn writing_into_a_missing_directory_fails_visibly_without_panic() {
    let (directory, path) = app_directory();
    let mut opened = open(&directory);
    // Каталог исчез между запуском и записью.
    std::fs::remove_dir_all(&path).expect("каталог убран");

    opened.parts.memory.save(record(3_000.0, 0.0, RememberedOrigin::Manual));
    let diagnostics = opened.diagnostics.clone();
    wait_until("отказ записи виден", || {
        diagnostics.snapshot().failed == 1
    });

    let stats = diagnostics.snapshot();
    assert_eq!(stats.saved, 0);
    assert!(
        stats
            .last_error
            .as_deref()
            .is_some_and(|text| text.contains("временный файл не создан")),
        "{stats:?}"
    );
}

#[test]
fn a_missing_directory_at_startup_means_nothing_remembered() {
    let (directory, path) = app_directory();
    std::fs::remove_dir_all(&path).expect("каталог убран");
    assert_eq!(open(&directory).parts.load, WhiteMemoryLoad::NothingRemembered);
}

// --- Настоящий файл: не читается ---

/// Кладёт файл с содержимым, открывает память и проверяет «не читается», карантин и сохранность содержимого.
pub(super) fn assert_unreadable_and_quarantined(content: &[u8], expected_reason: &str) {
    let (directory, path) = app_directory();
    std::fs::write(path.join("white.json"), content).expect("файл записан");

    let opened = open(&directory);

    match &opened.parts.load {
        WhiteMemoryLoad::Unreadable { reason } => {
            assert!(
                reason.contains(expected_reason),
                "причина {reason:?} не содержит {expected_reason:?}"
            );
            assert!(
                reason.contains(QUARANTINE_FILE_MARKER),
                "в причине виден карантин: {reason}"
            );
        }
        other => panic!("ждали Unreadable, получили {other:?}"),
    }
    assert!(!path.join("white.json").exists(), "испорченный файл убран с пути");
    let quarantined = quarantined_files(&path);
    assert_eq!(quarantined.len(), 1, "{quarantined:?}");
    assert_eq!(
        std::fs::read(&quarantined[0]).expect("карантин читается"),
        content,
        "чужое не потеряно"
    );
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn an_empty_file_is_unreadable_and_quarantined() {
    assert_unreadable_and_quarantined(b"", "не JSON");
}

#[test]
fn garbage_is_unreadable_and_quarantined() {
    assert_unreadable_and_quarantined(b"\xff\xfenot json at all", "не JSON");
}

#[test]
fn a_file_of_a_newer_schema_is_unreadable_and_quarantined_untouched() {
    assert_unreadable_and_quarantined(
        br#"{"schema_version":2,"kelvin":3200.0,"duv":0.0,"origin":"source","extra":1}"#,
        "новее",
    );
}

#[test]
fn a_file_without_a_schema_version_is_unreadable() {
    assert_unreadable_and_quarantined(br#"{"kelvin":3200.0,"duv":0.0,"origin":"source"}"#, "schema_version");
}

#[test]
fn a_foreign_document_with_an_unknown_field_or_origin_is_unreadable() {
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"kelvin":3200.0,"duv":0.0,"origin":"source","mood":"calm"}"#,
        "mood",
    );
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"kelvin":3200.0,"duv":0.0,"origin":"neural"}"#,
        "neural",
    );
}

#[test]
fn values_outside_the_domain_are_unreadable_and_not_adopted() {
    // Температура должна быть конечной и положительной: отрицательная не принимается типом домена.
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"kelvin":-5.0,"duv":0.0,"origin":"manual"}"#,
        "температура",
    );
    // Пропущенное поле — не «ноль по умолчанию».
    assert_unreadable_and_quarantined(br#"{"schema_version":1,"kelvin":3200.0,"origin":"manual"}"#, "duv");
}

#[test]
fn after_a_quarantine_the_memory_works_and_a_new_value_replaces_the_missing_file() {
    let (directory, path) = app_directory();
    std::fs::write(path.join("white.json"), b"{").expect("файл записан");
    let fresh = record(4_500.0, 0.0, RememberedOrigin::Manual);
    {
        let mut opened = open(&directory);
        assert!(matches!(opened.parts.load, WhiteMemoryLoad::Unreadable { .. }));
        opened.parts.memory.save(fresh);
    }

    assert_eq!(open(&directory).parts.load, WhiteMemoryLoad::Remembered(fresh));
    assert_eq!(quarantined_files(&path).len(), 1, "карантин остался для разбора");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_path_that_cannot_be_read_refuses_saves_and_keeps_what_is_there() {
    let (directory, path) = app_directory();
    // На месте файла лежит каталог с чужим содержимым: это не «файла нет», а «прочитать нельзя». Затирать его
    // записью нельзя, карантин для него не предусмотрен.
    std::fs::create_dir(path.join("white.json")).expect("каталог вместо файла");
    std::fs::write(path.join("white.json").join("inside.txt"), b"important").expect("файл внутри");

    let mut opened = open(&directory);
    match &opened.parts.load {
        WhiteMemoryLoad::Unreadable { reason } => assert!(reason.contains("не прочитан"), "{reason}"),
        other => panic!("ждали Unreadable, получили {other:?}"),
    }
    opened.parts.memory.save(record(3_000.0, 0.0, RememberedOrigin::Manual));
    let diagnostics = opened.diagnostics.clone();
    wait_until("отказ записи виден", || {
        diagnostics.snapshot().failed == 1
    });

    let stats = diagnostics.snapshot();
    assert!(
        stats
            .last_error
            .as_deref()
            .is_some_and(|text| text.contains("запись отключена")),
        "{stats:?}"
    );
    assert_eq!(
        std::fs::read(path.join("white.json").join("inside.txt")).expect("чужое цело"),
        b"important"
    );
    drop(opened);
    std::fs::remove_dir_all(path).expect("уборка");
}

// --- Режим none и передача движку ---

#[test]
fn mode_none_reads_no_file_writes_no_file_and_reports_nothing_remembered() {
    let (directory, path) = app_directory();
    let existing = record(3_300.0, 0.0, RememberedOrigin::Manual);
    {
        let mut opened = open(&directory);
        opened.parts.memory.save(existing);
    }
    let config = WhiteMemoryConfig {
        mode: WhiteMemoryMode::None,
        ..WhiteMemoryConfig::default()
    };

    let mut disabled = open_white_memory(&directory, &config);
    assert_eq!(
        disabled.parts.load,
        WhiteMemoryLoad::NothingRemembered,
        "белую владельца прогон не читает"
    );
    let before = std::fs::read(path.join("white.json")).expect("файл владельца на месте");
    disabled
        .parts
        .memory
        .save(record(9_000.0, 0.0, RememberedOrigin::Manual));
    drop(disabled);

    assert_eq!(
        std::fs::read(path.join("white.json")).expect("файл владельца на месте"),
        before,
        "прогон не портит белую владельца"
    );
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn the_handoff_gives_the_opened_memory_once_and_defaults_afterwards() {
    let (directory, path) = app_directory();
    let remembered = record(3_300.0, 0.0, RememberedOrigin::Manual);
    {
        let mut opened = open(&directory);
        opened.parts.memory.save(remembered);
    }
    let handoff = WhiteMemoryHandoff::holding(open(&directory));

    let first = handoff.take();
    let second = handoff.take();

    assert_eq!(first.load, WhiteMemoryLoad::Remembered(remembered));
    assert_eq!(second.load, WhiteMemoryLoad::NothingRemembered, "память у движка одна");
    assert_eq!(
        WhiteMemoryHandoff::default().take().load,
        WhiteMemoryLoad::NothingRemembered
    );
    std::fs::remove_dir_all(path).expect("уборка");
}

// --- Конфиг ---

#[test]
fn the_config_defaults_are_file_mode_with_the_documented_names_and_pass_validation() {
    let config = WhiteMemoryConfig::default();
    assert_eq!(config.mode, WhiteMemoryMode::File);
    assert_eq!(config.file_name, "white.json");
    assert_eq!(config.temp_file_name(), "white.json.tmp");
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn the_config_is_read_from_json_and_rejects_typos_and_bad_names() {
    let none: WhiteMemoryConfig = serde_json::from_str(r#"{"mode":"none"}"#).expect("разбор");
    assert_eq!(none.mode, WhiteMemoryMode::None);
    assert_eq!(none.file_name, "white.json", "остальное — по умолчанию");
    assert!(serde_json::from_str::<WhiteMemoryConfig>(r#"{"mode":"files"}"#).is_err());
    assert!(serde_json::from_str::<WhiteMemoryConfig>(r#"{"file":"x"}"#).is_err());

    for bad_name in ["", ".", "..", "a/b", "a\\b"] {
        let config = WhiteMemoryConfig {
            file_name: bad_name.to_owned(),
            ..WhiteMemoryConfig::default()
        };
        assert!(
            matches!(config.validate(), Err(WhiteMemoryConfigError::InvalidFileName { .. })),
            "{bad_name:?}"
        );
    }
    let empty_suffix = WhiteMemoryConfig {
        temp_file_suffix: String::new(),
        ..WhiteMemoryConfig::default()
    };
    assert_eq!(empty_suffix.validate(), Err(WhiteMemoryConfigError::EmptyTempSuffix));
    let escaping_suffix = WhiteMemoryConfig {
        temp_file_suffix: "/../x".to_owned(),
        ..WhiteMemoryConfig::default()
    };
    assert!(matches!(
        escaping_suffix.validate(),
        Err(WhiteMemoryConfigError::TempSuffixHasSeparator { .. })
    ));
}

#[test]
fn custom_file_names_from_the_config_are_used_for_both_files() {
    let (directory, path) = app_directory();
    let config = WhiteMemoryConfig {
        file_name: "custom-white.json".to_owned(),
        temp_file_suffix: ".partial".to_owned(),
        ..WhiteMemoryConfig::default()
    };
    {
        let mut opened = open_white_memory(&directory, &config);
        opened.parts.memory.save(record(3_000.0, 0.0, RememberedOrigin::Manual));
    }
    assert!(path.join("custom-white.json").exists());
    assert!(!path.join("white.json").exists());
    assert!(
        !path.join("custom-white.json.partial").exists(),
        "временный файл переименован"
    );
    std::fs::remove_dir_all(path).expect("уборка");
}
