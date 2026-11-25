//! Журнал белого: «значение и происхождение видны в диагностике» (критерий выхода white-balance 01).
//!
//! Пишет движок в момент события: продукт события движка не разбирает, а наблюдать за статусом со стороны значило
//! бы просыпаться на каждом кадре. Событий мало (открытие, команда, ответ), строка на событие.

use camera_contract::white::{HeldWhite, WhiteOrigin};

use crate::white::{WhiteFollowRecord, WhiteRequestResult, WhiteResumeRule};

/// Цель журнала: один фильтр собирает и белую, и её память.
pub(super) const LOG_TARGET: &str = "fasticamera::white";

/// Стартовая белая движка (один раз за запуск).
pub(super) fn white_started(held: &HeldWhite) {
    tracing::info!(
        target: LOG_TARGET,
        kelvin = held.setting.temperature.kelvin(),
        duv = held.setting.tint.duv(),
        origin = ?held.origin,
        "белая"
    );
}

/// Белая сменилась. Ручные правки идут потоком (шкала под пальцем — до 120 раз в секунду), поэтому строка о них
/// только в `debug`; значение, на котором правка остановилась, попадает в `info` строкой записи памяти белой
/// (`fasticamera::white`, «память белой записана»). Смены от источника редки и остаются в `info`.
pub(super) fn white_changed(held: &HeldWhite) {
    if held.origin == WhiteOrigin::Manual {
        tracing::debug!(
            target: LOG_TARGET,
            kelvin = held.setting.temperature.kelvin(),
            duv = held.setting.tint.duv(),
            origin = ?held.origin,
            "белая (ручная правка)"
        );
    } else {
        white_started(held);
    }
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

/// Человек выбрал правило возобновления белого (редкое событие: настройка, не шкала).
pub(super) fn resume_rule_chosen(rule: WhiteResumeRule) {
    tracing::info!(target: LOG_TARGET, rule = ?rule, "правило возобновления белого выбрано");
}

/// Слежение за белым включено, выключено или сменило скорость (действие человека, редкое). Отдельные вопросы и
/// ответы слежения в журнал не идут: строка появляется, только когда белая действительно сдвинулась.
pub(super) fn follow_changed(follow: &WhiteFollowRecord) {
    tracing::info!(
        target: LOG_TARGET,
        keeping = ?follow.keeping,
        chosen_speed_ms = follow.speed.map(crate::white::WhiteFollowSpeed::millis),
        "слежение за белой"
    );
}
