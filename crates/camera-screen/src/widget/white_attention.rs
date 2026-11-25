//! «Внимание» кнопок белого: мерцание ожидания ответа источника и тряска отказа (WB32).
//!
//! Один и тот же набор у двух кнопок (белого у затвора и «Phone» на карточке): обе показывают одно событие
//! движка, поэтому читают одни факты и одно состояние экрана. Мерцание ведёт `facts.white.take` (только своя команда «взять»)
//! ([`button_manner`]), тряску — рост [`crate::white::WhiteScreenState::refusal_serial`]. Сравнение номера
//! отказа — по НЕРАВЕНСТВУ с виденным: после пересборки сцены (смена размера) стартовое значение берётся из
//! состояния, поэтому «старый» отказ заново не трясёт.
//!
//! Сами эффекты — общие элементы `ui-kit` ([`WaitingFlicker`], [`RefusalShake`]); Reduce Motion они учитывают сами
//! (тряска заменяется короткой потерей прозрачности).

use ui_core::time::MonotonicNanos;
use ui_kit::attention::{RefusalShake, WaitingFlicker};

use super::Ctx;
use crate::white_display::{WhiteButtonManner, button_manner};

/// Состояние внимания одной кнопки белого.
#[derive(Debug, Clone)]
pub(crate) struct WhiteAttention {
    flicker: WaitingFlicker,
    shake: RefusalShake,
    /// Номер отказа, который кнопка уже показала.
    seen_refusal_serial: u64,
}

impl WhiteAttention {
    /// Покоящееся внимание; уже случившиеся отказы не показываются.
    pub(crate) fn new(ctx: &Ctx<'_>) -> Self {
        Self {
            flicker: WaitingFlicker::new(&ctx.tokens.attention),
            shake: RefusalShake::new(&ctx.tokens.attention, ctx.motion),
            seen_refusal_serial: ctx.state.white().refusal_serial(),
        }
    }

    /// Выравнивает мерцание и тряску по состоянию экрана; зовётся на каждом кадре.
    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) {
        self.shake.set_motion(ctx.motion);
        match button_manner(ctx.state.camera_facts().white.take) {
            WhiteButtonManner::Waiting => self.flicker.start(ctx.now),
            WhiteButtonManner::Normal => self.flicker.stop(ctx.now),
        }
        let serial = ctx.state.white().refusal_serial();
        if serial != self.seen_refusal_serial {
            self.seen_refusal_serial = serial;
            self.shake.trigger(ctx.now);
        }
    }

    /// Идёт ли ещё какое-то движение (кадры нужны, пока оно не кончилось).
    pub(crate) fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.flicker.is_animating(now) || self.shake.is_animating(now)
    }

    /// Итоговая прозрачность кнопки: мерцание × затемнение Reduce Motion.
    pub(crate) fn opacity(&self, now: MonotonicNanos) -> f32 {
        (self.flicker.opacity(now) * self.shake.opacity(now)).clamp(0.0, 1.0) as f32
    }

    /// Сдвиг тряски вбок, pt (при Reduce Motion всегда 0).
    pub(crate) fn offset_x_pt(&self, now: MonotonicNanos) -> f64 {
        self.shake.offset_x_pt(now)
    }
}
