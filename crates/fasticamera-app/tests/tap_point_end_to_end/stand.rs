//! Стенд сквозного теста «точка тапа не прилипает» (решение 241).
//!
//! НАСТОЯЩИЙ движок камеры (`CameraRuntime` на fake-бэкенде, собранный продуктовым `start_camera_with_links`, наш AE
//! `ExposureControllerKind::Own`) + НАСТОЯЩИЙ бэкенд показа (RADV headless, полная validation) на общих ящиках AE:
//! указания пишет движок, предложения и наблюдения «точка устарела» пишет рендер. Тест играет только камеру:
//! * экспозиция кадра — то, что движок ПОСТАВИЛ в повторяющийся запрос fake-бэкенда, дошедшее до кадров через
//!   задержку конвейера сенсора ([`SENSOR_DELAY_FRAMES`]);
//! * яркость кадра пропорциональна «выдержка × ISO» этого кадра и отражению плитки сцены ([`SceneLayout`]);
//! * результат кадра (с той же экспозицией) уходит в движок как результат повторяющегося запроса.
//!
//! Тапы и фон — настоящие команды через [`CameraHandle`]: точку в указание рендеру ставит сам движок.

use std::sync::Arc;
use std::time::Duration;

use camera_android::BackendDiagnostics;
use camera_contract::three_a::{AeAfLock, PointOfInterest, SensorPoint};
use camera_core::backend::BackendEvent;
use camera_core::fake::FakeBackendControl;
use camera_core::point_hold::{PointHold, PointReleaseCause};
use camera_core::request_controls::ExposurePlan;
use camera_core::status::CameraStatus;
use camera_core::{CameraEvent, CameraHandle};
use fasticamera::auto_exposure_link::ExposureControllerKind;
use fasticamera::camera_config::CameraConfig;
use fasticamera::camera_service::CameraServices;
use fasticamera::camera_wiring::{CameraBackendParts, CameraLinks, start_camera_with_links};
use fasticamera::raw_stream_services::RawStreamServices;
use isp_model::transfer::srgb_decode;
use render_loop::config::AutoExposureConfig;
use render_loop::viewfinder::AutoExposureLinks;

use super::ae_rig::{
    AeRig, RigSetup, RigStream, SENSOR_DELAY_FRAMES, SceneLayout, Shot, manual_lamp_white, rig_with_links,
};
use super::awb_support::stream_passport;
use super::support::Journal;

/// Сколько ждём реакции движка (он живёт на своём потоке).
const WAIT: Duration = Duration::from_secs(10);
/// Масштаб сцены стенда: цель замера ≈ 1 с·ISO на ровной сцене (как у `ae_rig`).
const SCENE_SCALE: f64 = super::ae_rig::SCENE_SCALE;
/// Где тапаем: середина левой половины кадра (там «тёмное место» сцены).
pub const TAP_X: f32 = 0.25;
/// Вертикаль тапа: середина кадра.
pub const TAP_Y: f32 = 0.5;

/// Что показывал экран и что стояло на кадрах за окно последних кадров (среднее по логарифму).
#[derive(Debug, Clone, Copy)]
pub struct Reading {
    /// «Выдержка × ISO» кадров.
    pub product: f64,
    /// Яркость плашки на экране, линейный свет.
    pub luminance: f64,
}

impl Reading {
    /// Разница произведений экспозиции `self` над `other`, EV.
    pub fn product_ev_above(&self, other: &Reading) -> f64 {
        (self.product / other.product).log2()
    }

    /// Разница яркости показанных пикселей `self` над `other`, EV.
    pub fn luminance_ev_above(&self, other: &Reading) -> f64 {
        (self.luminance / other.luminance).log2()
    }
}

pub struct Stand {
    pub rig: AeRig,
    journal: Arc<Journal>,
    camera: CameraServices,
    control: Arc<FakeBackendControl>,
    /// Произведение, стоявшее в повторяющемся запросе в начале каждого кадра (индекс — номер кадра стенда).
    commanded: Vec<f64>,
    /// Все события движка, снятые с очереди (ничего не теряем между проверками).
    events: Vec<CameraEvent>,
}

impl Stand {
    /// `None` — GPU недоступен и тест осознанно пропущен (`FASTICAMERA_SKIP_GPU_TESTS=1`).
    pub fn start(hold: PointHold, layout: SceneLayout) -> Option<Self> {
        let mut config = CameraConfig::default();
        config.engine.three_a.point_hold = hold;
        let links = CameraLinks::default();
        // Рендерная половина ящиков — как в корне композиции; наш AE (Own) даёт ящики указаний и предложений.
        let exposure_end = links
            .render_side(ExposureControllerKind::Own)
            .auto_exposure
            .expect("наш AE: у рендера есть ящики");
        let (rig, journal) = rig_with_links(
            RigSetup {
                auto_exposure: Some(AutoExposureConfig::default()),
                // Белая задана стендом: источник белого к точке тапа отношения не имеет.
                white_balance: Some(manual_lamp_white()),
                scene: "checker_bb2800",
                start_product: 1.0,
                scene_scale: SCENE_SCALE,
                allow_clipping: true,
                layout,
                stream: RigStream::Reference,
            },
            Some(AutoExposureLinks {
                directives: exposure_end.directives,
                proposals: exposure_end.proposals,
            }),
        )?;
        let control = FakeBackendControl::new();
        let camera = start_camera_with_links(
            &config,
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
            commanded: Vec::new(),
            events: Vec::new(),
        })
    }

    pub fn handle(&self) -> CameraHandle {
        self.camera.running().expect("движок запущен").handle().clone()
    }

    pub fn status(&self) -> CameraStatus {
        self.handle().status()
    }

    fn wait_status(&self, what: &str, matches: impl Fn(&CameraStatus) -> bool) -> CameraStatus {
        self.handle()
            .wait_until_status(WAIT, matches)
            .unwrap_or_else(|| panic!("не дождались: {what}; статус {:?}", self.status().three_a))
    }

    /// Возобновление приложения: движок открывает камеру и начинает поток; рендер получает новый поток кадров.
    pub fn resume(&mut self) {
        self.handle().enter_foreground().expect("команда принята");
        self.wait_status("поток движка идёт", |status| {
            status.state.name() == "streaming"
        });
        self.rig.epoch = self
            .rig
            .stream
            .producer()
            .begin_stream(Arc::new(stream_passport()))
            .epoch;
    }

    /// Уход приложения в фон: камера закрыта, поток кадров рендера завершён.
    pub fn pause(&mut self) {
        self.handle().enter_background().expect("команда принята");
        self.wait_status("камера закрыта", |status| {
            status.state.name() != "streaming"
        });
        self.rig.stream.producer().end_stream(self.rig.epoch);
    }

    /// Тап по тёмному месту (обычный).
    pub fn tap(&self) {
        self.handle().focus_and_expose_at(tap_point()).expect("команда принята");
    }

    /// Долгий тап (блокировка AE/AF по точке).
    pub fn long_tap(&self) {
        self.handle()
            .lock_focus_and_exposure_at(tap_point())
            .expect("команда принята");
    }

    /// Что движок держит как точку и блокировку сейчас.
    pub fn point_and_lock(&self) -> (PointOfInterest, AeAfLock) {
        let three_a = self.status().three_a;
        (three_a.point, three_a.lock)
    }

    /// Ждёт, пока движок примет тап: точка встала в указание.
    pub fn wait_point_held(&self) {
        self.wait_status("точка тапа принята движком", |status| {
            matches!(status.three_a.point, PointOfInterest::Point { .. })
        });
    }

    pub fn point_is_held(&self) -> bool {
        matches!(self.point_and_lock().0, PointOfInterest::Point { .. })
    }

    pub fn whole_frame(&self) -> bool {
        matches!(self.point_and_lock().0, PointOfInterest::WholeFrame)
    }

    /// Сколько раз движок отпускал точку (счётчик статуса).
    pub fn points_released(&self) -> u64 {
        self.status().three_a.counters.points_released
    }

    /// События «движок отпустил точку» (причины по порядку), включая ещё не снятые с очереди.
    pub fn released_causes(&mut self) -> Vec<PointReleaseCause> {
        self.events.extend(self.handle().drain_events(4096));
        self.events
            .iter()
            .filter_map(|event| match event {
                CameraEvent::PointOfInterestReleased { cause, .. } => Some(*cause),
                _ => None,
            })
            .collect()
    }

    /// Сменить сцену под камерой (свет и раскладка плиток).
    pub fn change_scene(&mut self, layout: SceneLayout) {
        self.rig.layout = layout;
    }

    /// Одна экспозиция из повторяющегося запроса fake-бэкенда: (выдержка, ISO).
    fn requested_exposure(&self) -> (Duration, u32) {
        match self.control.repeating_request().map(|(_, controls)| controls.exposure) {
            Some(ExposurePlan::Sensor { exposure_time, iso }) => (exposure_time.duration(), iso.value()),
            other => panic!("наш AE обязан ставить ручной запрос сенсора, а не {other:?}"),
        }
    }

    /// Кадр камеры: экспозиция из запроса с задержкой конвейера, сцена, показ, результат кадра в движок.
    pub fn step(&mut self) -> Shot {
        let frame = usize::try_from(self.rig.frame).expect("кадр");
        let (time, iso) = self.requested_exposure();
        self.commanded.push(time.as_secs_f64() * f64::from(iso));
        // Кадр несёт то, что стояло в запросе SENSOR_DELAY_FRAMES кадров назад (первые кадры — самое раннее).
        let delay = usize::try_from(SENSOR_DELAY_FRAMES).expect("задержка");
        let product = self.commanded[frame.saturating_sub(delay)];
        self.rig.sensor_product = product;

        let sensor_ts = self.rig.timestamp();
        self.rig.insert_scene(sensor_ts, product);
        let mut details = self
            .rig
            .result_with(sensor_ts, product, camera_contract::three_a::RequestOrigin::Repeating);
        // Результат несёт ту же экспозицию, что на кадре (выдержка и ISO запроса), а не сквозной ISO 100 стенда.
        let delayed_time = Duration::from_secs_f64(product / f64::from(iso));
        details.result.exposure_time = Some(delayed_time);
        details.result.sensitivity_iso = Some(iso);
        self.rig.offer(sensor_ts, Some(details));
        let shown = self.rig.render();
        let shot = Shot {
            frame: self.rig.frame,
            sensor_product: product,
            // Яркость экрана — среднее по ВСЕМУ изображению: центр кадра лежит на стыке половин сцены.
            display_luminance: shown.then(|| self.display_mean_luminance()),
            pixel: shown.then(|| self.rig.display_pixel()),
        };
        self.rig.history.push(shot);
        self.feed_engine(details);
        self.rig.frame += 1;
        shot
    }

    /// Средняя яркость всех пикселей показанного изображения, линейный свет экрана (sRGB-основные).
    fn display_mean_luminance(&self) -> f64 {
        let image = self
            .rig
            .backend
            .read_viewfinder_image()
            .expect("readback изображения видоискателя")
            .expect("изображение видоискателя есть");
        let sum: f64 = image
            .pixels
            .iter()
            .map(|pixel| {
                let [red, green, blue] = [pixel[0], pixel[1], pixel[2]].map(|channel| f64::from(srgb_decode(channel)));
                0.2126 * red + 0.7152 * green + 0.0722 * blue
            })
            .sum();
        sum / image.pixels.len() as f64
    }

    /// Результат кадра в движок и ожидание, пока движок его обработал (в том же проходе он читает ящик
    /// предложений и наблюдений рендера, ставит запрос).
    fn feed_engine(&self, details: camera_contract::frame_passport::FrameCaptureDetails) {
        let before = self.status().stream.results_completed;
        let session = self
            .control
            .current_session_generation()
            .expect("сессия камеры открыта");
        self.control.send_event(BackendEvent::CaptureCompleted {
            session,
            result: details.result,
        });
        self.wait_status("движок принял результат кадра", |status| {
            status.stream.results_completed > before
        });
    }

    pub fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.step();
        }
    }

    /// Среднее (по логарифму) произведения экспозиции и яркости экрана за последние `window` показанных кадров.
    pub fn reading(&self, window: usize) -> Reading {
        let shown: Vec<&Shot> = self
            .rig
            .history
            .iter()
            .rev()
            .filter(|shot| shot.display_luminance.is_some())
            .take(window)
            .collect();
        assert!(shown.len() == window, "показанных кадров меньше окна: {}", shown.len());
        let mean_log = |value: &dyn Fn(&Shot) -> f64| {
            (shown.iter().map(|shot| value(shot).log2()).sum::<f64>() / window as f64).exp2()
        };
        Reading {
            product: mean_log(&|shot| shot.sensor_product),
            luminance: mean_log(&|shot| shot.display_luminance.expect("показан")),
        }
    }

    /// Разброс (макс/мин, EV) яркости показанных пикселей за окно: «яркость стабильна».
    pub fn luminance_spread_ev(&self, window: usize) -> f64 {
        let values: Vec<f64> = self
            .rig
            .history
            .iter()
            .rev()
            .filter_map(|shot| shot.display_luminance)
            .take(window)
            .collect();
        let max = values.iter().copied().fold(f64::MIN, f64::max);
        let min = values.iter().copied().fold(f64::MAX, f64::min);
        (max / min).log2()
    }

    /// Завершение: поток рендера закрыт, validation без ошибок, утечек нет; движок останавливается при падении.
    pub fn finish(self) {
        let Self {
            rig, journal, camera, ..
        } = self;
        rig.finish(&journal);
        drop(camera);
    }
}

fn tap_point() -> SensorPoint {
    SensorPoint::new(TAP_X, TAP_Y).expect("точка в кадре")
}
