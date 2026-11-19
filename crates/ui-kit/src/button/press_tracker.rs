//! [`PressTracker`] — состояние кнопки «нажата / отпущена» и момент срабатывания.
//!
//! Правило из `docs/ios-camera/behavior.md` 1.1 (WWDC18 803): подсветка **сразу при касании**,
//! срабатывание **по отпусканию**; увёл палец с кнопки — подсветка снимается, вернул — включается
//! снова; отпустил за пределами — ничего не происходит; системная отмена — подсветка снята без
//! срабатывания.
//!
//! # Зона отслеживания
//! «На кнопке» значит «внутри рамки, расширенной на отступ» (токен
//! `press_tracking_slop_points`, `[reverse]`): палец, чуть соскользнувший с края, кнопку не
//! отпускает. Рамку трекеру отдаёт экран из раскладки (она может поменяться: [`PressTracker::set_frame`]).
//!
//! # Рецепт: кнопка = `Tap` + `Press` на одном узле
//! На узел кнопки в [`ui_core::scene::UiScene`] ставятся **оба** распознавателя:
//! * `Press` даёт подсветку и срабатывание (его события идут в [`PressTracker::handle_press`]);
//! * `Tap` на том же узле нужен **арбитражу**: `Press` не эксклюзивен и сам по себе не отнимает
//!   касание у предков. Если кнопка лежит внутри видоискателя с `Tap` (фокус), кнопка только с
//!   `Press` сработает **вместе с фокусом**. Глубокий `Tap` на кнопке выигрывает у `Tap` предка
//!   (он ближе к пальцу), и фокус получает `Failed`.
//!
//! Трекер событий `Tap` не принимает намеренно: нажатие решает один источник (`Press`), иначе
//! «сработала ли кнопка» зависело бы от двух распознавателей с разными порогами (`Tap` провалится
//! при сдвиге больше 10 pt, а зона трекера шире).

use thiserror::Error;
use ui_core::geometry::{Insets, Point, Rect};
use ui_core::gesture::{GestureData, GestureState};

use crate::tokens::UiKitTokens;

/// Как кнопка выглядит сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    /// Не нажата.
    Normal,
    /// Палец на кнопке (в зоне отслеживания): подсветка включена.
    Pressed,
}

/// Что принесло очередное событие `Press`. Различия важны экрану: подсветка и срабатывание — разные
/// реакции, и «ничего не изменилось» не должно перерисовывать кнопку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressOutcome {
    /// Состояние не менялось, срабатывания нет.
    Unchanged,
    /// Подсветка включилась или выключилась (новое состояние внутри). Срабатывания нет.
    StateChanged(ButtonState),
    /// Палец отпущен в зоне отслеживания: кнопка сработала, подсветка снята.
    Activated,
}

/// Ошибки трекера.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ButtonError {
    /// Рамка кнопки должна быть конечной и непустой.
    #[error("рамка кнопки должна быть конечной и непустой: {frame:?}")]
    InvalidFrame { frame: Rect },
    /// Токены не прошли проверку (в том числе отступ зоны отслеживания).
    #[error("токены: {0}")]
    Tokens(#[from] crate::tokens::TokenError),
    /// Событие нажатия пришло без данных `Press`: на узел подписан не тот распознаватель.
    #[error("событие {state:?} пришло без данных `Press`")]
    NotPressData { state: GestureState },
}

/// Механика нажатия одной кнопки.
#[derive(Debug, Clone, PartialEq)]
pub struct PressTracker {
    /// Зона отслеживания: рамка кнопки плюс отступ.
    tracking_zone: Rect,
    /// Отступ (из токенов), хранится, чтобы пересчитать зону при смене рамки.
    slop_points: f64,
    state: ButtonState,
}

impl PressTracker {
    /// Создать трекер для кнопки с рамкой `frame` (pt, в тех же координатах, что положения касаний).
    pub fn new(frame: Rect, tokens: &UiKitTokens) -> Result<Self, ButtonError> {
        tokens.validate()?;
        let slop_points = tokens.density.press_tracking_slop_points;
        Ok(Self {
            tracking_zone: Self::zone_of(frame, slop_points)?,
            slop_points,
            state: ButtonState::Normal,
        })
    }

    fn zone_of(frame: Rect, slop_points: f64) -> Result<Rect, ButtonError> {
        let finite = [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height]
            .iter()
            .all(|value| value.is_finite());
        if !finite || frame.size.width <= 0.0 || frame.size.height <= 0.0 {
            return Err(ButtonError::InvalidFrame { frame });
        }
        Ok(frame.outset_by(Insets::uniform(slop_points)))
    }

    pub fn state(&self) -> ButtonState {
        self.state
    }

    /// Зона отслеживания (рамка + отступ), pt.
    pub fn tracking_zone(&self) -> Rect {
        self.tracking_zone
    }

    /// Рамка кнопки изменилась (вёрстка). Идущее нажатие не прерывается: следующее событие
    /// сравнится уже с новой зоной. Ошибка — рамка вырождена; тогда трекер не меняется.
    pub fn set_frame(&mut self, frame: Rect) -> Result<(), ButtonError> {
        self.tracking_zone = Self::zone_of(frame, self.slop_points)?;
        Ok(())
    }

    /// Применить событие распознавателя `Press` этой кнопки.
    ///
    /// * `Began` / `Changed` — подсветка по положению пальца относительно зоны;
    /// * `Ended` — палец в зоне: [`PressOutcome::Activated`]; вне зоны: срабатывания нет;
    /// * `Cancelled` / `Failed` — подсветка снята, срабатывания нет;
    /// * `Possible` арена не публикует; это [`PressOutcome::Unchanged`].
    pub fn handle_press(&mut self, state: GestureState, data: GestureData) -> Result<PressOutcome, ButtonError> {
        match state {
            GestureState::Began | GestureState::Changed => {
                let position = Self::press_position(state, data)?;
                Ok(self.highlight(self.zone_state(position)))
            }
            GestureState::Ended => {
                let position = Self::press_position(state, data)?;
                if self.tracking_zone.contains(position) {
                    self.state = ButtonState::Normal;
                    Ok(PressOutcome::Activated)
                } else {
                    Ok(self.highlight(ButtonState::Normal))
                }
            }
            GestureState::Cancelled | GestureState::Failed => Ok(self.highlight(ButtonState::Normal)),
            GestureState::Possible => Ok(PressOutcome::Unchanged),
        }
    }

    fn press_position(state: GestureState, data: GestureData) -> Result<Point, ButtonError> {
        match data {
            GestureData::Press { position } => Ok(position),
            _ => Err(ButtonError::NotPressData { state }),
        }
    }

    fn zone_state(&self, position: Point) -> ButtonState {
        if self.tracking_zone.contains(position) {
            ButtonState::Pressed
        } else {
            ButtonState::Normal
        }
    }

    /// Выставить подсветку; сообщает об изменении только когда оно было.
    fn highlight(&mut self, next: ButtonState) -> PressOutcome {
        if self.state == next {
            return PressOutcome::Unchanged;
        }
        self.state = next;
        PressOutcome::StateChanged(next)
    }
}
