//! Уровень яркости показанного кадра относительно ДОСТИЖИМОЙ цели AE (сессия 13, смена объектива).
//!
//! Зачем отдельный модуль: правило «правдоподобна ли яркость нового потока» — чистая арифметика над
//! числами runner-а, без состояния. Здесь её можно тестировать на границах, не поднимая весь AE.
//!
//! # Почему не «цель AE» как есть
//! В тёмной комнате AE физически не может достичь цели: запрос упирается в самое светлое произведение
//! потока (`reach.brightest`), остаточная подтяжка ISP ограничена (решение 72) и тянет не дальше того же
//! предела. Сравнение с недостижимой целью давало ошибку ≈ −3,5 EV вечно, и КАЖДАЯ смена объектива
//! открывала кадр только по таймауту удержания. Когда кадр стоит на краю достижимого, AE сделал всё
//! возможное: ждать дальше бессмысленно.
//!
//! # Какую цель считаем достижимой
//! Ту же, к которой реально тянет экран контроллер (`AutoExposureController::display_gain`):
//! `clamp(цель + компенсация, reach)`. Предел подтяжки (`max_residual_ev`) отдельно не вычитается и не
//! прибавляется: контроллер сам режет подтяжку этим пределом, поэтому кадр, далёкий от достижимой цели
//! больше чем на подтяжку, честно остаётся «ошибочным» и мы ждём, пока AE дотянет сенсор.
//!
//! # Гарантия «reach принадлежит потоку кадра»
//! В указании движка (`ExposureDirective.applied`) нет номера потока. Защита из двух слоёв:
//! 1. Движок снимает `applied` при каждом конце потока (`end_three_a_stream`) и публикует новое только после
//!    постановки повторяющегося запроса нового потока. Пока нового нет — `applied == None`.
//! 2. Здесь: кадр, снятый по запросу потока, лежит в `reach` ЭТОГО потока (с допуском на округление
//!    выдержки и ISO). Если произведение кадра вне отрезка `reach` — отрезок чужой (старая камера), мы ему не
//!    верим и судим как при неизвестных пределах, то есть по полной цели. Тёмная сцена тогда даёт
//!    `Waiting`, а ограничивает ожидание таймаут удержания.
//!
//! Остаточный риск: чужой отрезок УЖЕ нового, а кадр нового потока случайно лежит внутри него. Тогда цель
//! режется чужим краем и ложное `Ready` возможно, если кадр стоит у этого края. Это бывает только если
//! `applied` не снят при смене потока (слой 1 нарушен), и ограничено таймаутом удержания в обе стороны.

use camera_contract::auto_exposure::{AppliedExposure, ProductReach};

/// Входные числа оценки уровня: всё в log2 от «секунды × ISO», подтяжка в EV.
#[derive(Debug, Clone, Copy)]
pub(super) struct DisplayedLevelInputs {
    /// Произведение «выдержка × ISO» показанного кадра.
    pub(super) frame_log2: f64,
    /// Остаточная подтяжка ISP, уже выбранная для кадра, EV.
    pub(super) gain_ev: f64,
    /// Сглаженная цель AE без компенсации.
    pub(super) target_log2: f64,
    /// Компенсация экспозиции пользователя, EV.
    pub(super) bias_ev: f64,
    /// Что движок сообщил о запросе (`None` — запроса нет или он снят).
    pub(super) applied: Option<AppliedExposure>,
    /// Допуск «кадр лежит в отрезке потока», EV (округление выдержки и ISO, шаг подтяжки).
    pub(super) reach_tolerance_ev: f64,
}

/// Насколько показанный уровень кадра выше достижимой цели AE, EV (отрицательное — темнее).
pub(super) fn displayed_level_error_ev(inputs: DisplayedLevelInputs) -> f64 {
    let reach = trusted_reach(&inputs);
    let achievable_target_log2 = reach.clamp_log2(inputs.target_log2 + inputs.bias_ev);
    inputs.frame_log2 + inputs.gain_ev - achievable_target_log2
}

/// Отрезок достижимого, которому можно верить для ЭТОГО кадра; иначе `Unbounded` (судим по полной цели).
fn trusted_reach(inputs: &DisplayedLevelInputs) -> ProductReach {
    let Some(AppliedExposure {
        reach: reach @ ProductReach::Between { darkest, brightest },
        ..
    }) = inputs.applied
    else {
        // Запроса нет или пределы не известны: ограничивать цель нечем.
        return ProductReach::Unbounded;
    };
    let darkest_log2 = darkest.seconds_iso().log2();
    let brightest_log2 = brightest.seconds_iso().log2().max(darkest_log2);
    let frame_inside = inputs.frame_log2 >= darkest_log2 - inputs.reach_tolerance_ev
        && inputs.frame_log2 <= brightest_log2 + inputs.reach_tolerance_ev;
    if frame_inside { reach } else { ProductReach::Unbounded }
}

#[cfg(test)]
#[path = "reveal_level_tests.rs"]
mod tests;
