//! Что экран ПОКАЗАЛ во время шага сценария `zoom` (сессия 13a, расследование первого переключения на телефоне): приращения счётчиков
//! непрерывности и гистограмма меток показанных кадров.
//!
//! Зачем. Счётчик «чёрных кадров там, где ждали картинку» и счётчик удержанных кадров ничего не говорят о кадрах других
//! видов: «чёрный, картинки ещё не было» (`NoPicture`), кадры без нового кадра камеры (`crop: None`, пересборка) или кадры
//! со старой камерой. Здесь они разложены по корзинам «метка + камера», у каждой — когда показан первый кадр.
//!
//! Всё — чистые функции от снимков счётчиков и кольца кадров: без часов и ввода-вывода.

use std::collections::BTreeMap;

use render_loop::viewfinder::{
    BlackKind, ContinuityCounters, FrameLabel, FrameRecord, TransitionKind, ViewfinderCounters,
};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;

use super::zoom_record::ms_between;

/// Приращения счётчиков непрерывности и ISP за окно (шаг или прогон), которых нет в других полях записи шага.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ContinuityDelta {
    /// Удержаний началось. Смена объектива без удержания — нулевое значение при `held_frames == 0`.
    pub holds_started: u64,
    /// Почему они начались (сумма равна `holds_started`): какое условие показа нового кадра не выполнено.
    pub hold_causes: HoldCauses,
    /// Удержаний закончилось по условию готовности цвета и яркости (норма).
    pub holds_revealed_by_condition: u64,
    /// Кадров показано живыми.
    pub live_frames_presented: u64,
    /// Чёрных кадров «картинки камеры ещё не было». Во время шага (картинка уже была) должно быть 0:
    /// такие кадры значат, что удержание не началось и экран был чёрным.
    pub black_frames_before_first_picture: u64,
    /// Удержаний, начавшихся без картинки у ISP (нечего держать, кадры чёрные).
    pub holds_without_image: u64,
    /// Кадров нового потока, обработанных ISP, но заменённых на экране удержанным кадром.
    pub frames_hidden_by_continuity: u64,
    /// Кадров, обработанных ISP и скрытых на холодном старте белой (цвет нового потока ещё неизвестен).
    pub frames_hidden_cold_start: u64,
    /// Сколько раз кадр экрана собран из прежнего изображения без нового кадра камеры (пересборка).
    pub recomposed: u64,
}

/// Причины начала удержаний за окно (приращения счётчиков рендера `holds_started_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct HoldCauses {
    /// Поток закрылся (`StreamInactive`). При повышении тёплой — нарушение инварианта обмена.
    pub stream_inactive: u64,
    /// Цвет кадра нового потока ещё неизвестен (AWB).
    pub color_unknown: u64,
    /// У яркости нового потока не было цели: ни своего замера, ни объявления движка для камеры кадра.
    pub exposure_without_target: u64,
    /// Цель яркости была, но уровень кадра дальше допуска.
    pub exposure_level_off: u64,
}

impl HoldCauses {
    fn between(start: &ContinuityCounters, end: &ContinuityCounters) -> Self {
        Self {
            stream_inactive: end
                .holds_started_stream_inactive
                .saturating_sub(start.holds_started_stream_inactive),
            color_unknown: end
                .holds_started_color_unknown
                .saturating_sub(start.holds_started_color_unknown),
            exposure_without_target: end
                .holds_started_exposure_without_target
                .saturating_sub(start.holds_started_exposure_without_target),
            exposure_level_off: end
                .holds_started_exposure_level_off
                .saturating_sub(start.holds_started_exposure_level_off),
        }
    }
}

impl ContinuityDelta {
    /// Приращения от снимков `start` до `end` (счётчики не убывают; убыль, если вдруг, даёт 0).
    pub fn between(
        continuity_start: &ContinuityCounters,
        continuity_end: &ContinuityCounters,
        isp_start: &ViewfinderCounters,
        isp_end: &ViewfinderCounters,
    ) -> Self {
        Self {
            holds_started: continuity_end
                .holds_started
                .saturating_sub(continuity_start.holds_started),
            hold_causes: HoldCauses::between(continuity_start, continuity_end),
            holds_revealed_by_condition: continuity_end
                .holds_revealed_by_condition
                .saturating_sub(continuity_start.holds_revealed_by_condition),
            live_frames_presented: continuity_end
                .live_frames_presented
                .saturating_sub(continuity_start.live_frames_presented),
            black_frames_before_first_picture: continuity_end
                .black_frames_before_first_picture
                .saturating_sub(continuity_start.black_frames_before_first_picture),
            holds_without_image: continuity_end
                .holds_without_image
                .saturating_sub(continuity_start.holds_without_image),
            frames_hidden_by_continuity: continuity_end
                .frames_hidden_by_continuity
                .saturating_sub(continuity_start.frames_hidden_by_continuity),
            frames_hidden_cold_start: isp_end
                .frames_hidden_cold_start
                .saturating_sub(isp_start.frames_hidden_cold_start),
            recomposed: isp_end.recomposed.saturating_sub(isp_start.recomposed),
        }
    }
}

/// Корзина показанных кадров окна: одна метка кадра и одна камера (или «нового кадра камеры не было»).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PresentedBucket {
    /// Метка кадра: `live`, `held_last_frame`, `black_no_picture`, `black_picture_expected`,
    /// `black_camera_unavailable`, `black_picture_unavailable`.
    pub label: &'static str,
    /// Камера кадрирования кадра; `null` — в этом показе нового кадра камеры не было (`crop: None`, пересборка).
    pub camera: Option<String>,
    /// Сколько кадров этой корзины показано в окне.
    pub frames: u64,
    /// Через сколько миллисекунд от действия показан первый кадр корзины.
    pub first_at_ms: f64,
}

/// Имя метки кадра для отчёта (стабильное, не зависит от `Debug`).
pub fn label_name(label: FrameLabel) -> &'static str {
    match label {
        FrameLabel::Live => "live",
        FrameLabel::Transition(TransitionKind::HeldLastFrame) => "held_last_frame",
        FrameLabel::Black(BlackKind::NoPicture) => "black_no_picture",
        FrameLabel::Black(BlackKind::PictureExpected) => "black_picture_expected",
        FrameLabel::Black(BlackKind::CameraUnavailable) => "black_camera_unavailable",
        FrameLabel::Black(BlackKind::PictureUnavailable) => "black_picture_unavailable",
    }
}

/// Гистограмма кадров кольца, показанных не раньше `since`, по корзинам «метка + камера»; корзины — по времени первого
/// кадра (раньше — первыми), при равенстве — по имени метки и камере (порядок детерминирован).
pub fn presented_histogram(frames: &[FrameRecord], since: MonotonicNanos) -> Vec<PresentedBucket> {
    // Ключ корзины → (кадров, время первого кадра).
    let mut buckets: BTreeMap<(&'static str, Option<String>), (u64, MonotonicNanos)> = BTreeMap::new();
    for frame in frames.iter().filter(|frame| frame.present_time >= since) {
        let camera = frame.crop.as_ref().map(|crop| crop.camera.to_string());
        let entry = buckets
            .entry((label_name(frame.label), camera))
            .or_insert((0, frame.present_time));
        entry.0 += 1;
        entry.1 = entry.1.min(frame.present_time);
    }
    let mut result: Vec<PresentedBucket> = buckets
        .into_iter()
        .map(|((label, camera), (count, first))| PresentedBucket {
            label,
            camera,
            frames: count,
            first_at_ms: ms_between(since, first),
        })
        .collect();
    // Сортировка устойчивая: порядок BTreeMap (метка, камера) сохраняется при равных временах.
    result.sort_by(|left, right| left.first_at_ms.total_cmp(&right.first_at_ms));
    result
}

#[cfg(test)]
#[path = "zoom_presented_tests.rs"]
mod tests;
