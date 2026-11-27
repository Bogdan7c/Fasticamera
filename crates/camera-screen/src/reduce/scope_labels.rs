//! Подписи прибора (решения 326, 327): рендер сообщил, где на картинке стоят буквы целей вектороскопа.
//!
//! Инвариант модуля: экран подписи только ЗАПОМИНАЕТ (как упоры ручек у `look`). Положения он не считает, эффектов не
//! порождает и намерений по ним не шлёт. Какие подписи рисовать, решает рисование по текущему запросу прибора.

use scope_contract::ScopeLabelsState;

use super::Ctx;
use crate::outcome::{ActionOutcome, NoChangeReason};

/// Новые подписи прибора от рендера: то же значение — «без изменений», иначе запомнено.
pub(super) fn on_scope_labels_changed(ctx: &mut Ctx<'_>, labels: ScopeLabelsState) -> ActionOutcome {
    if ctx.state.calibration.store_scope_labels(labels) {
        ActionOutcome::Applied
    } else {
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    }
}
