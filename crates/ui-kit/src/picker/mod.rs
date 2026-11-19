//! K5 «Выбор из вариантов» и K6 «Переключатель-значок» (`components.md`).
//!
//! Варианты — данные: значение плюс непрозрачные идентификаторы подписи и иконки. Элемент не знает,
//! что это «таймер» или «вспышка». Вибрации у выбора нет (`haptics.md`: кнопки — без вибрации).

mod option;
mod toggle;

pub use option::{IconId, LabelId, OptionIcon, OptionPicker, PickerError, PickerOption, SelectionChange};
pub use toggle::{IconToggle, TapBehavior, ToggleAction};
