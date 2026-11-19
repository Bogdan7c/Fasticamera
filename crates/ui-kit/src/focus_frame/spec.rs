//! Описание рамки фокуса: шкала «солнца» и правило удержания.

use ui_core::time::UiDuration;

use super::error::FocusFrameError;
use crate::drive::GrowthDirection;
use crate::scale::{ScaleSpec, TickVisibility};
use crate::tokens::FocusTokens;

/// Как долго рамка держится на экране.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusHold {
    /// Держать, пока вызывающий не скажет иначе (блокировка AE/AF LOCK).
    UntilReleased,
    /// Потускнеть через `dim_after`, исчезнуть через `hide_after` после последней активности.
    AutoFade {
        dim_after: UiDuration,
        hide_after: UiDuration,
    },
}

impl FocusHold {
    /// Автозатухание со сроками из токенов (`focus.dim_after_s`, `focus.hide_after_s`).
    pub fn auto_fade(tokens: &FocusTokens) -> Self {
        FocusHold::AutoFade {
            dim_after: UiDuration::from_secs_f64(tokens.dim_after_s),
            hide_after: UiDuration::from_secs_f64(tokens.hide_after_s),
        }
    }

    pub(super) fn validate(self) -> Result<(), FocusFrameError> {
        match self {
            FocusHold::UntilReleased => Ok(()),
            FocusHold::AutoFade { dim_after, hide_after } if dim_after > UiDuration::ZERO && hide_after > dim_after => {
                Ok(())
            }
            FocusHold::AutoFade { .. } => Err(FocusFrameError::InvalidHold),
        }
    }
}

/// Описание рамки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusFrameSpec {
    /// Шкала «солнца»: ось — вертикаль, рисок нет (проверяется при создании).
    pub sun: ScaleSpec,
    /// Удержание по умолчанию — «держать»: экран выбирает автозатухание явно
    /// ([`FocusHold::auto_fade`]) для обычного тап-фокуса.
    pub hold: FocusHold,
}

impl Default for FocusFrameSpec {
    fn default() -> Self {
        Self {
            sun: ScaleSpec {
                direction: GrowthDirection::FingerUp,
                ticks: TickVisibility::Hidden,
                ..ScaleSpec::default()
            },
            hold: FocusHold::UntilReleased,
        }
    }
}

impl FocusFrameSpec {
    pub(super) fn validate(&self) -> Result<(), FocusFrameError> {
        if self.sun.direction.axis() != crate::drive::ScreenAxis::Vertical {
            return Err(FocusFrameError::SunMustBeVertical(self.sun.direction));
        }
        if self.sun.ticks != TickVisibility::Hidden {
            return Err(FocusFrameError::SunTicksMustBeHidden);
        }
        self.hold.validate()
    }
}
