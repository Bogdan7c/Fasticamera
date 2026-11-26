//! Упоры ручек теплоты и оттенка в кэше параметров ISP (решения 288, 293).
//!
//! Упоры — факт для экрана калибровки, а не вход кадра: считаются один раз при смене того, от чего они зависят,
//! а не на каждый кадр. [`KnobStopsMemo`] живёт в состоянии потока (`StreamBoundState`): новый поток начинает с
//! пустой памяти, а старые упоры чужой камеры показаны быть не могут.
//!
//! # От чего зависят упоры
//! От профиля цвета потока, белой и нейтрали сцены, модели вида (постоянна на жизнь кэша) и ДВУХ ручек: теплоты и
//! оттенка. Остальные ручки вида, остаточная подтяжка AE, охват экрана и основа тона на упоры не влияют, поэтому
//! смена, например, контраста или `exposure_gain` пересчёта упоров не вызывает (ключ [`StopsKey`]).
//!
//! # Стоимость
//! Один расчёт — не более 65 вычислений сдвига белой (см. `isp_model::look::white_stops`). Кэш потока считает его
//! только после успешного пересчёта параметров с новым ключом. Побочные кэши (снимок, стенд V4) упоры тоже считают
//! (отдельный кэш — отдельная память), но один раз на снимок: на кадр видоискателя это не ложится.

use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::{CameraNeutral, DngColorProfile};
use isp_model::look::model::LookModelSettings;
use isp_model::look::white::WhiteShiftSkipReason;
use isp_model::look::white_stops::{WhiteKnobStops, white_knob_stops};
use look_contract::{LookKnob, LookSettings};

/// Итог расчёта упоров: упоры или причина, по которой сдвиг белой не применяется вовсе.
pub type KnobStopsOutcome = Result<WhiteKnobStops, WhiteShiftSkipReason>;

/// Входы, от которых зависит итог (см. модуль): при равном ключе повторный расчёт дал бы то же самое.
#[derive(Debug, Clone, Copy, PartialEq)]
struct StopsKey {
    /// Белая точка сцены (x, y), точное сравнение — как в ключе параметров кадра.
    white: [f64; 2],
    warmth: LookKnob,
    tint: LookKnob,
}

/// Память упоров одного потока: последний ключ, его итог и счётчик расчётов.
#[derive(Debug, Default)]
pub(super) struct KnobStopsMemo {
    computed: Option<(StopsKey, KnobStopsOutcome)>,
    computations: u32,
}

impl KnobStopsMemo {
    /// Пересчитывает упоры, если изменилась белая или одна из двух ручек; иначе ничего не делает.
    pub(super) fn refresh(
        &mut self,
        profile: &DngColorProfile,
        white: Chromaticity,
        scene: CameraNeutral,
        model: &LookModelSettings,
        look: &LookSettings,
    ) {
        let key = StopsKey {
            white: [white.x, white.y],
            warmth: look.warmth,
            tint: look.tint,
        };
        if self.computed.as_ref().is_some_and(|(known, _)| *known == key) {
            return;
        }
        self.computations += 1;
        self.computed = Some((key, white_knob_stops(profile, white, scene, model, look)));
    }

    /// Последний итог; `None` — упоры в этом потоке ещё ни разу не считались.
    pub(super) fn latest(&self) -> Option<&KnobStopsOutcome> {
        self.computed.as_ref().map(|(_, outcome)| outcome)
    }

    /// Сколько раз упоры считались в этом потоке (тесты следят, что неизменные входы не пересчитываются).
    #[cfg(test)]
    pub(super) fn computations(&self) -> u32 {
        self.computations
    }
}
