//! Журнал белого: «значение и происхождение видны в диагностике» (критерий выхода white-balance 01).
//!
//! Пишет движок в момент события: продукт события движка не разбирает, а наблюдать за статусом со стороны значило
//! бы просыпаться на каждом кадре. Событий мало (открытие, команда, ответ), строка на событие.

use camera_contract::white::HeldWhite;

use crate::white::WhiteRequestResult;

/// Цель журнала: один фильтр собирает и белую, и её память.
const LOG_TARGET: &str = "fasticamera::white";

/// Действующая белая: стартовая или новая.
pub(super) fn white_changed(held: &HeldWhite) {
    tracing::info!(
        target: LOG_TARGET,
        kelvin = held.setting.temperature.kelvin(),
        duv = held.setting.tint.duv(),
        origin = ?held.origin,
        "белая"
    );
}

/// Заявка источнику закрыта (любым исходом).
pub(super) fn request_finished(finished: &WhiteRequestResult) {
    tracing::info!(
        target: LOG_TARGET,
        request = finished.request.0,
        source = finished.source.name(),
        outcome = ?finished.result,
        "заявка источнику белого закрыта"
    );
}
