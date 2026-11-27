//! Проблема запуска хранилища пресетов доходит до плашки на экране (решение 287): НАСТОЯЩИЙ файл пресетов во временной
//! папке → `open_look_service` → библиотека → источник кадров → экран. Плашка поднимается при первом входе в раздел
//! калибровки (не при старте) и ровно один раз; нет файла — нет плашки.

use camera_screen::modes::CaptureMode;
use camera_screen::{ProblemKind, ScreenAction};
use look_contract::LookProblemKind;
use look_presets::{PresetStoreConfig, QUARANTINE_FILE_MARKER};

use super::*;
use crate::app_directory::AppDirectory;
use crate::camera_screen_input::ScreenInputSender;
use crate::look_facts::{BandSwatchStyle, LookFactsCatalog, LookFactsSource};
use crate::look_startup::open_look_service;
use crate::look_test_support::default_environment;

/// Что лежит на месте файла пресетов при старте.
enum PresetsFileOnDisk {
    /// Файла нет (первый запуск).
    Missing,
    /// Испорченный файл: библиотека убирает его в карантин, работают встроенные пресеты.
    Corrupt,
    /// На месте файла каталог: прочитать нельзя, не «нет файла» — сохранения отключены.
    UnreadableDirectory,
}

struct Stand {
    source: CameraScreenSource,
    sender: ScreenInputSender,
    /// Библиотека пресетов процесса: переживает экран, как в приложении.
    handle: crate::look_service::LookHandle,
    directory: std::path::PathBuf,
    now_ms: i64,
}

impl Stand {
    /// Запуск процесса с заданным файлом пресетов; экран в PHOTO.
    fn start(file: PresetsFileOnDisk) -> Self {
        let directory =
            std::env::temp_dir().join(format!("fasticamera-look-banner-{}-{}", std::process::id(), unique()));
        std::fs::create_dir_all(&directory).expect("папка создана");
        let config = PresetStoreConfig::default();
        let presets_path = directory.join(&config.file_name);
        match file {
            PresetsFileOnDisk::Missing => {}
            PresetsFileOnDisk::Corrupt => std::fs::write(&presets_path, b"{ not json").expect("файл"),
            PresetsFileOnDisk::UnreadableDirectory => std::fs::create_dir(&presets_path).expect("каталог"),
        }
        let app_directory = AppDirectory::new(directory.to_string_lossy().into_owned()).expect("папка годится");
        let opened = open_look_service(&app_directory, &config, &default_environment()).expect("открыто");
        let (source, sender) = Self::build_source(&opened.handle);
        Self {
            source,
            sender,
            handle: opened.handle,
            directory,
            now_ms: 0,
        }
    }

    /// Новый источник кадров (новый экран) поверх общей библиотеки пресетов.
    fn build_source(handle: &crate::look_service::LookHandle) -> (CameraScreenSource, ScreenInputSender) {
        let catalog = LookFactsCatalog::from_render_config(
            &render_loop::config::RenderLoopConfig::default(),
            &BandSwatchStyle::default(),
        );
        let (sender, receiver) = screen_input_channel(16);
        let source = CameraScreenSource::new(ScreenSourceParts {
            look_white_stops: crate::look_white_stops::LookWhiteStopsWatcher::silent(),
            scope_labels: crate::scope_labels::ScopeLabelsWatcher::silent(),
            screen: CameraScreen::new(
                ScreenConfig::default(),
                ui_kit::tokens::UiKitTokens::default(),
                ui_core::time::MotionPreference::Full,
            )
            .expect("экран"),
            input: receiver,
            facts: Box::new(LiveFacts),
            look_facts: Some(LookFactsSource::new(handle.clone(), catalog)),
            gravity: GravityLink::Unavailable {
                reason: "тест".to_owned(),
            },
            executor: EffectExecutor::new(EffectTargets {
                camera: Box::new(SwitchablePort {
                    outcome: Arc::new(Mutex::new(CameraIntentOutcome::NoEngine)),
                    received: Arc::default(),
                }),
                haptics: Arc::new(QuietHaptics),
                commands: Arc::new(QuietCommands),
                render: RecordingRenderControl::new(),
                capture: Arc::new(NoCapture),
                shutter_sound: Arc::new(crate::camera_screen_service::SilentShutterSound::default()),
                lens: camera_contract::zoom::LensIntentChannel::new(),
                look: Some(handle.clone()),
            }),
            board: Arc::new(ScreenBoard::default()),
            report_min_interval: Duration::ZERO,
        });
        (source, sender)
    }

    /// Экран пересоздан (восстановление после снятия рендером): библиотека та же, экран новый и пустой.
    fn recreate_screen(&mut self) {
        let (source, sender) = Self::build_source(&self.handle);
        self.source = source;
        self.sender = sender;
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.now_ms += 16;
            self.source.produce_frame(&frame_request(self.now_ms)).expect("кадр");
        }
    }

    fn select_mode(&mut self, mode: CaptureMode) {
        self.sender.send_action(ScreenAction::ModeSelected(mode));
        self.frames(3);
    }

    /// Какая проблема библиотеки висит плашкой сейчас.
    fn banner(&self) -> Option<LookProblemKind> {
        self.source
            .screen_state()
            .look()
            .banner()
            .and_then(|banner| match &banner.kind {
                ProblemKind::Look(kind) => Some(kind.clone()),
                _ => None,
            })
    }

    /// Закрывает плашку так, как это делает срок показа.
    fn dismiss_banner(&mut self) {
        let number = self.source.screen_state().look().banner().expect("плашка").number;
        self.sender.send_action(ScreenAction::ProblemDismissed { number });
        self.frames(2);
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        // Уборка не должна прятать падение теста: ошибка уборки допустима и ни на что не влияет.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn unique() -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn a_corrupt_presets_file_shows_presets_reset_once_on_the_first_entry_to_calibrate() {
    let mut stand = Stand::start(PresetsFileOnDisk::Corrupt);
    stand.frames(30);
    assert_eq!(stand.banner(), None, "при старте и в PHOTO плашки нет");
    let quarantined = std::fs::read_dir(&stand.directory)
        .expect("папка читается")
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().contains(QUARANTINE_FILE_MARKER));
    assert!(quarantined, "испорченный файл убран в карантин, а не потерян");

    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(
        stand.banner(),
        Some(LookProblemKind::PresetsReset),
        "плашка при входе в раздел"
    );
    stand.dismiss_banner();

    stand.select_mode(CaptureMode::Photo);
    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(stand.banner(), None, "второй вход плашку не повторяет");
}

#[test]
fn an_unreadable_presets_file_shows_saves_off_when_calibrate_is_opened_minutes_later() {
    let mut stand = Stand::start(PresetsFileOnDisk::UnreadableDirectory);
    // Минуты в PHOTO (≈ 3 минуты кадров по 16 мс): проблема не теряется и не всплывает.
    stand.frames(11_000);
    assert_eq!(stand.banner(), None);

    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(stand.banner(), Some(LookProblemKind::SavesRefused));
}

#[test]
fn a_missing_presets_file_is_a_normal_first_launch_and_shows_no_plaque() {
    let mut stand = Stand::start(PresetsFileOnDisk::Missing);
    stand.frames(10);
    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(stand.banner(), None);
}

#[test]
fn the_banner_survives_a_screen_recreated_before_the_first_entry_to_calibrate_and_shows_once() {
    let mut stand = Stand::start(PresetsFileOnDisk::Corrupt);
    stand.frames(30);
    assert_eq!(stand.banner(), None);

    // Экран пересоздали до первого входа в раздел (путь восстановления): проблема не пропала вместе со старым экраном.
    stand.recreate_screen();
    stand.frames(30);
    assert_eq!(stand.banner(), None, "в PHOTO плашки по-прежнему нет");
    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(
        stand.banner(),
        Some(LookProblemKind::PresetsReset),
        "новый экран показал плашку при первом входе"
    );
    stand.dismiss_banner();

    // Плашка показана: ещё одно пересоздание её не возвращает.
    stand.recreate_screen();
    stand.frames(30);
    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(stand.banner(), None, "после показа новый экран плашку не повторяет");
}

#[test]
fn a_screen_recreated_while_the_banner_is_up_does_not_repeat_it() {
    let mut stand = Stand::start(PresetsFileOnDisk::Corrupt);
    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(stand.banner(), Some(LookProblemKind::PresetsReset));
    stand.recreate_screen();
    stand.select_mode(CaptureMode::Calibrate);
    assert_eq!(
        stand.banner(),
        None,
        "показанную плашку подтвердили сервису, новый экран её не получает"
    );
}
