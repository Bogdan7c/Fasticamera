//! Применяемая белая рендера (white-balance 01): единственное место, где решается, какая цветность идёт в ISP
//! на каждом кадре. Белую называет владелец (`WhiteDirective`); здесь — только «как к ней приехать».
//!
//! # Правила
//! * Новая `white`, отличная от текущей ЦЕЛИ, запускает переход (`Immediate` или `Over`). Указание с той же
//!   `white` (сменилась только заявка источнику) переход не трогает.
//! * Переход линеен в (майред, Duv) от ТОЧКИ, применённой сейчас (при перенацеливании на ходу — от
//!   промежуточной), и идёт по меткам сенсора. Отсчёт `Over` — от метки кадра, ПЕРВЫМ принявшего указание
//!   (прогресс этого кадра нулевой: он показывает прежнюю белую, дальше шаг по разности меток).
//! * Последний шаг — ровно цель: побитово то, что даёт `chromaticity_from_cct_duv` для цели. После перехода
//!   каждый кадр отдаёт ту же цветность без пересчёта, поэтому накопления ошибки нет.
//! * Если с момента скрытия кадров ни один кадр не показан (холодный старт прячет), переход мгновенный:
//!   плавность нужна глазу, а глаз ничего не видит. Первое указание сессии показа — тоже мгновенное.
//! * Кадр с меткой не новее прежней шага не делает.
//! * Пока указаний нет вовсе — [`WHITE_WITHOUT_DIRECTIVE`], кадры не прячутся. В продукте это недостижимо (движок публикует
//!   указание при создании); белая нужна стендам, собранным без движка белого.

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::{WhiteDirective, WhiteTransition};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, WhitePointError, chromaticity_from_cct_duv};

/// Белая стенда БЕЗ движка белого: владелец ничего не назвал, указаний нет. D65 — калибровочный свет профиля цвета.
/// В продукте недостижима: движок публикует указание сразу при создании, и применяется оно, а не эта константа.
pub const WHITE_WITHOUT_DIRECTIVE: Chromaticity = Chromaticity::D65;

/// Точка в координатах перехода: майред и Duv.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhitePoint {
    /// Обратная температура, 1e6 / К.
    pub mired: f64,
    /// Отклонение от кривой Планка.
    pub duv: f64,
}

impl WhitePoint {
    fn of_setting(setting: &WhiteBalanceSetting) -> Self {
        Self {
            mired: 1.0e6 / setting.temperature.kelvin(),
            duv: setting.tint.duv(),
        }
    }

    /// Точка как «температура + Duv».
    pub fn cct_duv(self) -> CctDuv {
        CctDuv {
            temperature: Kelvin(1.0e6 / self.mired),
            duv: Duv(self.duv),
        }
    }

    fn chromaticity(self) -> Result<Chromaticity, WhitePointError> {
        chromaticity_from_cct_duv(self.cct_duv())
    }

    fn lerp(self, other: Self, progress: f64) -> Self {
        Self {
            mired: self.mired + (other.mired - self.mired) * progress,
            duv: self.duv + (other.duv - self.duv) * progress,
        }
    }
}

/// Цветность белой настройки: тот же перевод, которым цель владельца попадает в ISP (`chromaticity_from_cct_duv`).
/// Единственная точка перевода: путь вспышки берёт отсюда белую плана, а не считает свою.
pub fn chromaticity_of_setting(setting: &WhiteBalanceSetting) -> Result<Chromaticity, WhitePointError> {
    WhitePoint::of_setting(setting).chromaticity()
}

/// Идущий переход.
#[derive(Debug, Clone, Copy)]
struct Transition {
    from: WhitePoint,
    /// Метка кадра, принявшего указание.
    started_at_nanos: i64,
    duration_nanos: i64,
}

/// Цель: что просил владелец.
#[derive(Debug, Clone, Copy)]
struct Target {
    setting: WhiteBalanceSetting,
    point: WhitePoint,
    /// Цветность цели, посчитанная один раз.
    chromaticity: Chromaticity,
}

/// Что отдаётся на кадре.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppliedFrame {
    /// Цветность для параметров ISP.
    pub white: Chromaticity,
    /// Указаний ещё не было вовсе: взята [`WHITE_WITHOUT_DIRECTIVE`] (стенд без движка белого).
    pub without_directive: bool,
}

/// Владелец применяемой белой.
#[derive(Debug, Default)]
pub struct AppliedWhite {
    target: Option<Target>,
    /// Указание, принятое, но ещё не начатое (начинается на ближайшем кадре — от его метки).
    pending: Option<(WhiteBalanceSetting, WhiteTransition)>,
    /// Применённая сейчас точка и её цветность; `None` — указаний не было.
    current: Option<(WhitePoint, Chromaticity)>,
    transition: Option<Transition>,
    last_step_nanos: Option<i64>,
    /// Показан ли хоть один кадр с момента последнего скрытия.
    visible: bool,
    /// Сколько целей не удалось перевести в цветность (температура вне таблицы).
    unconvertible_targets: u64,
}

impl AppliedWhite {
    /// Принимает указание. Та же белая, что цель, ничего не меняет.
    pub fn accept(&mut self, directive: &WhiteDirective) {
        if self.target.is_some_and(|target| target.setting == directive.white) {
            return;
        }
        let point = WhitePoint::of_setting(&directive.white);
        match point.chromaticity() {
            Ok(chromaticity) => {
                self.target = Some(Target {
                    setting: directive.white,
                    point,
                    chromaticity,
                });
                self.pending = Some((directive.white, directive.transition));
            }
            Err(_) => {
                // Прежняя белая остаётся; владелец прижимает диапазон, сюда попадать не должно.
                self.unconvertible_targets += 1;
            }
        }
    }

    /// Шаг по кадру. `will_show` — покажется ли кадр: скрытый кадр гасит «картинка видна», поэтому переход,
    /// начатый на нём, мгновенный. Видимость берётся ДО вердикта этого кадра для показываемых кадров: первый
    /// показанный кадр после скрытия получает цель мгновенно.
    pub fn advance(&mut self, sensor_timestamp_nanos: i64, will_show: bool) -> AppliedFrame {
        if !will_show {
            self.visible = false;
        }
        self.start_pending(sensor_timestamp_nanos);
        self.step(sensor_timestamp_nanos);
        if will_show {
            self.visible = true;
        }
        match self.current {
            Some((_, white)) => AppliedFrame {
                white,
                without_directive: false,
            },
            None => AppliedFrame {
                white: WHITE_WITHOUT_DIRECTIVE,
                without_directive: true,
            },
        }
    }

    fn start_pending(&mut self, now_nanos: i64) {
        let (Some((setting, transition)), Some(target)) = (self.pending.take(), self.target) else {
            return;
        };
        debug_assert!(target.setting == setting);
        let instant = match transition {
            WhiteTransition::Immediate => true,
            WhiteTransition::Over(duration) => duration.is_zero() || !self.visible,
        };
        let Some((from, _)) = self.current else {
            // Первое указание сессии: переезжать не от чего.
            self.settle(target);
            return;
        };
        match transition {
            WhiteTransition::Over(duration) if !instant => {
                let duration_nanos = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
                self.transition = Some(Transition {
                    from,
                    started_at_nanos: now_nanos,
                    duration_nanos,
                });
            }
            _ => self.settle(target),
        }
    }

    fn settle(&mut self, target: Target) {
        self.current = Some((target.point, target.chromaticity));
        self.transition = None;
    }

    fn step(&mut self, now_nanos: i64) {
        let (Some(transition), Some(target)) = (self.transition, self.target) else {
            self.last_step_nanos = Some(self.last_step_nanos.map_or(now_nanos, |last| last.max(now_nanos)));
            return;
        };
        if self.last_step_nanos.is_some_and(|last| now_nanos <= last) {
            return;
        }
        self.last_step_nanos = Some(now_nanos);
        let elapsed = now_nanos - transition.started_at_nanos;
        if elapsed >= transition.duration_nanos {
            self.settle(target);
            return;
        }
        if elapsed <= 0 {
            return;
        }
        // i64 → f64: переход — единицы секунд в наносекундах, точности f64 хватает с запасом.
        let progress = elapsed as f64 / transition.duration_nanos as f64;
        let point = transition.from.lerp(target.point, progress);
        if let Ok(chromaticity) = point.chromaticity() {
            self.current = Some((point, chromaticity));
        } else {
            // Середина отрезка между двумя допустимыми точками допустима; сюда не попасть, но молча не глотаем.
            self.unconvertible_targets += 1;
        }
    }

    /// Цель (последняя принятая белая).
    pub fn target(&self) -> Option<WhiteBalanceSetting> {
        self.target.map(|target| target.setting)
    }

    /// Применённая сейчас точка.
    pub fn applied_point(&self) -> Option<WhitePoint> {
        self.current.map(|(point, _)| point)
    }

    /// Идёт ли переход (в том числе принятый, но ещё не начатый).
    pub fn is_transitioning(&self) -> bool {
        self.transition.is_some() || self.pending.is_some()
    }

    /// Сколько целей не перевелось в цветность.
    pub fn unconvertible_targets(&self) -> u64 {
        self.unconvertible_targets
    }
}
