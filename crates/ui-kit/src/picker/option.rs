//! K5 «Выбор из вариантов»: варианты — данные, выбор одиночный.

use std::fmt::Debug;

use thiserror::Error;

/// Непрозрачный идентификатор подписи (ключ строки; текст и перевод — забота экрана).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LabelId(pub u32);

/// Иконка из SDF-атласа (решения 81, 88): один тип на весь UI, набор запекает `ui-draw` при сборке.
/// Раньше здесь была временная заглушка `IconId(u32)`; правило второго использования (решение 84).
pub use ui_draw::icons::IconId;

/// Иконка варианта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionIcon {
    /// Только подпись.
    LabelOnly,
    Icon(IconId),
}

/// Вариант: значение (его же сравнивают) и то, как его показать.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickerOption<T> {
    pub value: T,
    pub label: LabelId,
    pub icon: OptionIcon,
}

/// Ошибка выбора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PickerError {
    #[error("список вариантов пуст")]
    EmptyOptions,
    #[error("в списке вариантов есть повторяющиеся значения")]
    DuplicateValue,
    /// Значения нет среди вариантов.
    #[error("значения нет среди вариантов")]
    UnknownValue,
    #[error("индекс варианта {index} вне списка из {length}")]
    IndexOutOfRange { index: usize, length: usize },
}

/// Итог выбора: изменился ли он. Различие нужно экрану (отправлять ли намерение камере).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionChange<T> {
    Changed {
        from: T,
        to: T,
    },
    /// Выбран тот же вариант: ничего не изменилось.
    Unchanged,
}

/// Выбор одного варианта из списка.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionPicker<T> {
    options: Vec<PickerOption<T>>,
    selected: usize,
}

impl<T: Copy + PartialEq + Debug> OptionPicker<T> {
    /// Создать выбор. Ошибки: пустой список, повторы, `selected` не из списка.
    pub fn new(options: Vec<PickerOption<T>>, selected: T) -> Result<Self, PickerError> {
        if options.is_empty() {
            return Err(PickerError::EmptyOptions);
        }
        let has_duplicates = options
            .iter()
            .enumerate()
            .any(|(index, option)| options[..index].iter().any(|earlier| earlier.value == option.value));
        if has_duplicates {
            return Err(PickerError::DuplicateValue);
        }
        let selected = Self::position_of(&options, selected)?;
        Ok(Self { options, selected })
    }

    pub fn options(&self) -> &[PickerOption<T>] {
        &self.options
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> &PickerOption<T> {
        &self.options[self.selected]
    }

    /// Выбрать вариант по индексу.
    pub fn select_index(&mut self, index: usize) -> Result<SelectionChange<T>, PickerError> {
        if index >= self.options.len() {
            return Err(PickerError::IndexOutOfRange {
                index,
                length: self.options.len(),
            });
        }
        let from = self.selected().value;
        self.selected = index;
        let to = self.selected().value;
        Ok(if from == to {
            SelectionChange::Unchanged
        } else {
            SelectionChange::Changed { from, to }
        })
    }

    /// Выбрать вариант по значению; значение не из списка — ошибка, выбор не меняется.
    pub fn select_value(&mut self, value: T) -> Result<SelectionChange<T>, PickerError> {
        let index = Self::position_of(&self.options, value)?;
        self.select_index(index)
    }

    fn position_of(options: &[PickerOption<T>], value: T) -> Result<usize, PickerError> {
        options
            .iter()
            .position(|option| option.value == value)
            .ok_or(PickerError::UnknownValue)
    }
}
