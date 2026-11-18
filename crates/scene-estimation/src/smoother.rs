//! Скалярное сглаживание белой точки по времени кадров сенсора.
//!
//! Состояние — `(майред, Duv)`: линейно по ним белая движется равномерно «для глаза» (майреды) и не
//! выходит из полосы вокруг кривой. Приближение экспоненциальное: `x ← x + (цель − x)·(1 − e^(−dt/τ))`
//! — монотонное, без перелёта, зависит от `dt` аналитически (одинаково при 30 и 60 к/с).
//!
//! # Отчёт о смене
//! ISP пересчитывает параметры (`IspParams::resolve`) при ЛЮБОМ изменении белой, поэтому белая
//! сообщается заново, только когда ушла от **последней сообщённой** больше порога (майред или Duv).
//! Порог считается от сообщённого значения, а не от прошлого шага: иначе медленный профиль «Видео»
//! (шаг за кадр меньше порога) не сообщил бы ничего никогда.

/// Точка белого как `(майред, Duv)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteState {
    /// Обратная температура, 10⁶/К.
    pub mired: f64,
    /// Расстояние до кривой Планка (CIE 1960 uv), плюс — к зелёному.
    pub duv: f64,
}

/// Пороги отчёта о смене.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReportThresholds {
    /// Сдвиг по майредам, при котором белая сообщается заново.
    pub mired: f64,
    /// Сдвиг по Duv, при котором белая сообщается заново.
    pub duv: f64,
}

/// Что сообщить после шага.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Report {
    /// Состояния ещё нет (белая не определена).
    NoWhiteYet,
    /// Сообщённая белая прежняя: ISP пересчитывать не надо.
    Unchanged(WhiteState),
    /// Белая сдвинулась заметно: новое значение (теперь оно и есть «сообщённое»).
    Changed(WhiteState),
}

/// Сглаживатель белой точки.
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteSmoother {
    current: Option<WhiteState>,
    reported: Option<WhiteState>,
    thresholds: ReportThresholds,
}

impl WhiteSmoother {
    /// Пустой сглаживатель с порогами отчёта.
    pub fn new(thresholds: ReportThresholds) -> Self {
        Self {
            current: None,
            reported: None,
            thresholds,
        }
    }

    /// Текущее (внутреннее) состояние.
    pub fn current(&self) -> Option<WhiteState> {
        self.current
    }

    /// Последнее сообщённое ISP значение.
    pub fn reported(&self) -> Option<WhiteState> {
        self.reported
    }

    /// Прыжок в цель без сглаживания (белой ещё нет, ручная точка на свежем потоке).
    pub fn snap_to(&mut self, target: WhiteState) {
        self.current = Some(target);
    }

    /// Шаг к цели за `dt_seconds` при постоянной времени `time_constant_seconds`.
    /// Белой ещё нет — прыжок в цель. `dt = 0` ничего не двигает.
    pub fn approach(&mut self, target: WhiteState, time_constant_seconds: f64, dt_seconds: f64) {
        let Some(current) = self.current else {
            self.current = Some(target);
            return;
        };
        let fraction = 1.0 - (-dt_seconds / time_constant_seconds).exp();
        self.current = Some(WhiteState {
            mired: current.mired + (target.mired - current.mired) * fraction,
            duv: current.duv + (target.duv - current.duv) * fraction,
        });
    }

    /// Сверяет текущее состояние с сообщённым и решает, что сообщить.
    pub fn report(&mut self) -> Report {
        let Some(current) = self.current else {
            return Report::NoWhiteYet;
        };
        let moved = self.reported.is_none_or(|reported| {
            (current.mired - reported.mired).abs() > self.thresholds.mired
                || (current.duv - reported.duv).abs() > self.thresholds.duv
        });
        if moved {
            self.reported = Some(current);
            Report::Changed(current)
        } else {
            Report::Unchanged(self.reported.unwrap_or(current))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLDS: ReportThresholds = ReportThresholds {
        mired: 0.5,
        duv: 0.0002,
    };

    fn state(mired: f64, duv: f64) -> WhiteState {
        WhiteState { mired, duv }
    }

    #[test]
    fn first_approach_snaps_and_first_report_is_a_change() {
        let mut smoother = WhiteSmoother::new(THRESHOLDS);
        assert_eq!(smoother.report(), Report::NoWhiteYet);
        smoother.approach(state(154.0, 0.0), 1.5, 0.016);
        assert_eq!(
            smoother.current(),
            Some(state(154.0, 0.0)),
            "без белой цель принимается сразу"
        );
        assert_eq!(smoother.report(), Report::Changed(state(154.0, 0.0)));
        assert_eq!(smoother.report(), Report::Unchanged(state(154.0, 0.0)));
    }

    #[test]
    fn approach_is_exponential_and_independent_of_the_step_size() {
        let (target, tau) = (state(357.0, 0.01), 0.25);
        let mut coarse = WhiteSmoother::new(THRESHOLDS);
        coarse.snap_to(state(154.0, 0.0));
        coarse.approach(target, tau, 0.5);
        let mut fine = WhiteSmoother::new(THRESHOLDS);
        fine.snap_to(state(154.0, 0.0));
        for _ in 0..30 {
            fine.approach(target, tau, 0.5 / 30.0);
        }
        let (coarse, fine) = (coarse.current().expect("состояние"), fine.current().expect("состояние"));
        assert!((coarse.mired - fine.mired).abs() < 1e-9, "{coarse:?} против {fine:?}");
        let expected = 154.0 + (357.0 - 154.0) * (1.0 - (-0.5_f64 / tau).exp());
        assert!((coarse.mired - expected).abs() < 1e-9);
    }

    #[test]
    fn zero_step_does_not_move_and_never_overshoots() {
        let mut smoother = WhiteSmoother::new(THRESHOLDS);
        smoother.snap_to(state(154.0, 0.0));
        smoother.approach(state(357.0, 0.0), 0.25, 0.0);
        assert_eq!(smoother.current(), Some(state(154.0, 0.0)));
        for _ in 0..1000 {
            smoother.approach(state(357.0, 0.0), 0.25, 0.1);
            assert!(smoother.current().expect("состояние").mired <= 357.0);
        }
    }

    #[test]
    fn report_compares_against_the_last_reported_value_so_slow_motion_still_reports() {
        let mut smoother = WhiteSmoother::new(THRESHOLDS);
        smoother.snap_to(state(154.0, 0.0));
        assert!(matches!(smoother.report(), Report::Changed(_)));
        // Шаги по 0,2 майред (меньше порога 0,5): сообщение должно появиться на третьем шаге, а не никогда.
        let mut reported_at = None;
        for step in 1..=5 {
            smoother.snap_to(state(154.0 + 0.2 * f64::from(step), 0.0));
            if matches!(smoother.report(), Report::Changed(_)) {
                reported_at = Some(step);
                break;
            }
        }
        assert_eq!(reported_at, Some(3), "0,6 майред от сообщённого > порога 0,5");
    }

    #[test]
    fn duv_shift_alone_triggers_a_report() {
        let mut smoother = WhiteSmoother::new(THRESHOLDS);
        smoother.snap_to(state(154.0, 0.0));
        let _ = smoother.report();
        smoother.snap_to(state(154.0, 0.0001));
        assert!(matches!(smoother.report(), Report::Unchanged(_)));
        smoother.snap_to(state(154.0, 0.0003));
        assert!(matches!(smoother.report(), Report::Changed(_)));
    }
}
