//! Общие помощники тестов: конфиг, готовый экран на 1080×2412 с живой камерой, пошаговый прогон.

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::three_a::{AeAfLock, MeteringRegion, PointOfInterest, PointRequestId, SensorPoint};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Point;

use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::exposure::ExposureRangeFact;
use crate::facts::{CameraFacts, CameraPresence, FocusPhase};
use crate::reduce::{ScreenTransition, reduce};
use crate::settings::AspectRatio;
use crate::state::ScreenState;
use crate::viewfinder_map::{MirrorMode, StreamGeometry};

/// Наш экран: рабочее разрешение из `layout.md` раздел 3.
pub(crate) const OUR_SURFACE: SurfaceSize = SurfaceSize {
    width_px: 1080,
    height_px: 2412,
};

/// Момент времени в миллисекундах от нуля шкалы.
pub(crate) fn at_ms(milliseconds: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(milliseconds * 1_000_000)
}

/// Основная камера OnePlus 11: сенсор 4096×3072, ориентация 90°, полезная область — весь буфер.
pub(crate) fn main_camera_stream() -> StreamGeometry {
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

/// Факты работающей камеры без точки интереса и блокировки.
pub(crate) fn live_facts() -> CameraFacts {
    CameraFacts {
        presence: CameraPresence::Live,
        point_of_interest: PointOfInterest::WholeFrame,
        lock: AeAfLock::Unlocked,
        focus: FocusPhase::Continuous,
        exposure_range: ExposureRangeFact::NotReported,
        stream: Some(main_camera_stream()),
        lens: crate::facts::LensFacts::NONE,
        // Большинство тестов проверяют вспышку как возможность камеры; «вспышки нет» — отдельные тесты.
        flash: crate::facts::FlashAvailability::Available,
    }
}

/// Факты с точкой камеры (центр региона и номер запроса).
pub(crate) fn facts_with_point(center: SensorPoint, request: u64) -> CameraFacts {
    let region = MeteringRegion::new(center, 0.1).expect("регион");
    CameraFacts {
        point_of_interest: PointOfInterest::Point {
            region,
            request: PointRequestId(request),
        },
        ..live_facts()
    }
}

/// Прогон действий с запоминанием состояния — как делало бы приложение.
pub(crate) struct Screen {
    pub(crate) state: ScreenState,
    pub(crate) config: ScreenConfig,
}

impl Screen {
    /// Экран с размером surface и живой камерой.
    pub(crate) fn ready() -> Self {
        let mut screen = Self::unprepared(ScreenConfig::default());
        screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
        screen.step(ScreenAction::CameraFactsChanged(live_facts()));
        screen
    }

    /// Экран только что создан: ни surface, ни фактов камеры.
    pub(crate) fn unprepared(config: ScreenConfig) -> Self {
        Self {
            state: ScreenState::new(&config).expect("валидный конфиг"),
            config,
        }
    }

    /// Применяет действие и сохраняет новое состояние.
    pub(crate) fn step(&mut self, action: ScreenAction) -> ScreenTransition {
        let transition = reduce(&self.state, action, &self.config);
        self.state = transition.state.clone();
        transition
    }

    /// Центр кадра выбранного соотношения, pt.
    pub(crate) fn frame_center(&self, aspect: AspectRatio) -> Point {
        self.state.layout().expect("раскладка").frame_rect(aspect).center()
    }

    /// Центр кадра текущего соотношения.
    pub(crate) fn current_frame_center(&self) -> Point {
        self.frame_center(self.state.settings().aspect)
    }

    /// Тап по центру кадра в момент `at`.
    pub(crate) fn tap_center(&mut self, at: MonotonicNanos) -> ScreenTransition {
        let point = self.current_frame_center();
        self.step(ScreenAction::ViewfinderTapped { point, at })
    }
}

/// Команда рендеру без зума и без каталога камер (до сессии 13 кадрирование было только таким).
pub(crate) fn plain_framing(
    layout: &crate::layout::ScreenLayout,
    aspect: crate::settings::AspectRatio,
) -> crate::layout::FramingIntent {
    layout.framing_intent(
        aspect,
        camera_contract::ids::LensFacing::Back,
        camera_contract::zoom::ZoomRatio::ONE,
        std::sync::Arc::new(camera_contract::zoom::ZoomCatalog::default()),
    )
}

// ---- зум (сессия 13) ----

use std::sync::Arc;

use camera_contract::geometry::PixelSize as ZoomPixelSize;
use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::lens_calibration::{OpticalAxis, UncalibratedReason};
use camera_contract::viewfinder_crop::ZoomFactor;
use camera_contract::zoom::{
    LensGroup, LensZoomPoint, PreviewMirror, ZoomCatalog, ZoomRange, ZoomRatio, ZoomStop, ZoomStopKind,
};

use crate::facts::{ActiveLens, LensFacts, LensSwitch, LensSwitchFailure};

/// Коэффициент из литерала теста.
pub(crate) fn ratio(value: f32) -> ZoomRatio {
    ZoomRatio::new(value).expect("коэффициент теста конечен и > 0")
}

/// ID камеры OnePlus 11 из каталога тестов: ширик, основная, фронтальная.
pub(crate) fn camera_id(id: &str) -> CameraId {
    CameraId::new(id).expect("ID теста непустой")
}

fn lens_point(role: LensRole, id: &str, native: f32, focal_mm: f32, mirror: PreviewMirror) -> LensZoomPoint {
    LensZoomPoint {
        role,
        camera: camera_id(id),
        native_ratio: ratio(native),
        equivalent_focal_mm: focal_mm,
        optical_axis: OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::NotReported,
        },
        active_array: ZoomPixelSize::new(4096, 3072),
        preview_mirror: mirror,
        max_digital_zoom: ZoomFactor::new(10.0).expect("зум теста"),
    }
}

/// Шкалы как у OnePlus 11 (решения 111, 115): задняя — ширик 0,593 / основная 1 / кроп 2× (до 10×), фронтальная 1…3×.
pub(crate) fn zoom_catalog() -> Arc<ZoomCatalog> {
    let stop = |value: f32, kind: ZoomStopKind| ZoomStop {
        ratio: ratio(value),
        kind,
    };
    Arc::new(ZoomCatalog {
        back: Some(LensGroup {
            facing: LensFacing::Back,
            lenses: vec![
                lens_point(LensRole::UltraWide, "3", 0.593, 14.0, PreviewMirror::None),
                lens_point(LensRole::Wide, "2", 1.0, 24.0, PreviewMirror::None),
            ],
            stops: vec![
                stop(0.593, ZoomStopKind::Lens(LensRole::UltraWide)),
                stop(1.0, ZoomStopKind::Lens(LensRole::Wide)),
                stop(2.0, ZoomStopKind::DigitalCrop),
            ],
            range: ZoomRange {
                min: ratio(0.593),
                max: ratio(10.0),
            },
        }),
        front: Some(LensGroup {
            facing: LensFacing::Front,
            lenses: vec![lens_point(LensRole::Front, "1", 1.0, 20.0, PreviewMirror::Horizontal)],
            stops: vec![stop(1.0, ZoomStopKind::Lens(LensRole::Front))],
            range: ZoomRange {
                min: ratio(1.0),
                max: ratio(3.0),
            },
        }),
    })
}

/// Факты живой камеры с каталогом зума и активным объективом `role` (ширик «3», основная «2», фронтальная «1»).
pub(crate) fn zoom_facts(role: LensRole) -> CameraFacts {
    let (camera, facing) = match role {
        LensRole::UltraWide => ("3", LensFacing::Back),
        LensRole::Wide | LensRole::Tele => ("2", LensFacing::Back),
        LensRole::Front => ("1", LensFacing::Front),
    };
    CameraFacts {
        lens: LensFacts {
            catalog: Some(zoom_catalog()),
            active: Some(ActiveLens {
                camera: camera_id(camera),
                role,
                facing,
            }),
            switch: LensSwitch::Steady,
            last_failure: None,
        },
        ..live_facts()
    }
}

/// Те же факты, но с неудачей смены объектива номер `sequence` (на камеру `target`).
pub(crate) fn zoom_facts_with_failure(role: LensRole, sequence: u64, target: &str) -> CameraFacts {
    let mut facts = zoom_facts(role);
    facts.lens.last_failure = Some(LensSwitchFailure {
        sequence,
        target: camera_id(target),
    });
    facts
}

impl Screen {
    /// Экран с размером surface, живой камерой и каталогом зума; активна основная камера.
    pub(crate) fn zoomed(role: LensRole) -> Self {
        let mut screen = Self::unprepared(ScreenConfig::default());
        screen.step(ScreenAction::SurfaceResized(OUR_SURFACE));
        screen.step(ScreenAction::CameraFactsChanged(zoom_facts(role)));
        screen
    }
}
