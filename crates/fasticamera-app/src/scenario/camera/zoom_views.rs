//! Срезы наблюдения для исполнителя `zoom` (сессия 13): что он знает о движке и об экране. Сам статус движка и отчёт
//! экрана тяжёлые, а тесту незачем их собирать целиком, поэтому исполнитель получает только эти срезы.

use std::sync::Arc;

use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::zoom::ZoomCatalog;
use camera_core::CameraStatus;

use crate::camera_screen_effects::RecentHapticTimes;
use crate::camera_screen_source::{ScreenReport, ZoomView};

/// Объектив, который сейчас отдаёт кадры, глазами исполнителя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLensView {
    pub role: LensRole,
    pub camera: CameraId,
    pub facing: LensFacing,
}

/// Что исполнителю нужно знать о движке (срез `CameraStatus`: сам статус тяжёлый и тесту незачем его собирать).
#[derive(Debug, Clone)]
pub struct EngineView {
    /// Поток идёт.
    pub streaming: bool,
    pub active: Option<ActiveLensView>,
    pub catalog: Arc<ZoomCatalog>,
    /// Измеренная частота потока текущей сессии по меткам сенсора, к/с.
    pub measured_fps: Option<f64>,
}

impl EngineView {
    pub fn of(status: &CameraStatus) -> Self {
        Self {
            streaming: matches!(status.state, camera_core::CameraState::Streaming { .. }),
            active: status.lens.active.as_ref().map(|active| ActiveLensView {
                role: active.role,
                camera: active.camera.clone(),
                facing: active.facing,
            }),
            catalog: Arc::clone(&status.lens.zoom_catalog),
            measured_fps: status.stream.measured_frames_per_second,
        }
    }
}

/// Что исполнителю нужно знать об экране камеры (срез отчёта).
#[derive(Debug, Clone, Copy)]
pub struct ScreenView {
    pub zoom: ZoomView,
    /// Когда принимались последние вибрации (кольцо исполнителя эффектов).
    pub haptics: RecentHapticTimes,
    /// Сколько намерений объектива экран положил в ящик за всё время.
    pub lens_published: u64,
}

impl ScreenView {
    pub fn of(report: &ScreenReport) -> Self {
        Self {
            zoom: report.state.zoom,
            haptics: report.effects.haptics.recent_queued,
            lens_published: report.effects.lens.published,
        }
    }
}

/// Камера роли по каталогу зума.
pub fn camera_of_role(catalog: &ZoomCatalog, role: LensRole) -> Option<CameraId> {
    [catalog.back.as_ref(), catalog.front.as_ref()]
        .into_iter()
        .flatten()
        .find_map(|group| group.lens_for_role(role))
        .map(|lens| lens.camera.clone())
}
