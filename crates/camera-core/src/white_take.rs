//! Исход команды «взять белую»: отчёт, который экран читает из статуса (white-balance 02, WB31).
//!
//! Зачем отдельно от [`crate::white::WhiteRequestState`]: тот описывает **заявку источнику** (в том числе
//! заявку открытия приложения), а экрану нужен исход **именно своей команды** — иначе возобновление приложения
//! или чужая заявка перепутали бы дрожание кнопки «Phone» (решение WB32). Отчёт ведёт владелец белого вместе
//! с заявкой; экран только читает.

use serde::Serialize;

use crate::command::{CommandId, InvalidStateReason};
use crate::white::WhiteRequestOutcome;

/// Где сейчас команда «взять белую».
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum WhiteTakeState {
    /// Заявка выдана источнику, ответа ещё нет.
    Waiting,
    /// Заявка команды закрыта; исход — тот же, что в событии `WhiteRequestFinished`.
    Finished(WhiteRequestOutcome),
    /// Команда отклонена до заявки: источнику ничего не передавали, белая не менялась. Причина различима
    /// (нет потока, движок сломан), потому что экрану важно, «спросили и не вышло» это или «не спрашивали».
    NotAsked { reason: InvalidStateReason },
}

/// Отчёт о последней команде «взять белую». Хранится один — последний: новая команда закрывает прежний
/// отчёт (если он ещё ждал — исходом `Cancelled`) и заводит свой.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteTakeReport {
    /// Номер команды, которой принадлежит отчёт (экран сверяет со своим).
    pub command: CommandId,
    pub state: WhiteTakeState,
}

impl WhiteTakeReport {
    /// Команда принята, заявка выдана.
    pub(crate) fn waiting(command: CommandId) -> Self {
        Self {
            command,
            state: WhiteTakeState::Waiting,
        }
    }

    /// Команда отклонена без заявки.
    pub(crate) fn not_asked(command: CommandId, reason: InvalidStateReason) -> Self {
        Self {
            command,
            state: WhiteTakeState::NotAsked { reason },
        }
    }

    /// Тот же отчёт, закрытый исходом заявки.
    pub(crate) fn finished(self, outcome: WhiteRequestOutcome) -> Self {
        Self {
            state: WhiteTakeState::Finished(outcome),
            ..self
        }
    }
}
