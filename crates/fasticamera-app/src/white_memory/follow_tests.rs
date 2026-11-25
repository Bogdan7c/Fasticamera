//! Выбор слежения за белым в памяти (план white-balance, сессия 04, WB44–WB56): формат файла (скупая запись,
//! чтобы файл без «Auto» читался прежними сборками), три независимых места, склейка и частота записи третьего места.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use camera_core::white::{
    RememberedOrigin, WhiteFollowRecord, WhiteFollowSpeed, WhiteKeeping, WhiteMemory, WhiteMemoryLoad, WhiteResumeRule,
};

use super::tests::{app_directory, assert_unreadable_and_quarantined, record, wait_until};
use super::*;

fn open(directory: &AppDirectory) -> OpenedWhiteMemory {
    open_white_memory(directory, &WhiteMemoryConfig::default())
}

/// Открыть, записать всё из `act`, закрыть (Drop дописывает принятое), открыть заново — как перезапуск процесса.
fn restart_after(directory: &AppDirectory, act: impl FnOnce(&mut dyn WhiteMemory)) -> OpenedWhiteMemory {
    {
        let mut opened = open(directory);
        act(opened.parts.memory.as_mut());
    }
    open(directory)
}

fn speed(millis: u64) -> WhiteFollowSpeed {
    WhiteFollowSpeed::from_millis(millis).expect("скорость в пределах")
}

fn following(millis: Option<u64>) -> WhiteFollowRecord {
    WhiteFollowRecord {
        keeping: WhiteKeeping::FollowSource,
        speed: millis.map(speed),
    }
}

/// Текст файла белой после записи.
fn file_text(path: &std::path::Path) -> String {
    std::fs::read_to_string(path.join("white.json")).expect("файл есть")
}

/// Ключи верхнего уровня JSON файла (для проверки «этого поля в файле нет»).
fn file_keys(path: &std::path::Path) -> Vec<String> {
    let document: serde_json::Value = serde_json::from_str(&file_text(path)).expect("файл — JSON");
    document.as_object().expect("объект").keys().cloned().collect()
}

// --- Файл: чтение и скупая запись ---

#[test]
fn a_follow_choice_survives_the_restart_through_the_real_file() {
    let (directory, path) = app_directory();

    let reopened = restart_after(&directory, |memory| memory.save_follow(following(Some(1_500))));

    assert_eq!(reopened.parts.follow, following(Some(1_500)));
    assert_eq!(reopened.parts.load, WhiteMemoryLoad::NothingRemembered, "белой не было");
    let text = file_text(&path);
    assert!(text.contains("\"keeping\": \"follow_source\""), "{text}");
    assert!(text.contains("\"follow_transition_ms\": 1500"), "{text}");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_zero_speed_is_a_real_choice_and_is_not_confused_with_no_choice() {
    let (directory, path) = app_directory();

    let reopened = restart_after(&directory, |memory| memory.save_follow(following(Some(0))));

    assert_eq!(
        reopened.parts.follow.speed,
        Some(speed(0)),
        "0 мс — «сразу», а не «не выбирал»"
    );
    assert!(file_keys(&path).contains(&"follow_transition_ms".to_owned()));
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_file_without_auto_contains_neither_new_field_so_the_previous_build_still_reads_it() {
    let (directory, path) = app_directory();
    let white = record(3_900.0, 0.002, RememberedOrigin::Manual);

    // Режим «держать» без выбранной скорости: ни одно из новых полей не пишется.
    restart_after(&directory, |memory| {
        memory.save(white);
        memory.save_follow(WhiteFollowRecord::default());
    });

    let keys = file_keys(&path);
    assert!(!keys.contains(&"keeping".to_owned()), "{keys:?}");
    assert!(!keys.contains(&"follow_transition_ms".to_owned()), "{keys:?}");
    assert!(keys.contains(&"kelvin".to_owned()), "белая на месте: {keys:?}");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn turning_follow_off_after_a_chosen_speed_drops_the_keeping_field_but_keeps_the_speed() {
    let (directory, path) = app_directory();

    let reopened = restart_after(&directory, |memory| {
        memory.save_follow(following(Some(800)));
        memory.save_follow(WhiteFollowRecord {
            keeping: WhiteKeeping::Hold,
            speed: Some(speed(800)),
        });
    });

    assert_eq!(
        reopened.parts.follow,
        WhiteFollowRecord {
            keeping: WhiteKeeping::Hold,
            speed: Some(speed(800))
        }
    );
    let keys = file_keys(&path);
    assert!(!keys.contains(&"keeping".to_owned()), "{keys:?}");
    assert!(keys.contains(&"follow_transition_ms".to_owned()), "{keys:?}");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_file_from_before_the_follow_fields_loads_with_the_default_choice() {
    let (directory, path) = app_directory();
    let legacy = br#"{"schema_version":1,"kelvin":3200.0,"duv":0.004,"origin":"source","resume_rule":"keep_last"}"#;
    std::fs::write(path.join("white.json"), legacy).expect("старый файл");

    let opened = open(&directory);

    assert_eq!(opened.parts.follow, WhiteFollowRecord::default());
    assert_eq!(opened.parts.follow.keeping, WhiteKeeping::Hold);
    assert_eq!(opened.parts.follow.speed, None);
    assert_eq!(
        opened.parts.load,
        WhiteMemoryLoad::Remembered(record(3_200.0, 0.004, RememberedOrigin::Source))
    );
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_file_holding_only_a_follow_choice_is_valid_and_has_no_white_and_no_rule() {
    let (directory, path) = app_directory();
    std::fs::write(
        path.join("white.json"),
        br#"{"schema_version":1,"keeping":"follow_source","follow_transition_ms":250}"#,
    )
    .expect("файл");

    let opened = open(&directory);

    assert_eq!(opened.parts.follow, following(Some(250)));
    assert_eq!(opened.parts.load, WhiteMemoryLoad::NothingRemembered);
    assert_eq!(opened.parts.resume_rule, None);
    assert!(path.join("white.json").exists(), "файл читается и остаётся на месте");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn an_explicit_hold_alone_is_a_valid_file_too() {
    let (directory, path) = app_directory();
    std::fs::write(path.join("white.json"), br#"{"schema_version":1,"keeping":"hold"}"#).expect("файл");

    let opened = open(&directory);

    assert_eq!(opened.parts.follow, WhiteFollowRecord::default());
    assert!(path.join("white.json").exists());
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn auto_turned_on_and_off_on_a_fresh_install_leaves_a_file_the_next_start_can_read() {
    let (directory, path) = app_directory();

    let reopened = restart_after(&directory, |memory| {
        memory.save_follow(following(None));
        memory.flush();
        memory.save_follow(WhiteFollowRecord::default());
    });

    assert_eq!(reopened.parts.follow, WhiteFollowRecord::default());
    assert_eq!(
        reopened.parts.load,
        WhiteMemoryLoad::NothingRemembered,
        "собственный файл не испорчен"
    );
    assert!(path.join("white.json").exists(), "и не отложен в карантин");
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_bad_keeping_or_an_out_of_range_speed_sends_the_file_to_quarantine_with_the_reason() {
    assert_unreadable_and_quarantined(br#"{"schema_version":1,"keeping":"always"}"#, "поля файла не подходят");
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"keeping":"follow_source","follow_transition_ms":10001}"#,
        "follow_transition_ms: скорость слежения 10001 мс",
    );
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"kelvin":3200.0,"duv":0.0,"origin":"manual","follow_transition_ms":-5}"#,
        "поля файла не подходят",
    );
    assert_unreadable_and_quarantined(
        br#"{"schema_version":1,"follow_transition_ms":"fast"}"#,
        "поля файла не подходят",
    );
}

#[test]
fn the_speed_limit_itself_is_accepted() {
    let (directory, path) = app_directory();
    std::fs::write(
        path.join("white.json"),
        br#"{"schema_version":1,"follow_transition_ms":10000}"#,
    )
    .expect("файл");

    assert_eq!(open(&directory).parts.follow.speed, Some(speed(10_000)));
    std::fs::remove_dir_all(path).expect("уборка");
}

// --- Три места не стирают друг друга ---

#[test]
fn white_rule_and_follow_never_erase_each_other_in_any_order_of_writes() {
    let white = record(4_321.0, -0.008, RememberedOrigin::Manual);
    let follow = following(Some(600));
    let orders: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    for order in orders {
        let (directory, path) = app_directory();
        // Каждое место пишется в отдельном «запуске»: порядок записей между перезапусками.
        for place in order {
            let mut opened = open(&directory);
            match place {
                0 => opened.parts.memory.save(white),
                1 => opened.parts.memory.save_resume_rule(WhiteResumeRule::KeepLast),
                _ => opened.parts.memory.save_follow(follow),
            }
        }

        let reopened = open(&directory);

        assert_eq!(reopened.parts.load, WhiteMemoryLoad::Remembered(white), "{order:?}");
        assert_eq!(reopened.parts.resume_rule, Some(WhiteResumeRule::KeepLast), "{order:?}");
        assert_eq!(reopened.parts.follow, follow, "{order:?}");
        std::fs::remove_dir_all(path).expect("уборка");
    }
}

#[test]
fn all_three_places_saved_in_one_run_survive_one_restart() {
    let (directory, path) = app_directory();
    let white = record(5_000.0, 0.0, RememberedOrigin::Source);

    let reopened = restart_after(&directory, |memory| {
        memory.save_follow(following(None));
        memory.save(white);
        memory.save_resume_rule(WhiteResumeRule::TakeFromSource);
    });

    assert_eq!(reopened.parts.load, WhiteMemoryLoad::Remembered(white));
    assert_eq!(reopened.parts.resume_rule, Some(WhiteResumeRule::TakeFromSource));
    assert_eq!(reopened.parts.follow, following(None));
    std::fs::remove_dir_all(path).expect("уборка");
}

// --- Поток записи: третье место ящика ---

/// Что приёмник получил за одну запись (запись целиком: видно, какие места занять).
type UpdateLog = Arc<Mutex<Vec<WhiteMemoryUpdate>>>;

/// Приёмник-заглушка: первая запись ждёт «ворота», каждая запись — в списке.
struct UpdateSink {
    updates: UpdateLog,
    first_write_started: Option<Sender<()>>,
    gate: Option<Receiver<()>>,
}

impl UpdateSink {
    fn plain(updates: &UpdateLog) -> Self {
        Self {
            updates: Arc::clone(updates),
            first_write_started: None,
            gate: None,
        }
    }
}

impl WhiteRecordSink for UpdateSink {
    fn write(&mut self, update: &WhiteMemoryUpdate) -> Result<WriteCommit, WhiteMemoryError> {
        if let Some(started) = self.first_write_started.take() {
            started.send(()).expect("тест ждёт");
        }
        if let Some(gate) = self.gate.take() {
            gate.recv().expect("ворота открыты");
        }
        self.updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(*update);
        Ok(WriteCommit::Durable)
    }
}

fn logged(updates: &UpdateLog) -> Vec<WhiteMemoryUpdate> {
    updates.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

#[test]
fn all_three_places_taken_in_one_wake_go_to_the_sink_as_one_write_and_each_place_coalesces_on_its_own() {
    let updates = UpdateLog::default();
    let (started_sender, started) = channel();
    let (open_gate, gate) = channel();
    let mut sink = UpdateSink::plain(&updates);
    sink.first_write_started = Some(started_sender);
    sink.gate = Some(gate);
    let mut writer = WhiteMemoryWriter::spawn(sink, WhiteMemoryDiagnostics::default(), Duration::ZERO).expect("поток");
    let first = record(3_000.0, 0.0, RememberedOrigin::Manual);
    let middle = record(4_000.0, 0.0, RememberedOrigin::Manual);
    let last = record(5_000.0, 0.01, RememberedOrigin::Source);

    writer.save(first);
    started
        .recv_timeout(Duration::from_secs(5))
        .expect("поток начал первую запись");
    // Пока диск «занят»: слежение дважды, правило, белая дважды — по одному вытеснению у слежения и у белой.
    writer.save_follow(following(Some(100)));
    writer.save_follow(following(Some(900)));
    writer.save_resume_rule(WhiteResumeRule::KeepLast);
    writer.save(middle);
    writer.save(last);
    open_gate.send(()).expect("ворота");
    let stats = writer.close();

    assert_eq!(
        logged(&updates),
        vec![
            WhiteMemoryUpdate {
                record: Some(first),
                ..WhiteMemoryUpdate::default()
            },
            WhiteMemoryUpdate {
                record: Some(last),
                rule: Some(WhiteResumeRule::KeepLast),
                follow: Some(following(Some(900))),
            },
        ],
        "второе пробуждение — одна запись с последним значением каждого из трёх мест"
    );
    assert_eq!(stats.requested, 6);
    assert_eq!(stats.coalesced, 2, "по одной склейке у слежения и у белой");
    assert_eq!(stats.saved, 2, "две записи на диск, не шесть");
    assert_eq!(stats.failed, 0);
}

#[test]
fn a_follow_choice_alone_is_written_alone_and_does_not_touch_the_other_places() {
    let updates = UpdateLog::default();
    let mut writer = WhiteMemoryWriter::spawn(
        UpdateSink::plain(&updates),
        WhiteMemoryDiagnostics::default(),
        Duration::ZERO,
    )
    .expect("поток");

    writer.save_follow(following(Some(300)));
    let stats = writer.close();

    assert_eq!(
        logged(&updates),
        vec![WhiteMemoryUpdate {
            follow: Some(following(Some(300))),
            ..WhiteMemoryUpdate::default()
        }]
    );
    assert_eq!(stats.saved, 1);
}

#[test]
fn flush_releases_a_follow_choice_waiting_for_the_write_interval_and_closing_writes_the_last_one() {
    let updates = UpdateLog::default();
    // Интервал в час: без `flush` вторая запись ждала бы весь час.
    let mut writer = WhiteMemoryWriter::spawn(
        UpdateSink::plain(&updates),
        WhiteMemoryDiagnostics::default(),
        Duration::from_secs(3_600),
    )
    .expect("поток");
    writer.save_follow(following(Some(100)));
    wait_until("первая запись ушла сразу", || {
        logged(&updates).len() == 1
    });

    writer.save_follow(following(Some(200)));
    // Без `flush` вторая запись обязана ждать интервал: она ещё не легла.
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(logged(&updates).len(), 1, "интервал держит вторую запись");
    writer.flush();
    wait_until("flush снял отсрочку со слежения", || {
        logged(&updates).len() == 2
    });
    assert_eq!(logged(&updates)[1].follow, Some(following(Some(200))));

    writer.save_follow(following(Some(300)));
    writer.close();
    let all = logged(&updates);
    assert_eq!(all.len(), 3, "закрытие дописало последнее принятое");
    assert_eq!(all[2].follow, Some(following(Some(300))));
}

#[test]
fn a_follow_choice_inside_the_interval_is_throttled_and_lands_after_it() {
    let updates = UpdateLog::default();
    let interval = Duration::from_millis(80);
    let mut writer = WhiteMemoryWriter::spawn(UpdateSink::plain(&updates), WhiteMemoryDiagnostics::default(), interval)
        .expect("поток");

    writer.save_follow(following(Some(100)));
    wait_until("первая запись ушла сразу", || {
        logged(&updates).len() == 1
    });
    for millis in [200, 300, 400] {
        writer.save_follow(following(Some(millis)));
    }
    assert_eq!(
        logged(&updates).len(),
        1,
        "три быстрых выбора не дали записей внутри интервала"
    );
    wait_until("последний выбор лёг после интервала", || logged(&updates).len() == 2);

    assert_eq!(
        logged(&updates)[1].follow,
        Some(following(Some(400))),
        "победил последний, средние склеены"
    );
    assert_eq!(writer.close().coalesced, 2);
}

#[test]
fn a_follow_choice_saved_after_closing_is_rejected_and_counted() {
    let updates = UpdateLog::default();
    let mut writer = WhiteMemoryWriter::spawn(
        UpdateSink::plain(&updates),
        WhiteMemoryDiagnostics::default(),
        Duration::ZERO,
    )
    .expect("поток");
    writer.close();

    writer.save_follow(following(Some(100)));

    let stats = writer.close();
    assert_eq!(stats.rejected, 1);
    assert!(logged(&updates).is_empty());
}

#[test]
fn a_file_sink_applies_every_taken_place_before_persisting_so_no_place_is_lost_from_the_disk() {
    // Настоящий файл, одна запись с тремя занятыми местами: все три ложатся в одну запись на диск.
    let (directory, path) = app_directory();
    let white = record(6_000.0, 0.0, RememberedOrigin::Manual);
    let mut opened = WhiteMemoryFile::open(path.as_path(), &WhiteMemoryConfig::default());

    opened
        .file
        .write(&WhiteMemoryUpdate {
            record: Some(white),
            rule: Some(WhiteResumeRule::KeepLast),
            follow: Some(following(Some(450))),
        })
        .expect("запись удалась");

    let reopened = open(&directory);
    assert_eq!(reopened.parts.load, WhiteMemoryLoad::Remembered(white));
    assert_eq!(reopened.parts.resume_rule, Some(WhiteResumeRule::KeepLast));
    assert_eq!(reopened.parts.follow, following(Some(450)));
    std::fs::remove_dir_all(path).expect("уборка");
}
