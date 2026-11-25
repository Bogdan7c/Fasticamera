//! Стенд сквозного теста белой: НАСТОЯЩИЙ движок камеры (`CameraRuntime` на fake-бэкенде, собранный тем же
//! `start_camera_with_links`, что и в продукте) + НАСТОЯЩИЙ бэкенд показа (RADV headless, полная validation) на
//! общих ящиках белой. Тест играет только роль камеры (подаёт кадры в рендер и их результаты в движок) и роль
//! источника света (`fake_source`).
//!
//! Связь тестового кода с продуктом ровно та же, что в `shell_wiring.rs`: `CameraLinks` → `render_side()` →
//! `WhiteLinks` бэкенда; реестр источников собирает тест (там — `product_white_sources`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera_android::BackendDiagnostics;
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::three_a::{DirectiveSlot, RequestOrigin};
use camera_contract::white::WhiteDirective;
use camera_core::CameraHandle;
use camera_core::fake::FakeBackendControl;
use camera_core::status::CameraStatus;
use camera_core::white::WhiteRequestState;
use fasticamera::auto_exposure_link::ExposureControllerKind;
use fasticamera::camera_config::CameraConfig;
use fasticamera::camera_service::CameraServices;
use fasticamera::camera_wiring::{CameraBackendParts, CameraLinks, start_camera_with_links};
use fasticamera::raw_stream_services::RawStreamServices;
use fasticamera::white_memory::WhiteMemoryHandoff;
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::WhiteSourceRegistry;

use super::awb_support::stream_passport;
use super::support::Journal;
use super::white_rig::{FIRST_TS, FRAME_NANOS, FrameImage, Rig, rig_or_skip, scene_with_light_change};

/// Сколько ждём реакции движка (он живёт на своём потоке).
pub const WAIT: Duration = Duration::from_secs(10);
/// Интервал меток сенсора у результатов fake-бэкенда.
const SENSOR_INTERVAL: Duration = Duration::from_nanos(16_666_667);
/// Сколько кадров сцены заготовлено (хватает на все тесты).
const SCENE_FRAMES: i64 = 200;
/// Кадр, с которого сцена меняет свет (лампа → дневной): белая не должна на это реагировать.
pub const LIGHT_CHANGES_AT: i64 = 20;

/// Что собирает стенд.
pub struct StandOptions {
    /// Реестр источников белого (рендер).
    pub sources: WhiteSourceRegistry,
    /// Открытая память белой (по умолчанию пустая: без сохранения).
    pub memory: WhiteMemoryHandoff,
}

pub struct Stand {
    pub rig: Rig,
    journal: Arc<Journal>,
    pub camera: CameraServices,
    pub control: Arc<FakeBackendControl>,
    pub answers: camera_contract::white::WhiteAnswerChannel,
    /// Указания движка рендеру (только чтение: когда движок разобрал ответ).
    directives: DirectiveSlot<WhiteDirective>,
    next_frame: i64,
    /// Поколение последнего ответа источника, которого уже дождались у движка.
    seen_answer: Option<camera_contract::three_a::DirectiveGeneration>,
}

impl Stand {
    /// `None` — GPU недоступен и тест осознанно пропущен (`FASTICAMERA_SKIP_GPU_TESTS=1`).
    pub fn start(options: StandOptions) -> Option<Self> {
        Self::start_with_config(options, CameraConfig::default())
    }

    /// То же с заданным конфигом камеры (например, выбор правила возобновления белого, как параметр запуска).
    pub fn start_with_config(options: StandOptions, camera_config: CameraConfig) -> Option<Self> {
        let links = CameraLinks {
            white_memory: options.memory,
            ..CameraLinks::default()
        };
        // Рендерная половина ящиков и реестр источников — как в корне композиции.
        let mut render_links = links.render_side(ExposureControllerKind::Vendor);
        render_links.white.sources = options.sources;
        let answers = render_links.white.answers.clone();
        let directives = render_links.white.directives.clone();
        let (rig, journal) = rig_or_skip(
            scene_with_light_change(SCENE_FRAMES, LIGHT_CHANGES_AT),
            render_links.white,
            CaptureColorHints::default(),
            WhiteBalanceConfig::default(),
        )?;

        let control = FakeBackendControl::new();
        let camera = start_camera_with_links(
            &camera_config,
            CameraBackendParts {
                factory: control.factory(),
                diagnostics: Arc::new(BackendDiagnostics::new(4)),
                raw_stream: RawStreamServices::detached(),
            },
            links,
        );
        Some(Self {
            rig,
            journal,
            camera,
            control,
            answers,
            directives,
            next_frame: 0,
            seen_answer: None,
        })
    }

    pub fn handle(&self) -> CameraHandle {
        self.camera.running().expect("движок запущен").handle().clone()
    }

    pub fn status(&self) -> CameraStatus {
        self.handle().status()
    }

    /// Ждёт состояния движка, подходящего под условие.
    pub fn wait_status(&self, what: &str, matches: impl Fn(&CameraStatus) -> bool) -> CameraStatus {
        self.handle()
            .wait_until_status(WAIT, matches)
            .unwrap_or_else(|| panic!("не дождались: {what}; статус {:?}", self.status().three_a.white))
    }

    /// Возобновление приложения: движок открывает камеру и начинает поток (как по `Resumed`).
    pub fn resume(&mut self) {
        self.handle().enter_foreground().expect("команда принята");
        self.wait_status("поток движка идёт", |status| {
            status.state.name() == "streaming"
        });
        // Камера открылась заново — рендер получает новый поток (так бывает на телефоне).
        self.rig.begin_stream(stream_passport());
    }

    /// Уход приложения в фон: движок закрывает камеру.
    pub fn pause(&mut self) {
        self.handle().enter_background().expect("команда принята");
        self.wait_status("камера закрыта", |status| {
            status.state.name() != "streaming"
        });
    }

    /// Гонит `count` кадров: кадр в рендер, его результат в движок, потом ждёт, пока движок обработает всё, что
    /// рендер успел ему сказать (ответ источника). Подсказки телефона на кадре — по номеру кадра.
    pub fn run_frames(&mut self, count: i64, hints: &dyn Fn(i64) -> CaptureColorHints) -> Vec<FrameImage> {
        let mut images = Vec::new();
        for _ in 0..count {
            let index = self.next_frame;
            self.next_frame += 1;
            self.rig.hints = hints(index);
            let shown = self
                .rig
                .present_new_frame(FIRST_TS + index * FRAME_NANOS, RequestOrigin::Repeating);
            images.push(shown.gray.is_some().then(|| self.rig.pixels()));
            self.feed_engine_one_result();
            self.wait_for_engine_to_read_new_answer();
        }
        images
    }

    /// Номер следующего кадра сцены (кадры одной сцены у стенда и у эталона совпадают по номеру).
    pub fn next_frame_index(&self) -> i64 {
        self.next_frame
    }

    /// Результат кадра в движок: `complete_frames` шлёт результат повторяющегося запроса текущей сессии.
    fn feed_engine_one_result(&self) {
        let before = self.status().stream.results_completed;
        self.control.complete_frames(1, SENSOR_INTERVAL);
        self.wait_status("движок принял результат кадра", |status| {
            status.stream.results_completed > before
        });
    }

    /// Если рендер положил новый ответ источника, ждём, пока движок его разберёт: заявка перестала ждать ИМЕННО
    /// этот ответ (принята, отказана или ответ признан чужим).
    fn wait_for_engine_to_read_new_answer(&mut self) {
        let Some((generation, answer)) = self.answers.newer_than(self.seen_answer) else {
            return;
        };
        self.seen_answer = Some(generation);
        self.wait_status("движок разобрал ответ источника", |status| {
            !matches!(
                status.three_a.white.request,
                WhiteRequestState::Waiting { request, .. } if request == answer.request
            )
        });
        // Вопросы слежения в статусе не видны, а в указании рендеру видны: разобранный ответ снимает заявку оттуда.
        Self::wait_until("указание без разобранной заявки", || {
            self.directives
                .latest()
                .is_none_or(|(_, directive)| directive.request.is_none_or(|request| request.id != answer.request))
        });
    }

    /// Запоминает поколение ответа, лежащего в ящике сейчас: ответы, которых тест сам не ждёт (положил руками),
    /// стенд не трактует как «рендер что-то сказал».
    pub fn mark_answers_seen(&mut self) {
        self.seen_answer = self.answers.newer_than(None).map(|(generation, _)| generation);
    }

    /// Завершение: поток рендера закрыт, validation без ошибок, утечек нет; движок останавливается при падении.
    pub fn finish(self) {
        let Self {
            rig, journal, camera, ..
        } = self;
        rig.finish(&journal);
        drop(camera);
    }

    /// Метка кадра, которой нет в сцене (для прямых ответов в ящик): значение не важно, но нужно для формы ответа.
    pub fn unrelated_frame_stamp() -> SensorTimestamp {
        SensorTimestamp::from_nanos(0)
    }

    /// Дожидается указанного момента с проверкой раз в миллисекунду (для условий вне статуса движка).
    pub fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !condition() {
            assert!(Instant::now() < deadline, "не дождались: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
