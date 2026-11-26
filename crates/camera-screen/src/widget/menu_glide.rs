//! Переезд кнопки меню PHOTO ↔ CALIBRATE (решения 282, 295): при смене режима рисуется ОДНА кнопка, которая едет по
//! прямой на экране от своего места в прежнем режиме к месту в новом.
//!
//! # Почему так
//! У кнопки два дома — нижний ряд PHOTO и нижний ряд раздела (там она стоит на линии капсулы и в повёрнутом корне).
//! Это два разных виджета в двух деревьях сцены, которые переключаются за один кадр. Чтобы кнопка не «телепортировалась»,
//! пока режим меняется, `Stage` на время переезда рисует третью, ездящую копию, а статические копии не рисует (двойной
//! кнопки нет). Материал кнопки меняется ОДНИМ кадром в начале переезда (решения 269, 295): ездящая копия сразу
//! рисуется материалом режима назначения.
//!
//! # Данные
//! Состояние переезда — переходное состояние сцены, оно живёт в `Stage` (как завеса перехода), а не в `ScreenState`:
//! редьюсер про него ничего не знает. Концы переезда — центр кнопки на экране и угол значка; конец-назначение каждый
//! кадр берётся у дома заново (раздел может ещё поворачиваться вместе с телефоном), поэтому по окончании кнопка
//! садится в якорь раскладки без шва со статической копией.
//!
//! # Пружина и Reduce Motion
//! Прогресс 0 → 1 едет пружиной токена `CardMorph` (морф ≈ 0,3 с без перелёта: центр кнопки движется монотонно, «прямо к
//! цели», а кнопка не остаётся неотзывчивой дольше нужного — пока она едет, касаний она не ловит).
//! Reduce Motion — переезда нет вовсе (прыжок), как у остальных движений экрана.

use ui_core::geometry::Point;
use ui_core::spring::{RestCriterion, SpringAnimation, SpringPhase, SpringStart};
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_draw::transform::Affine2;
use ui_kit::tokens::MotionToken;

use super::Ctx;
use super::error::{WidgetError, element};

/// Критерий покоя прогресса 0..1: переезд кончается, когда кнопка в пределах 0,5 % пути от якоря (≈ 1 pt на пути в
/// 250 pt) и почти не движется. Хвост пружины глубже незаметен глазу, а кнопка в нём не нажималась бы лишние
/// десятки миллисекунд; статическая копия садится в якорь на этот долю точки.
const PROGRESS_REST: RestCriterion = RestCriterion {
    relative_epsilon: 0.005,
    absolute_position_floor: 0.005,
    velocity_tolerance_factor: 1.0,
};

/// Куда идёт переезд.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlideDirection {
    /// PHOTO → раздел калибровки: материал поверхности раздела.
    PhotoToSection,
    /// Раздел калибровки → PHOTO: материал стекла PHOTO.
    SectionToPhoto,
}

/// Конец переезда: где кнопка стоит на экране и как повёрнут её значок.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GlideEndpoint {
    /// Центр кнопки, pt поверхности.
    pub centre_pt: Point,
    /// Поворот значка на экране, радианы.
    pub icon_angle_rad: f64,
}

/// Где и как нарисовать ездящую кнопку в этот кадр.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GlideFrame {
    pub direction: GlideDirection,
    pub endpoint: GlideEndpoint,
}

impl GlideFrame {
    /// Преобразование значка: поворот вокруг центра кнопки, затем сдвиг в центр (пиксели экрана).
    pub(crate) fn icon_transform(&self, ctx: &Ctx<'_>) -> Affine2 {
        Affine2::rotate(self.endpoint.icon_angle_rad as f32).then(self.button_transform(ctx))
    }

    /// Преобразование кнопки: круг не зависит от поворота, только сдвиг центра (пиксели экрана).
    pub(crate) fn button_transform(&self, ctx: &Ctx<'_>) -> Affine2 {
        Affine2::translate(
            ctx.px.px(self.endpoint.centre_pt.x),
            ctx.px.px(self.endpoint.centre_pt.y),
        )
    }
}

/// Идущий переезд.
#[derive(Debug, Clone, Copy)]
struct GlideRun {
    direction: GlideDirection,
    from: GlideEndpoint,
    progress: SpringAnimation,
    /// Где кнопку нарисовали в последний раз: если режим снова сменили на полпути, новый переезд стартует отсюда.
    last_drawn: GlideEndpoint,
}

/// Переезд кнопки меню: пусто или один идущий.
#[derive(Debug, Default)]
pub(crate) struct MenuButtonGlide {
    run: Option<GlideRun>,
}

impl MenuButtonGlide {
    /// Начать переезд из `from`. Reduce Motion — ничего не начинает (кнопка прыгает). Если прежний переезд ещё идёт,
    /// новый стартует с места, где кнопку нарисовали последний раз (скачка нет).
    pub(crate) fn start(
        &mut self,
        ctx: &Ctx<'_>,
        direction: GlideDirection,
        from: GlideEndpoint,
    ) -> Result<(), WidgetError> {
        if ctx.motion == MotionPreference::Reduced {
            self.run = None;
            return Ok(());
        }
        // `last_drawn` годится только пока прежний переезд ещё идёт. Завершённый (но не очищенный — `frame` мог не
        // вызываться) переезд хранит устаревшее место: кнопка с тех пор могла уехать вместе с поворотом раздела.
        let start_point = match self.run {
            Some(run) if run.progress.sample(ctx.now).phase == SpringPhase::Moving => run.last_drawn,
            _ => from,
        };
        let spring = ctx.spring(MotionToken::CardMorph)?;
        let progress = SpringAnimation::start(SpringStart {
            spring,
            from: 0.0,
            to: 1.0,
            initial_velocity: 0.0,
            start_time: ctx.now,
            rest: PROGRESS_REST,
        })
        .map_err(element("пружина переезда кнопки меню"))?;
        self.run = Some(GlideRun {
            direction,
            from: start_point,
            progress,
            last_drawn: start_point,
        });
        Ok(())
    }

    /// Едет ли кнопка в момент `now`: статические копии при этом не рисуются и не нажимаются.
    pub(crate) fn is_active(&self, now: MonotonicNanos) -> bool {
        self.run
            .is_some_and(|run| run.progress.sample(now).phase == SpringPhase::Moving)
    }

    /// Направление идущего переезда (чтобы Stage взял конец-назначение у нужного дома).
    pub(crate) fn direction(&self, now: MonotonicNanos) -> Option<GlideDirection> {
        self.is_active(now).then(|| self.run.map(|run| run.direction))?
    }

    /// Где рисовать кнопку в момент `now`, если конец-назначение сейчас в `target`. `None` — переезд не идёт
    /// (закончился, и статическая копия стоит в том же месте).
    pub(crate) fn frame(&mut self, now: MonotonicNanos, target: GlideEndpoint) -> Option<GlideFrame> {
        let run = self.run.as_mut()?;
        let sample = run.progress.sample(now);
        if sample.phase == SpringPhase::AtRest {
            self.run = None;
            return None;
        }
        let progress = sample.value;
        let endpoint = GlideEndpoint {
            centre_pt: Point::new(
                lerp(run.from.centre_pt.x, target.centre_pt.x, progress),
                lerp(run.from.centre_pt.y, target.centre_pt.y, progress),
            ),
            icon_angle_rad: run.from.icon_angle_rad
                + shortest_angle_between(run.from.icon_angle_rad, target.icon_angle_rad) * progress,
        };
        run.last_drawn = endpoint;
        Some(GlideFrame {
            direction: run.direction,
            endpoint,
        })
    }
}

fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    from + (to - from) * progress
}

/// Знаковая разность углов `to − from`, приведённая в (−π, π]: значок поворачивается кратчайшим путём.
fn shortest_angle_between(from: f64, to: f64) -> f64 {
    let full_turn = std::f64::consts::TAU;
    let mut difference = (to - from) % full_turn;
    if difference > std::f64::consts::PI {
        difference -= full_turn;
    } else if difference <= -std::f64::consts::PI {
        difference += full_turn;
    }
    difference
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_turns_by_the_short_way_round() {
        let quarter = std::f64::consts::FRAC_PI_2;
        let three_quarters = 3.0 * quarter;
        assert!(
            (shortest_angle_between(0.0, three_quarters) + quarter).abs() < 1e-12,
            "270° — это −90°"
        );
        assert!((shortest_angle_between(quarter, 0.0) + quarter).abs() < 1e-12);
        assert!(shortest_angle_between(1.0, 1.0).abs() < 1e-12);
    }
}
