//! Водитель экрана камеры для golden: настоящий фасад `CameraScreen`, настоящие касания и действия,
//! детерминированное время (кадры по шагу 16 мс от нуля, `present_time` задаётся самим водителем).
//!
//! Состояние экрана НЕ подменяется: до нужного вида экран доводится теми же путями, что и в приложении
//! (касание → жест → действие → редьюсер → виджеты → список рисования).

use std::sync::Arc;

use camera_contract::control::ExposureBias;
use camera_contract::fixtures::{camera_id, front_camera, main_wide_camera, ultra_wide_camera};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::three_a::{AeAfLock, MeteringRegion, PointOfInterest, PointRequestId, SensorPoint};
use camera_contract::zoom::ZoomCatalog;
use camera_core::role_classifier::classify_catalog;
use camera_core::zoom::{ZoomConfig, build_zoom_catalog};
use camera_screen::exposure::ExposureRangeFact;
use camera_screen::viewfinder_map::{MirrorMode, StreamGeometry};
use camera_screen::{
    ActiveLens, CameraFacts, CameraPresence, CameraScreen, Effect, FocusPhase, LensFacts, LensSwitch, ScreenAction,
    ScreenCameraIntent, ScreenConfig, ScreenTouch,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_core::geometry::Point;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::light::LightDirection;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::UiKitTokens;

/// Наш экран: ширина surface в px (`layout.md` раздел 3).
pub const WIDTH_PX: u32 = 1080;
/// Наш экран: высота surface в px.
pub const HEIGHT_PX: u32 = 2412;
/// 1 pt = ширина / 440 px (решение 82).
pub const PX_PER_PT: f32 = WIDTH_PX as f32 / 440.0;
/// Шаг кадров водителя: 60 Гц, мс. Шаг фиксирован, поэтому пружины в каждом прогоне идут по одним и тем же моментам.
const FRAME_STEP_MS: i64 = 16;
/// Достаточно, чтобы все пружины и появления доехали до покоя (самые долгие — шторка и поворот значков, < 1 с).
pub const SETTLE_MS: i64 = 1_200;
/// Удержание долгого тапа: порог жеста — 0,5 с, берём с запасом.
const LONG_PRESS_HOLD_MS: i64 = 560;

/// Момент `value` мс от нуля шкалы водителя.
pub fn at_ms(value: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(value * 1_000_000)
}

/// Геометрия потока основной камеры OnePlus 11: сенсор 4096×3072, ориентация 90°, полезная область — весь буфер.
fn main_camera_stream() -> StreamGeometry {
    StreamGeometry {
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(4096, 3072),
        },
        active_array_size: PixelSize::new(4096, 3072),
        sensor_orientation: SensorOrientation::Degrees90,
        mirror: MirrorMode::None,
    }
}

/// Факты работающей камеры: точки интереса и блокировки нет.
pub fn live_facts() -> CameraFacts {
    CameraFacts {
        presence: CameraPresence::Live,
        point_of_interest: PointOfInterest::WholeFrame,
        lock: AeAfLock::Unlocked,
        focus: FocusPhase::Continuous,
        exposure_range: ExposureRangeFact::NotReported,
        stream: Some(main_camera_stream()),
        lens: live_lens_facts(),
        // Golden-кадры снимают раскладку экрана со значком вспышки (он вернётся в 14d, решение 138);
        // скрытие значка без вспышки доказывает `camera-screen` тест рисования, а не golden.
        flash: camera_screen::FlashAvailability::Available,
        white: camera_screen::WhiteFacts::STARTING,
    }
}

/// Камера роли `wide` в каталоге OnePlus 11.
const MAIN_CAMERA: &str = "2";

/// Шкала зума OnePlus 11, построенная теми же функциями движка, что и на телефоне (образцы камер по фактам 00).
pub fn oneplus_zoom_catalog() -> Arc<ZoomCatalog> {
    let cameras = vec![main_wide_camera(), ultra_wide_camera(), front_camera()];
    let classified = classify_catalog(cameras, &camera_core::config::CameraEngineConfig::default().roles)
        .expect("образцы камер без повторных ID");
    Arc::new(build_zoom_catalog(&classified.catalog, &ZoomConfig::default()))
}

/// Объективы работающей основной камеры: шкала OnePlus 11 (ширик 0,59 / основная 1 / фронтальная), активна `wide`.
pub fn live_lens_facts() -> LensFacts {
    LensFacts {
        catalog: Some(oneplus_zoom_catalog()),
        active: Some(ActiveLens {
            camera: camera_id(MAIN_CAMERA),
            role: LensRole::Wide,
            facing: LensFacing::Back,
        }),
        switch: LensSwitch::Steady,
        last_failure: None,
    }
}

/// Факты камеры, подтвердившей блокировку AE/AF на точке `center` (запрос номер `request`).
pub fn locked_facts_at(center: SensorPoint, request: u64) -> CameraFacts {
    let region = MeteringRegion::new(center, 0.1).expect("регион 10 % допустим");
    CameraFacts {
        point_of_interest: PointOfInterest::Point {
            region,
            request: PointRequestId(request),
        },
        lock: AeAfLock::Locked,
        focus: FocusPhase::Focused,
        ..live_facts()
    }
}

pub struct ScreenDriver {
    screen: CameraScreen,
    now_ms: i64,
    last_list: DrawList,
    /// Что рендер сообщает экрану о кадре под UI (по умолчанию — свежий кадр камеры).
    backdrop: BackdropContinuity,
}

impl ScreenDriver {
    /// Экран с живой камерой: первый кадр построен (сцена собирается на первом кадре), факты камеры приняты,
    /// эффекты первого кадра (кадрирование) отброшены — как делает приложение до первого действия пользователя.
    pub fn new() -> Self {
        let screen =
            CameraScreen::new(ScreenConfig::default(), UiKitTokens::default(), MotionPreference::Full).expect("экран");
        let mut driver = Self {
            screen,
            now_ms: 0,
            last_list: DrawList::new(LightDirection::from_angle_degrees(45.0)),
            backdrop: BackdropContinuity::Live,
        };
        driver.frame();
        driver.send(ScreenAction::CameraFactsChanged(live_facts()));
        driver.run(SETTLE_MS);
        driver.screen.drain_effects();
        driver
    }

    pub fn screen(&self) -> &CameraScreen {
        &self.screen
    }

    /// Текущее время водителя (метка для действий, которые несут `at`).
    pub fn now(&self) -> MonotonicNanos {
        at_ms(self.now_ms)
    }

    /// Что рендер сообщает экрану о кадре под UI на следующих кадрах (удержание кадра на переходе камеры).
    pub fn set_backdrop(&mut self, backdrop: BackdropContinuity) {
        self.backdrop = backdrop;
    }

    /// Последний построенный список рисования.
    pub fn list(&self) -> &DrawList {
        &self.last_list
    }

    /// Эффекты, накопленные экраном с прошлого вызова.
    pub fn drain_effects(&mut self) -> Vec<Effect> {
        self.screen.drain_effects()
    }

    /// Один кадр на текущем времени.
    pub fn frame(&mut self) {
        let request = UiFrameRequest {
            present_time: at_ms(self.now_ms),
            logical_size: PxSize::new(WIDTH_PX as f32, HEIGHT_PX as f32),
            px_per_pt: PX_PER_PT,
            backdrop: self.backdrop,
        };
        self.last_list = self.screen.produce_frame(&request).expect("экран строит кадр");
    }

    /// Проходит `duration_ms` кадрами по 60 Гц (последний кадр — на конце интервала).
    pub fn run(&mut self, duration_ms: i64) {
        let end = self.now_ms + duration_ms;
        while self.now_ms < end {
            self.now_ms = (self.now_ms + FRAME_STEP_MS).min(end);
            self.frame();
        }
    }

    /// Действие (факты камеры, ориентация, выбор в меню…) и кадр, на котором оно синхронизируется с виджетами.
    pub fn act(&mut self, action: ScreenAction) {
        self.send(action);
        self.run(FRAME_STEP_MS);
    }

    /// «Сохранено» вместе с картинкой миниатюры (так их доставляет учёт снимков приложения, 14g) и кадр после.
    pub fn save_with_picture(
        &mut self,
        request: camera_screen::shutter::ShotRequestId,
        picture: ui_draw::picture::UiPicture,
    ) {
        self.screen.handle_saved_with_picture(request, picture);
        self.run(FRAME_STEP_MS);
    }

    fn send(&mut self, action: ScreenAction) {
        self.screen.handle_action(action);
        self.frame();
    }

    fn touch(&mut self, phase: TouchPhase, point: Point, at: i64) {
        self.screen.handle_touch(ScreenTouch::Sample(TouchSample {
            pointer: PointerId::new(0),
            phase,
            position: SurfacePoint {
                x_px: (point.x * f64::from(PX_PER_PT)) as f32,
                y_px: (point.y * f64::from(PX_PER_PT)) as f32,
            },
            time: at_ms(at),
        }));
    }

    /// Короткий тап в точке `point` (pt): касание, отрыв через 50 мс, кадр.
    pub fn tap(&mut self, point: Point) {
        let start = self.now_ms;
        self.touch(TouchPhase::Began, point, start);
        self.touch(TouchPhase::Ended, point, start + 50);
        self.now_ms = start + 60;
        self.frame();
    }

    /// Медленная протяжка `from → to` (pt) с задержкой в конце: содержимое останавливается там, где отпущен палец.
    pub fn drag(&mut self, from: Point, to: Point) {
        const STEPS: i64 = 24;
        let start = self.now_ms;
        self.touch(TouchPhase::Began, from, start);
        for step in 1..=2 * STEPS {
            // Вторая половина — задержка на месте: скорость отпускания нулевая.
            let t = (step.min(STEPS)) as f64 / STEPS as f64;
            let point = Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
            self.touch(TouchPhase::Moved, point, start + step * FRAME_STEP_MS);
        }
        self.touch(TouchPhase::Ended, to, start + (2 * STEPS + 1) * FRAME_STEP_MS);
        self.now_ms = start + (2 * STEPS + 2) * FRAME_STEP_MS;
        self.frame();
    }

    /// Долгий тап: палец неподвижен, жест срабатывает по таймеру на кадре, потом отрыв.
    pub fn long_press(&mut self, point: Point) {
        let start = self.now_ms;
        self.touch(TouchPhase::Began, point, start);
        self.now_ms = start + LONG_PRESS_HOLD_MS;
        self.frame();
        self.touch(TouchPhase::Ended, point, self.now_ms + 10);
        self.now_ms += 20;
        self.frame();
    }

    /// Точка сенсора, на которую камеру просили заблокироваться (из эффектов). `None` — намерения не было.
    pub fn lock_target_in(effects: &[Effect]) -> Option<SensorPoint> {
        effects.iter().find_map(|effect| match effect {
            Effect::Camera(ScreenCameraIntent::LockFocusAndExposureAt(point)) => Some(*point),
            _ => None,
        })
    }

    /// Восстановление EV «камерой» (программная установка шкалы без эха камере).
    pub fn restore_exposure(&mut self, ev: f64) {
        let bias = ExposureBias::from_ev(ev as f32).expect("EV в допустимых пределах");
        self.act(ScreenAction::ExposureRestored { bias });
    }

    /// Центр кадра текущего соотношения, pt.
    pub fn frame_center(&self) -> Point {
        let state = self.screen.state();
        state
            .layout()
            .expect("раскладка есть после первого кадра")
            .frame_rect(state.settings().aspect)
            .center()
    }

    /// Центр последнего члена стекла-круга заданного диаметра в px (затвор Ø197, кнопка меню и миниатюра Ø≈118).
    pub fn glass_circle_center_px(&self, diameter_px: f32) -> Option<(f32, f32)> {
        self.list().leaf_items().find_map(|item| match item {
            DrawItem::Glass(group) => group.members.iter().find_map(|member| match member.geometry {
                ShapeGeometry::Circle { diameter } if (diameter - diameter_px).abs() < 2.0 => {
                    Some((member.transform.tx, member.transform.ty))
                }
                _ => None,
            }),
            _ => None,
        })
    }

    /// Рамка стекла панели (первый член-скруглённый прямоугольник) в px: `(x0, y0, x1, y1)`.
    pub fn glass_panel_rect_px(&self) -> Option<(f32, f32, f32, f32)> {
        self.list().leaf_items().find_map(|item| match item {
            DrawItem::Glass(group) => group.members.iter().find_map(|member| match member.geometry {
                ShapeGeometry::RoundedRect { size, .. } => {
                    let (half_width, half_height) = (size.width * 0.5, size.height * 0.5);
                    Some((
                        member.transform.tx - half_width,
                        member.transform.ty - half_height,
                        member.transform.tx + half_width,
                        member.transform.ty + half_height,
                    ))
                }
                _ => None,
            }),
            _ => None,
        })
    }

    /// Центр иконки `icon` в px (первой найденной).
    pub fn icon_center_px(&self, icon: IconId) -> Option<(f32, f32)> {
        self.list().leaf_items().find_map(|item| match item {
            DrawItem::Icon(item) if item.icon == icon => Some((item.transform.tx, item.transform.ty)),
            _ => None,
        })
    }
}
