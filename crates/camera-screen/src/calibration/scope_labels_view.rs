//! Подписи прибора на экране (решения 326, 327): последнее, что рендер сообщил о положении подписей.
//!
//! Экран подписи НЕ считает: положение буквы цели вектороскопа зависит от охвата экрана, масштаба из настроек и зума,
//! это знает только рендер. Здесь они только хранятся и отдаются рисованию с проверкой запроса: подписи, посчитанные
//! для другого запроса (после свайпа, смены осей, зума или размера), НЕ отдаются — иначе поверх нового прибора ещё
//! кадр-другой стояли бы буквы прежнего.

use scope_contract::{ScopeLabel, ScopeLabelsState, ScopeRequest};

/// Хранитель последних подписей прибора.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ScopeLabelsView {
    latest: ScopeLabelsState,
}

impl ScopeLabelsView {
    /// Принимает новое значение; `false` — оно такое же, как было (состояние не менялось).
    pub(crate) fn store(&mut self, state: ScopeLabelsState) -> bool {
        if self.latest == state {
            return false;
        }
        self.latest = state;
        true
    }

    /// Подписи, которые можно рисовать при желании экрана `desired`: только посчитанные именно для него.
    pub(crate) fn labels_for(&self, desired: &ScopeRequest) -> &[ScopeLabel] {
        self.latest.labels_for(desired)
    }
}
