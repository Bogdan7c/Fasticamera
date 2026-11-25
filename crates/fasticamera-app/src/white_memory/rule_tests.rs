//! Правило возобновления в памяти белой (WB36): формат файла, два независимых места, одна запись на диск за
//! пробуждение, «пиши немедленно», отказ испорченного, точность чисел после перезапуска процесса.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use camera_core::white::{RememberedOrigin, WhiteMemory, WhiteMemoryLoad, WhiteMemoryRecord, WhiteResumeRule};
use std::sync::mpsc::{Receiver, Sender, channel};

use super::tests::{app_directory, assert_unreadable_and_quarantined, record, wait_until};
use super::*;

fn open(directory: &AppDirectory) -> OpenedWhiteMemory {
    open_white_memory(directory, &WhiteMemoryConfig::default())
}

/// Открыть, записать в память всё из `act`, закрыть (Drop дописывает принятое), открыть заново — как перезапуск процесса.
fn restart_after(directory: &AppDirectory, act: impl FnOnce(&mut dyn WhiteMemory)) -> OpenedWhiteMemory {
    {
        let mut opened = open(directory);
        act(opened.parts.memory.as_mut());
    }
    open(directory)
}

// --- Файл ---

#[test]
fn a_file_written_by_the_previous_version_without_a_rule_still_reads_and_keeps_its_white_after_a_rule_is_saved() {
    let (directory, path) = app_directory();
    // Буквальные байты файла прежней версии: без `resume_rule`.
    let legacy = b"{\n  \"schema_version\": 1,\n  \"kelvin\": 3200.0,\n  \"duv\": 0.004,\n  \"origin\": \"source\"\n}";
    std::fs::write(path.join("white.json"), legacy).expect("старый файл");

    let opened = open(&directory);
    assert_eq!(
        opened.parts.load,
        WhiteMemoryLoad::Remembered(record(3_200.0, 0.004, RememberedOrigin::Source))
    );
    assert_eq!(opened.parts.resume_rule, None, "выбора в старом файле нет");
    drop(opened);

    // Выбор правила записывается, белая старого файла остаётся.
    let reopened = restart_after(&directory, |memory| memory.save_resume_rule(WhiteResumeRule::KeepLast));
    assert_eq!(
        reopened.parts.load,
        WhiteMemoryLoad::Remembered(record(3_200.0, 0.004, RememberedOrigin::Source))
    );
    assert_eq!(reopened.parts.resume_rule, Some(WhiteResumeRule::KeepLast));
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_file_holding_only_the_rule_reads_as_no_white_and_the_choice() {
    let (directory, path) = app_directory();
    std::fs::write(
        path.join("white.json"),
        br#"{"schema_version":1,"resume_rule":"keep_last"}"#,
    )
    .expect("файл");

    let opened = open(&directory);

    assert_eq!(opened.parts.load, WhiteMemoryLoad::NothingRemembered);
    assert_eq!(opened.parts.resume_rule, Some(WhiteResumeRule::KeepLast));
    assert!(quarantined(&path).is_empty(), "файл читается, карантина нет");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_rule_chosen_before_the_first_white_is_written_alone_and_survives_the_restart() {
    let (directory, path) = app_directory();

    let reopened = restart_after(&directory, |memory| {
        memory.save_resume_rule(WhiteResumeRule::TakeFromSource)
    });

    assert_eq!(reopened.parts.load, WhiteMemoryLoad::NothingRemembered);
    assert_eq!(reopened.parts.resume_rule, Some(WhiteResumeRule::TakeFromSource));
    let text = std::fs::read_to_string(path.join("white.json")).expect("файл есть");
    assert!(text.contains("\"resume_rule\": \"take_from_source\""), "{text}");
    assert!(!text.contains("kelvin"), "белой нет — полей белой в файле нет: {text}");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn the_order_of_writes_never_loses_the_other_place() {
    let white = record(4_321.0, -0.008, RememberedOrigin::Manual);
    for rule_first in [true, false] {
        let (directory, path) = app_directory();
        // Каждое место пишется в отдельном «запуске»: порядок записей между перезапусками.
        {
            let mut opened = open(&directory);
            if rule_first {
                opened.parts.memory.save_resume_rule(WhiteResumeRule::KeepLast);
            } else {
                opened.parts.memory.save(white);
            }
        }
        let reopened = restart_after(&directory, |memory| {
            if rule_first {
                memory.save(white);
            } else {
                memory.save_resume_rule(WhiteResumeRule::KeepLast);
            }
        });

        assert_eq!(
            reopened.parts.load,
            WhiteMemoryLoad::Remembered(white),
            "rule_first={rule_first}"
        );
        assert_eq!(
            reopened.parts.resume_rule,
            Some(WhiteResumeRule::KeepLast),
            "rule_first={rule_first}"
        );
        std::fs::remove_dir_all(path).expect("уборка");
    }
}

#[test]
fn writing_the_white_again_does_not_forget_the_rule_chosen_in_the_same_run() {
    let (directory, path) = app_directory();
    let first = record(3_000.0, 0.0, RememberedOrigin::Manual);
    let last = record(7_000.0, 0.01, RememberedOrigin::Source);

    let reopened = restart_after(&directory, |memory| {
        memory.save_resume_rule(WhiteResumeRule::KeepLast);
        memory.save(first);
        memory.flush();
        memory.save(last);
    });

    assert_eq!(reopened.parts.load, WhiteMemoryLoad::Remembered(last));
    assert_eq!(reopened.parts.resume_rule, Some(WhiteResumeRule::KeepLast));
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_broken_or_ambiguous_document_is_unreadable_and_quarantined_untouched() {
    // Белая задана частично: ни «ноль по умолчанию», ни выброс одной половины.
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"kelvin":3200.0,"resume_rule":"keep_last"}"#,
        "частично",
    );
    // Неизвестное правило.
    assert_unreadable_and_quarantined(br#"{"schema_version":1,"resume_rule":"always"}"#, "always");
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"kelvin":3200.0,"duv":0.0,"origin":"source","resume_rule":"never"}"#,
        "never",
    );
    // Пустой документ: ни белой, ни правила.
    assert_unreadable_and_quarantined(br#"{"schema_version":1}"#, "ни белой, ни правила");
}

#[test]
fn awkward_floats_survive_the_json_round_trip_bit_for_bit() {
    // Числа, на которых разбор JSON без точного режима ошибается в последнем знаке: «не меняется ни на бит»
    // после перезапуска процесса требует, чтобы К и Duv возвращались теми же двоичными числами.
    let awkward_kelvin = [
        5_601.123_456_789_012_f64,
        3_200.0_f64.next_up(),
        2_000.0_f64.next_up(),
        10_000.0_f64.next_down(),
        4_500.1,
        6_503.629_f64.next_down(),
        7_777.777_777_777_777,
        12_000.0 / 3.0,
    ];
    let awkward_duv = [
        0.004_f64.next_up(),
        -0.05_f64.next_up(),
        0.012_345_678_901_234_567,
        -0.1 / 3.0,
        2.225_073_858_507_201e-5,
        1.7e-9,
    ];
    for (index, kelvin) in awkward_kelvin.into_iter().enumerate() {
        let duv = awkward_duv[index % awkward_duv.len()];
        let (directory, path) = app_directory();
        let saved = record(kelvin, duv, RememberedOrigin::Source);

        let reopened = restart_after(&directory, |memory| memory.save(saved));

        let WhiteMemoryLoad::Remembered(read) = reopened.parts.load else {
            panic!("файл не прочитан: {:?}", reopened.parts.load)
        };
        assert_eq!(
            read.setting.temperature.kelvin().to_bits(),
            kelvin.to_bits(),
            "К {kelvin:?} вернулось как {:?}",
            read.setting.temperature.kelvin()
        );
        assert_eq!(
            read.setting.tint.duv().to_bits(),
            duv.to_bits(),
            "Duv {duv:?} вернулось как {:?}",
            read.setting.tint.duv()
        );
        std::fs::remove_dir_all(path).expect("уборка");
    }
}

#[test]
fn mode_none_ignores_a_remembered_rule_and_writes_no_rule() {
    let (directory, path) = app_directory();
    restart_after(&directory, |memory| memory.save_resume_rule(WhiteResumeRule::KeepLast));
    let before = std::fs::read(path.join("white.json")).expect("файл владельца");
    let config = WhiteMemoryConfig {
        mode: WhiteMemoryMode::None,
        ..WhiteMemoryConfig::default()
    };

    let mut disabled = open_white_memory(&directory, &config);
    assert_eq!(disabled.parts.resume_rule, None, "прогон не читает выбор владельца");
    disabled.parts.memory.save_resume_rule(WhiteResumeRule::TakeFromSource);
    drop(disabled);

    assert_eq!(std::fs::read(path.join("white.json")).expect("файл"), before);
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn the_handoff_carries_the_remembered_rule_to_the_engine_wiring() {
    let (directory, path) = app_directory();
    let reopened = restart_after(&directory, |memory| memory.save_resume_rule(WhiteResumeRule::KeepLast));
    let handoff = WhiteMemoryHandoff::holding(reopened);

    assert_eq!(handoff.take().resume_rule, Some(WhiteResumeRule::KeepLast));
    assert_eq!(handoff.take().resume_rule, None, "память у движка одна");
    assert_eq!(WhiteMemoryHandoff::default().take().resume_rule, None);
    std::fs::remove_dir_all(path).expect("уборка");
}

fn quarantined(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(directory)
        .expect("папка читается")
        .map(|entry| entry.expect("запись").path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains(QUARANTINE_FILE_MARKER))
        })
        .collect()
}

// --- Поток записи: два места ящика ---

/// Что приёмник получил за одну запись.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Record(WhiteMemoryRecord),
    Rule(WhiteResumeRule),
    Both(WhiteMemoryRecord, WhiteResumeRule),
}

/// Приёмник-заглушка: первая запись ждёт «ворота», каждый вызов — в списке. Заглушка видит ОДНУ запись
/// на всё занятое к пробуждению, как файл.
struct CallLog {
    calls: Arc<Mutex<Vec<Call>>>,
    first_write_started: Option<Sender<()>>,
    gate: Option<Receiver<()>>,
}

impl CallLog {
    fn wait_gate(&mut self) {
        if let Some(started) = self.first_write_started.take() {
            started.send(()).expect("тест ждёт");
        }
        if let Some(gate) = self.gate.take() {
            gate.recv().expect("ворота открыты");
        }
    }

    fn log(&self, call: Call) {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(call);
    }
}

impl WhiteRecordSink for CallLog {
    fn write(&mut self, update: &WhiteMemoryUpdate) -> Result<WriteCommit, WhiteMemoryError> {
        self.wait_gate();
        let call = match (update.record, update.rule, update.follow) {
            (Some(record), None, None) => Call::Record(record),
            (None, Some(rule), None) => Call::Rule(rule),
            (Some(record), Some(rule), None) => Call::Both(record, rule),
            other => panic!("эти тесты не сохраняют слежение и не шлют пустых записей: {other:?}"),
        };
        self.log(call);
        Ok(WriteCommit::Durable)
    }
}

#[test]
fn both_places_taken_in_one_wake_go_to_the_disk_as_one_write_and_each_place_coalesces_on_its_own() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (started_sender, started) = channel();
    let (open_gate, gate) = channel();
    let sink = CallLog {
        calls: Arc::clone(&calls),
        first_write_started: Some(started_sender),
        gate: Some(gate),
    };
    let diagnostics = WhiteMemoryDiagnostics::default();
    let mut writer = WhiteMemoryWriter::spawn(sink, diagnostics, Duration::ZERO).expect("поток");
    let first = record(3_000.0, 0.0, RememberedOrigin::Manual);
    let middle = record(4_000.0, 0.0, RememberedOrigin::Manual);
    let last = record(5_000.0, 0.01, RememberedOrigin::Source);

    writer.save(first);
    started
        .recv_timeout(Duration::from_secs(5))
        .expect("поток начал первую запись");
    // Пока диск «занят»: выбор, ещё выбор (вытесняет первый), белая, ещё белая (вытесняет первую).
    writer.save_resume_rule(WhiteResumeRule::TakeFromSource);
    writer.save_resume_rule(WhiteResumeRule::KeepLast);
    writer.save(middle);
    writer.save(last);
    open_gate.send(()).expect("ворота");
    let stats = writer.close();

    assert_eq!(
        *calls.lock().expect("замок"),
        vec![Call::Record(first), Call::Both(last, WhiteResumeRule::KeepLast)],
        "второе пробуждение — одна запись с последним значением каждого места"
    );
    assert_eq!(stats.requested, 5);
    assert_eq!(stats.coalesced, 2, "по одной склейке в каждом месте");
    assert_eq!(stats.saved, 2, "две записи на диск, не три");
    assert_eq!(stats.failed, 0);
}

#[test]
fn a_rule_alone_is_written_as_a_rule_alone_and_does_not_touch_the_white_place() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let sink = CallLog {
        calls: Arc::clone(&calls),
        first_write_started: None,
        gate: None,
    };
    let mut writer = WhiteMemoryWriter::spawn(sink, WhiteMemoryDiagnostics::default(), Duration::ZERO).expect("поток");

    writer.save_resume_rule(WhiteResumeRule::KeepLast);
    let stats = writer.close();

    assert_eq!(
        *calls.lock().expect("замок"),
        vec![Call::Rule(WhiteResumeRule::KeepLast)]
    );
    assert_eq!(stats.saved, 1);
}

#[test]
fn flush_releases_a_rule_waiting_for_the_write_interval_and_closing_writes_the_last_rule() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let sink = CallLog {
        calls: Arc::clone(&calls),
        first_write_started: None,
        gate: None,
    };
    // Интервал в час: без `flush` вторая запись ждала бы весь час.
    let mut writer =
        WhiteMemoryWriter::spawn(sink, WhiteMemoryDiagnostics::default(), Duration::from_secs(3_600)).expect("поток");
    writer.save_resume_rule(WhiteResumeRule::KeepLast);
    wait_until("первая запись ушла сразу", || {
        calls.lock().expect("замок").len() == 1
    });

    writer.save_resume_rule(WhiteResumeRule::TakeFromSource);
    writer.flush();
    wait_until("flush снял отсрочку с выбора правила", || {
        calls.lock().expect("замок").len() == 2
    });

    assert_eq!(
        *calls.lock().expect("замок"),
        vec![
            Call::Rule(WhiteResumeRule::KeepLast),
            Call::Rule(WhiteResumeRule::TakeFromSource)
        ]
    );
    // Закрытие дописывает последнее принятое тоже.
    writer.save_resume_rule(WhiteResumeRule::KeepLast);
    writer.close();
    assert_eq!(calls.lock().expect("замок").len(), 3);
}

#[test]
fn a_rule_saved_after_closing_is_rejected_and_counted() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let sink = CallLog {
        calls: Arc::clone(&calls),
        first_write_started: None,
        gate: None,
    };
    let mut writer = WhiteMemoryWriter::spawn(sink, WhiteMemoryDiagnostics::default(), Duration::ZERO).expect("поток");
    writer.close();

    writer.save_resume_rule(WhiteResumeRule::KeepLast);

    let stats = writer.close();
    assert_eq!(stats.rejected, 1);
    assert!(calls.lock().expect("замок").is_empty());
}
