//! Цветовые приборы: экран только просит (решение 183: «экран спрашивает, ничего не считает»).
//!
//! Инвариант модуля: редьюсер НЕ хранит, какой прибор открыт, и не проверяет размер картинки. Предел размера
//! знает только рендер (`viewfinder.scopes.max_picture_side_px`), он же отвечает отказом; раздел калибровки с
//! раскладкой и показом отказа придёт в 14b. Поэтому здесь нет ни отказов, ни изменения состояния экрана.

use scope_contract::ScopeIntent;

use super::Ctx;
use crate::effect::Effect;
use crate::outcome::ActionOutcome;

/// Намерение по прибору уходит наружу ровно одним эффектом, без изменений и без отказов.
pub(super) fn on_scopes(ctx: &mut Ctx<'_>, intent: ScopeIntent) -> ActionOutcome {
    ctx.emit(Effect::Scopes(intent));
    ActionOutcome::Applied
}
