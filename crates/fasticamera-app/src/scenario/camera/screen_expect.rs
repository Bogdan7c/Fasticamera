//! Ожидания одного шага сценария `screen`: что должно измениться на экране и в камере после жеста.
//!
//! Ожидание — данные ([`Expectation`]), а проверка — чистые функции над приращениями ([`ProbeDelta`]): так каждое
//! правило проверяется тестом без маршрутизатора. Счётчики сравниваются точно ([`Count::Exactly`]) или с запасом
//! ([`Count::AtLeast`]); «ничего лишнего» — это `Exactly(0)`: например, тап при открытой панели обязан только
//! закрыть её, а не отправить камере намерение фокуса.

use camera_contract::three_a::AeAfLock;

use super::screen_probe::ProbeDelta;
use super::screen_sensor_point::{FramePoint, PointCheck, check_point};
use crate::camera_screen_source::{ModeKind, PanelKind, Presence, ShutterPlacement};

/// Сколько раз счётчик должен вырасти за шаг.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// Ровно столько (0 — «ничего не случилось»).
    Exactly(u64),
    /// Не меньше (жест вроде перетаскивания шкалы EV шлёт камере несколько значений).
    AtLeast(u64),
}

impl Count {
    /// Нужное минимальное число событий, без которых шаг ещё не выполнен.
    fn required(self) -> u64 {
        match self {
            Count::Exactly(count) | Count::AtLeast(count) => count,
        }
    }

    /// Описание для текста нарушения.
    fn describe(self) -> String {
        match self {
            Count::Exactly(count) => format!("ровно {count}"),
            Count::AtLeast(count) => format!("не меньше {count}"),
        }
    }

    fn accepts(self, observed: u64) -> bool {
        match self {
            Count::Exactly(count) => observed == count,
            Count::AtLeast(count) => observed >= count,
        }
    }
}

/// Какой счётчик приращения проверяется.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Распознанные экраном тапы по видоискателю и капсуле.
    Taps,
    /// Распознанные долгие нажатия.
    LongPresses,
    /// Начатые панорамы (свайпы).
    Pans,
    /// Нажатия круглых кнопок экрана (значки, кнопки панели Exposure).
    ButtonPresses,
    /// Намерения камере, принятые очередью движка.
    CameraIntents,
    /// Триггеры фокуса, дошедшие до HAL (счётчик движка).
    FocusTriggers,
    /// Запросы снимка, принятые фейк-сервисом.
    CaptureRequests,
}

impl Metric {
    pub fn name(self) -> &'static str {
        match self {
            Metric::Taps => "тапы",
            Metric::LongPresses => "долгие нажатия",
            Metric::Pans => "свайпы",
            Metric::ButtonPresses => "нажатия кнопок",
            Metric::CameraIntents => "намерения камере",
            Metric::FocusTriggers => "триггеры до HAL",
            Metric::CaptureRequests => "запросы снимка",
        }
    }

    pub fn read(self, delta: &ProbeDelta) -> u64 {
        match self {
            Metric::Taps => delta.taps,
            Metric::LongPresses => delta.long_presses,
            Metric::Pans => delta.pans,
            Metric::ButtonPresses => delta.button_presses,
            Metric::CameraIntents => delta.camera_intents,
            Metric::FocusTriggers => delta.focus_triggers,
            Metric::CaptureRequests => delta.capture_requests,
        }
    }
}

/// Проверка текущего значения (не приращения).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StateCheck {
    /// Точка интереса движка стоит там, куда указывал тап: `frame` — доли кадра на экране, `tolerance` — допуск по
    /// каждой оси в долях сенсора. Ожидаемая точка выводится независимо от кода экрана (`screen_sensor_point`).
    SensorPoint { frame: FramePoint, tolerance: f32 },
    /// Блокировка AE/AF в движке именно такая.
    Lock(AeAfLock),
    /// Компенсация экспозиции в движке равна нулю.
    ExposureBiasIsZero,
    /// Компенсация экспозиции в движке не равна нулю.
    ExposureBiasIsSet,
    /// Миниатюра показывает последний снимок.
    ThumbnailShowsLast,
    // ---- прямые проверки состояния экрана (снимок `ScreenStateSnapshot`) ----
    /// Открыта именно эта панель.
    Panel(PanelKind),
    /// Затвор виден / ушёл под шторку.
    Shutter(ShutterPlacement),
    /// Плашка «AE/AF LOCK» показана / скрыта.
    LockPlaque(Presence),
    /// Шкала EV на экране в нуле.
    ScreenExposureStepsAreZero,
    /// Шкала EV на экране не в нуле.
    ScreenExposureStepsAreSet,
    /// Текущий режим съёмки.
    Mode(ModeKind),
}

impl StateCheck {
    fn holds(self, delta: &ProbeDelta) -> bool {
        match self {
            StateCheck::SensorPoint { frame, tolerance } => point_check(delta, frame, tolerance) == PointCheck::Matches,
            StateCheck::Lock(lock) => delta.lock == lock,
            StateCheck::ExposureBiasIsZero => delta.exposure_bias_ev == 0.0,
            StateCheck::ExposureBiasIsSet => delta.exposure_bias_ev != 0.0,
            StateCheck::ThumbnailShowsLast => delta.thumbnail_has_last,
            StateCheck::Panel(panel) => delta.screen.panel == panel,
            StateCheck::Shutter(placement) => delta.screen.shutter_placement == placement,
            StateCheck::LockPlaque(presence) => delta.screen.lock_plaque == presence,
            StateCheck::ScreenExposureStepsAreZero => delta.screen.exposure_steps == 0,
            StateCheck::ScreenExposureStepsAreSet => delta.screen.exposure_steps != 0,
            StateCheck::Mode(mode) => delta.screen.mode == mode,
        }
    }

    fn describe(self, delta: &ProbeDelta) -> String {
        match self {
            StateCheck::SensorPoint { frame, tolerance } => describe_point(point_check(delta, frame, tolerance), frame),
            StateCheck::Lock(lock) => format!("блокировка {lock:?}, увидели {:?}", delta.lock),
            StateCheck::ExposureBiasIsZero => format!("EV = 0, увидели {:+.2}", delta.exposure_bias_ev),
            StateCheck::ExposureBiasIsSet => "EV не равен 0, увидели 0".to_owned(),
            StateCheck::ThumbnailShowsLast => "миниатюра показывает последний снимок, увидели пустую".to_owned(),
            StateCheck::Panel(panel) => format!("панель (panel) {panel:?}, увидели {:?}", delta.screen.panel),
            StateCheck::Shutter(placement) => format!(
                "затвор (shutter_placement) {placement:?}, увидели {:?}",
                delta.screen.shutter_placement
            ),
            StateCheck::LockPlaque(presence) => format!(
                "плашка AE/AF LOCK (lock_plaque) {presence:?}, увидели {:?}",
                delta.screen.lock_plaque
            ),
            StateCheck::ScreenExposureStepsAreZero => format!(
                "шаги EV на экране (exposure_steps) = 0, увидели {}",
                delta.screen.exposure_steps
            ),
            StateCheck::ScreenExposureStepsAreSet => {
                "шаги EV на экране (exposure_steps) не равны 0, увидели 0".to_owned()
            }
            StateCheck::Mode(mode) => format!("режим (mode) {mode:?}, увидели {:?}", delta.screen.mode),
        }
    }
}

fn point_check(delta: &ProbeDelta, frame: FramePoint, tolerance: f32) -> PointCheck {
    check_point(delta.point_of_interest, delta.sensor_orientation, frame, tolerance)
}

/// Текст нарушения про точку: ожидали одно место сенсора, увидели другое (или точки нет).
fn describe_point(check: PointCheck, frame: FramePoint) -> String {
    let tap = format!("тап в кадре ({:.2}; {:.2})", frame.u, frame.v);
    match check {
        PointCheck::Matches => format!("{tap}: точка интереса сенсора верна"),
        PointCheck::NoPoint => format!("{tap}: точка интереса движка (three_a.point) должна появиться, её нет"),
        PointCheck::UnknownOrientation => format!("{tap}: ориентация сенсора неизвестна, точку проверить нечем"),
        PointCheck::Off { expected, actual } => format!(
            "{tap}: точка интереса сенсора (three_a.point) ({:.2}; {:.2}), увидели ({:.2}; {:.2}) — оси, зеркало или ориентация сенсора перепутаны",
            expected.x, expected.y, actual.x, actual.y
        ),
    }
}

/// Что обязано произойти за шаг.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Expectation {
    counts: Vec<(Metric, Count)>,
    states: Vec<StateCheck>,
}

impl Expectation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Добавляет условие на приращение счётчика.
    pub fn count(mut self, metric: Metric, count: Count) -> Self {
        self.counts.push((metric, count));
        self
    }

    /// Добавляет условие на текущее значение.
    pub fn state(mut self, check: StateCheck) -> Self {
        self.states.push(check);
        self
    }

    /// Условия на счётчики (для таблицы шагов в тестах).
    pub fn counts(&self) -> &[(Metric, Count)] {
        &self.counts
    }

    /// Условия на текущие значения (для тестов таблицы шагов).
    #[cfg(test)]
    pub fn states(&self) -> &[StateCheck] {
        &self.states
    }

    /// Всё нужное уже случилось: каждый счётчик дорос до требуемого числа, состояния верны. «Лишнего не было»
    /// отсюда не следует — это проверяет [`Expectation::violations`] после выдержки.
    pub fn is_reached(&self, delta: &ProbeDelta) -> bool {
        self.counts
            .iter()
            .all(|(metric, count)| metric.read(delta) >= count.required())
            && self.states.iter().all(|check| check.holds(delta))
    }

    /// Счётчик уже перерос точное число: ждать дальше нечего, шаг провален (лишнее намерение камере, второй тап).
    pub fn is_overshot(&self, delta: &ProbeDelta) -> bool {
        self.counts
            .iter()
            .any(|(metric, count)| matches!(count, Count::Exactly(exact) if metric.read(delta) > *exact))
    }

    /// Нарушения «ожидали X, увидели Y» (пусто — всё верно).
    pub fn violations(&self, delta: &ProbeDelta) -> Vec<String> {
        let mut violations: Vec<String> = self
            .counts
            .iter()
            .filter(|(metric, count)| !count.accepts(metric.read(delta)))
            .map(|(metric, count)| {
                format!(
                    "{}: ожидали {}, увидели {}",
                    metric.name(),
                    count.describe(),
                    metric.read(delta)
                )
            })
            .collect();
        violations.extend(
            self.states
                .iter()
                .filter(|check| !check.holds(delta))
                .map(|check| format!("ожидали {}", check.describe(delta))),
        );
        violations
    }
}

#[cfg(test)]
#[path = "screen_expect_tests.rs"]
mod tests;
