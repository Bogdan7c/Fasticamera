//! Запись бесшовного шага `zoom` (13b): что исполнитель измерил, кроме общих полей записи шага.
//!
//! Чистые функции и данные: «разрыв между последним кадром старой камеры и первым живым кадром новой» считается по
//! кольцу показанных кадров (шкала рендера, одна на оба кадра), период кадра — по самим кадрам старой камеры, а не
//! по константе (панель может идти на 120 Гц, камера — на 60 к/с).

use camera_contract::ids::CameraId;
use render_loop::viewfinder::{FrameLabel, FrameRecord};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;

use super::zoom_record::ms_between;

/// Сколько последних кадров старой камеры берём для периода кадра.
const PERIOD_SAMPLE_FRAMES: usize = 30;
/// Меньше стольких кадров старой камеры — период не измерен.
const MIN_FRAMES_FOR_PERIOD: usize = 3;

/// Частоты обеих камер за окно, пока тёплая готова (к/с).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FpsPair {
    /// Показываемая камера (кадры, прошедшие в раздачу).
    pub primary: f64,
    /// Тёплая камера (кадры, сразу возвращённые камере).
    pub standby: f64,
}

impl FpsPair {
    /// Меньшая из двух частот.
    pub fn minimum(self) -> f64 {
        self.primary.min(self.standby)
    }
}

/// Разрыв показа при смене камеры.
///
/// Два разных вопроса, их нельзя смешивать (урок 13b: «зазор 1,0» по любому кадру скрывал повтор старого кадра):
/// * `gap_ms` — НАСТОЯЩАЯ заморозка картинки: от последнего НОВОГО кадра старой камеры до первого нового кадра новой
///   (оба прошли через ISP в этом показе, `crop` есть). Именно он идёт в вердикт.
/// * `any_frame_gap_ms` — от последнего показанного кадра любого вида (в том числе повтора без нового кадра камеры)
///   до первого живого кадра новой: «экран что-то перерисовывал», но картинка могла стоять.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SwitchGap {
    /// От последнего живого кадра старой камеры до первого живого кадра новой, мс.
    pub gap_ms: f64,
    /// От последнего показанного кадра любого вида до первого живого кадра новой, мс.
    pub any_frame_gap_ms: f64,
    /// Период кадра старой камеры (медиана промежутков между её показанными кадрами), мс.
    pub period_ms: f64,
}

impl SwitchGap {
    /// Заморозка картинки в периодах кадра: 1 — новый кадр пришёл на следующем месте, 2 — один кадр пропущен.
    pub fn periods(self) -> f64 {
        self.gap_ms / self.period_ms
    }

    /// Сколько кадров пропущено на стыке: разрыв в периодах, округлённый до целого, минус один «положенный» период.
    ///
    /// Округляем, потому что времена показа дрожат на доли мс относительно частоты экрана: один пропущенный кадр
    /// на телефоне меряется как 2,0003 периода, и сравнение дробных периодов с порогом «2» роняло вердикт на шуме.
    /// Ровно половина периода (1,5) округляется вверх — «скорее пропуск, чем нет».
    pub fn frames_missed(self) -> u32 {
        // Число периодов неотрицательно; отрицательный или нечисловой разрыв (часы назад, нулевой период) — 0 пропусков.
        let whole_periods = self.periods().round();
        if whole_periods.is_finite() && whole_periods > 1.0 {
            // `as u32` насыщается на огромных значениях, а не переполняется.
            whole_periods as u32 - 1
        } else {
            0
        }
    }

    /// Разрыв «до любого показанного кадра» в периодах кадра (диагностика, в вердикт не идёт).
    pub fn any_frame_periods(self) -> f64 {
        self.any_frame_gap_ms / self.period_ms
    }
}

/// Что записал бесшовный шаг.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeamlessRecord {
    /// `wheel` или `pinch`.
    pub gesture: &'static str,
    /// `out`, `back` или `release`.
    pub leg: &'static str,
    /// Сколько событий `LensSwitched` пришло за шаг (бесшовная смена — ровно одно).
    pub switch_events: u32,
    /// От нажатия до готовности тёплой, мс (`None` — шаг тёплую не ждал или она не стала готова).
    pub ready_ms: Option<f64>,
    /// Частота обеих камер, пока тёплая готова.
    pub fps: Option<FpsPair>,
    /// Разрыв показа при смене камеры.
    pub gap: Option<SwitchGap>,
    /// Коэффициент отметки объектива, на который ведёт жест, тысячные (`None` — у шага нет цели).
    pub target_ratio_milli: Option<u32>,
    /// Что показывал экран в тысячных, когда пришло событие смены объектива (`None` — смены не было).
    pub ratio_at_switch_milli: Option<u32>,
    /// От подъёма пальцев до «колесо ушло, тёплых нет», мс (только у шага `release`).
    pub closed_ms: Option<f64>,
}

/// Времена показа живых кадров камеры `camera` в порядке кольца (старые первыми).
fn live_times(frames: &[FrameRecord], camera: &CameraId) -> Vec<MonotonicNanos> {
    frames
        .iter()
        .filter(|frame| frame.label == FrameLabel::Live)
        .filter(|frame| frame.crop.as_ref().is_some_and(|crop| &crop.camera == camera))
        .map(|frame| frame.present_time)
        .collect()
}

/// Разрыв показа при смене: между последним живым кадром старой камеры и первым живым кадром новой, показанным не
/// раньше `since` (плюс диагностика «до любого показанного кадра»).
/// `None` — кадров не хватило (кольцо вытеснило записи или новой камеры ещё нет).
pub fn switch_gap(frames: &[FrameRecord], old: &CameraId, new: &CameraId, since: MonotonicNanos) -> Option<SwitchGap> {
    let first_new = live_times(frames, new).into_iter().find(|time| *time >= since)?;
    let old_times: Vec<MonotonicNanos> = live_times(frames, old)
        .into_iter()
        .filter(|time| *time < first_new)
        .collect();
    // Настоящая заморозка — до последнего живого кадра старой камеры; без него смену мерить нечем.
    let last_old_live = *old_times.last()?;
    // Для диагностики: ближайший показанный кадр ЛЮБОГО вида (пересборка/повтор без нового кадра камеры).
    let last_shown = frames
        .iter()
        .map(|frame| frame.present_time)
        .filter(|time| *time < first_new)
        .max()?;
    let recent = &old_times[old_times.len().saturating_sub(PERIOD_SAMPLE_FRAMES)..];
    if recent.len() < MIN_FRAMES_FOR_PERIOD {
        return None;
    }
    let mut deltas: Vec<f64> = recent.windows(2).map(|pair| ms_between(pair[0], pair[1])).collect();
    deltas.sort_by(f64::total_cmp);
    let period_ms = deltas[deltas.len() / 2];
    (period_ms > 0.0).then(|| SwitchGap {
        gap_ms: ms_between(last_old_live, first_new),
        any_frame_gap_ms: ms_between(last_shown, first_new),
        period_ms,
    })
}

#[cfg(test)]
#[path = "zoom_seamless_record_tests.rs"]
mod tests;
