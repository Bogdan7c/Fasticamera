//! Запись одного шага сценария `zoom` и чистые разборы наблюдений (сессия 13): «первый живой кадр новой камеры»,
//! «зеркало кадров камеры», «вибрации окна шага», «не потерял ли кольцо кадров рендера нужные записи».
//!
//! Всё здесь — функции от данных, без часов и ввода-вывода: тесты подают подставные кадры и счётчики и сверяют результат.

use camera_contract::ids::{CameraId, LensFacing};
use camera_contract::zoom::PreviewMirror;
use render_loop::viewfinder::{FrameLabel, FrameRecord};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;

use super::zoom_reveal::RevealFaults;

/// Наносекунд в миллисекунде.
const NANOS_PER_MS: f64 = 1_000_000.0;

/// Миллисекунды между двумя метками одной шкалы; назад часы не идут (меньше — 0).
pub fn ms_between(from: MonotonicNanos, to: MonotonicNanos) -> f64 {
    (to.as_nanos().saturating_sub(from.as_nanos())).max(0) as f64 / NANOS_PER_MS
}

/// Сколько миллисекунд от действия до первого ЖИВОГО кадра камеры `camera`, показанного не раньше действия.
/// `None` — такого кадра в кольце нет (ещё не пришёл или кольцо его уже вытеснило — это различает [`RingWatch`]).
pub fn first_live_frame_ms(frames: &[FrameRecord], camera: &CameraId, action_at: MonotonicNanos) -> Option<f64> {
    frames
        .iter()
        .filter(|frame| frame.label == FrameLabel::Live && frame.present_time >= action_at)
        .filter(|frame| frame.crop.as_ref().is_some_and(|crop| &crop.camera == camera))
        .map(|frame| frame.present_time)
        .min()
        .map(|time| ms_between(action_at, time))
}

/// Что показало кадрирование кадров камеры о зеркале.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorCheck {
    /// Шаг направление не менял и кадров новой камеры не ждал: проверять нечего.
    NotApplicable,
    /// У всех живых кадров камеры зеркало как у iOS: фронтальная — по горизонтали, задние — без зеркала.
    Correct,
    /// Хотя бы у одного кадра зеркало не то (сколько кадров ошиблись).
    Wrong { frames: usize },
    /// Живых кадров этой камеры в кольце не нашлось: зеркало не измерено.
    NoFrameObserved,
}

/// Проверка зеркала у живых кадров камеры `camera`, показанных после `since`.
pub fn mirror_check(
    frames: &[FrameRecord],
    camera: &CameraId,
    since: MonotonicNanos,
    facing: LensFacing,
) -> MirrorCheck {
    let expected = match facing {
        LensFacing::Front => PreviewMirror::Horizontal,
        LensFacing::Back | LensFacing::External => PreviewMirror::None,
    };
    let own_frames: Vec<&FrameRecord> = frames
        .iter()
        .filter(|frame| frame.label == FrameLabel::Live && frame.present_time >= since)
        .filter(|frame| frame.crop.as_ref().is_some_and(|crop| &crop.camera == camera))
        .collect();
    if own_frames.is_empty() {
        return MirrorCheck::NoFrameObserved;
    }
    let wrong = own_frames
        .iter()
        .filter(|frame| frame.crop.as_ref().is_some_and(|crop| crop.framed.mirror != expected))
        .count();
    if wrong == 0 {
        MirrorCheck::Correct
    } else {
        MirrorCheck::Wrong { frames: wrong }
    }
}

/// Следит, не вытеснило ли кольцо кадров рендера записи, которые мы не успели прочитать.
///
/// Кольцо конечно (`viewfinder.continuity.recent_frames`): если за один тик показано больше кадров, чем оно держит,
/// старейшая запись нового снимка новее последней виденной, и между ними дыра. Тогда «первого живого кадра нет» —
/// не «его не было», а «не измерено», и отчёт обязан это сказать.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RingWatch {
    last_seen: Option<MonotonicNanos>,
    overran: bool,
}

impl RingWatch {
    /// Учесть свежий снимок кольца (старые кадры первыми).
    pub fn absorb(&mut self, frames: &[FrameRecord]) {
        let (Some(oldest), Some(newest)) = (frames.first(), frames.last()) else {
            return;
        };
        if let Some(last_seen) = self.last_seen
            && oldest.present_time > last_seen
        {
            self.overran = true;
        }
        self.last_seen = Some(newest.present_time);
    }

    /// Была ли дыра с прошлого [`RingWatch::reset_overrun`].
    pub fn overran(&self) -> bool {
        self.overran
    }

    /// Начать шаг с чистой пометкой (последний виденный кадр остаётся: дыру между шагами видно по нему же).
    pub fn reset_overrun(&mut self) {
        self.overran = false;
    }
}

/// Вибрации в окне шага.
///
/// Промежутки между вибрациями здесь НЕ меряются: ограничитель частоты решает по времени касаний, а исполнитель
/// штампует вибрации временем кадра UI — на разных часах «минимальный промежуток» дал бы ложный провал (два образца в
/// одном кадре → 0 мс) или ложный проход. Ограничитель в 30 мс доказывают юнит-тесты `ui-kit` на времени касаний;
/// сценарий судит только число вибраций: не меньше порога и не больше, чем ограничитель вообще мог пропустить.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HapticWindow {
    /// Сколько вибраций принято в окне.
    pub count: u64,
    /// Длина окна на шкале рендера, мс: от действия до конца наблюдения (верхняя граница числа тиков).
    pub window_ms: f64,
}

/// Вибрации окна: принятых к `total_now` больше, чем к началу окна (`total_at_start`); окно — `since..until`.
pub fn haptic_window(
    total_now: u64,
    total_at_start: u64,
    since: MonotonicNanos,
    until: MonotonicNanos,
) -> HapticWindow {
    HapticWindow {
        count: total_now.saturating_sub(total_at_start),
        window_ms: ms_between(since, until),
    }
}

/// Итог проверки зеркала и частоты вынесен в запись шага; остальное — измерения.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ZoomStepRecord {
    /// Имя шага из плана (`01_tap_0.6`).
    pub name: String,
    /// Что ждали от объектива: `no_switch`, `to_ultra_wide`, `to_any_back` …
    pub expected: String,
    /// Смена объектива, которую сообщил движок.
    pub switch: Option<SwitchTiming>,
    /// Действие → первый живой кадр новой камеры на экране, мс (шкала рендера).
    pub first_live_frame_ms: Option<f64>,
    /// Камера, чей кадр показан первым.
    pub new_camera: Option<String>,
    /// Кадров показано удержанными за шаг.
    pub held_frames: u64,
    /// Чёрных кадров там, где ждали картинку, за шаг. Должно быть 0.
    pub black_frames_while_picture_expected: u64,
    /// Плохие исходы держания переходного кадра за шаг (раскрытие по сроку, честное чёрное). Должны быть нулевыми.
    pub reveal_faults: RevealFaults,
    /// Промежуточных целей смены, отменённых новой целью.
    pub retargets: u32,
    /// Намерений объектива, которые экран положил в ящик за шаг.
    pub lens_intents_published: u64,
    /// Измеренная частота потока по меткам сенсора, к/с.
    pub measured_fps: Option<f64>,
    /// Ожидаемая частота камеры, к/с.
    pub expected_fps: Option<u32>,
    pub mirror: MirrorCheck,
    /// Вибрации окна шага (только у протяжки по колесу).
    pub haptics: Option<HapticWindow>,
    /// Коэффициент, который экран показывает в конце шага.
    pub screen_ratio: f64,
    /// Кольцо кадров рендера теряло записи во время шага: «нет кадра» значит «не измерено».
    pub ring_overran: bool,
    /// Причина провала шага (`None` — шаг дошёл до конца).
    pub failure: Option<String>,
    /// Заметки без провала (деградация частоты, потолок частоты камеры).
    pub notes: Vec<String>,
}

impl ZoomStepRecord {
    /// Запись шага, который провалился до начала (или стал провалом прогона вне шагов): измерений нет.
    pub fn not_started(name: &str, reason: &str) -> Self {
        Self {
            name: name.to_owned(),
            expected: "not_started".to_owned(),
            switch: None,
            first_live_frame_ms: None,
            new_camera: None,
            held_frames: 0,
            black_frames_while_picture_expected: 0,
            reveal_faults: RevealFaults::default(),
            retargets: 0,
            lens_intents_published: 0,
            measured_fps: None,
            expected_fps: None,
            mirror: MirrorCheck::NotApplicable,
            haptics: None,
            screen_ratio: 0.0,
            ring_overran: false,
            failure: Some(reason.to_owned()),
            notes: Vec::new(),
        }
    }
}

/// Смена объектива глазами движка и сценария.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SwitchTiming {
    pub from: &'static str,
    pub to: &'static str,
    /// Время, которое насчитал сам движок (от намерения до первого кадра новой камеры), мс.
    pub engine_elapsed_ms: u64,
    /// Время от действия сценария до того, как сценарий увидел событие, мс (точность — тик маршрутизатора).
    pub observed_ms: u64,
}

#[cfg(test)]
#[path = "zoom_record_tests.rs"]
mod tests;
