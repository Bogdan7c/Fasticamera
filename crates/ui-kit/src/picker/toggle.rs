//! K6 «Переключатель-значок»: частный случай K5 с показом одного текущего состояния.

use std::fmt::Debug;

use serde::{Deserialize, Serialize};

use super::option::{OptionPicker, PickerError, SelectionChange};

/// Что делает тап по значку.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapBehavior {
    /// Следующий вариант по кругу (вспышка Off → On → Auto → Off).
    #[default]
    CycleThroughOptions,
    /// Тап открывает подменю; варианты выбираются в нём.
    OpenSubmenu,
}

/// Что произошло от жеста по значку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleAction<T> {
    /// Выбран другой вариант.
    Selected { from: T, to: T },
    /// Вариант остался прежним (в списке один вариант).
    Unchanged,
    /// Экрану нужно открыть подменю.
    OpenSubmenu,
}

/// Значок-переключатель над [`OptionPicker`].
#[derive(Debug, Clone, PartialEq)]
pub struct IconToggle<T> {
    picker: OptionPicker<T>,
    tap_behavior: TapBehavior,
}

impl<T: Copy + PartialEq + Debug> IconToggle<T> {
    pub fn new(picker: OptionPicker<T>, tap_behavior: TapBehavior) -> Self {
        Self { picker, tap_behavior }
    }

    pub fn picker(&self) -> &OptionPicker<T> {
        &self.picker
    }

    pub fn tap_behavior(&self) -> TapBehavior {
        self.tap_behavior
    }

    /// Выбрать вариант из подменю (или программно).
    pub fn select_value(&mut self, value: T) -> Result<SelectionChange<T>, PickerError> {
        self.picker.select_value(value)
    }

    /// Тап по значку.
    pub fn tapped(&mut self) -> Result<ToggleAction<T>, PickerError> {
        match self.tap_behavior {
            TapBehavior::OpenSubmenu => Ok(ToggleAction::OpenSubmenu),
            TapBehavior::CycleThroughOptions => {
                let next = (self.picker.selected_index() + 1) % self.picker.options().len();
                Ok(match self.picker.select_index(next)? {
                    SelectionChange::Changed { from, to } => ToggleAction::Selected { from, to },
                    SelectionChange::Unchanged => ToggleAction::Unchanged,
                })
            }
        }
    }

    /// Долгий тап: подменю открывается всегда, при любом поведении тапа. Выбор не меняется.
    pub fn long_pressed(&self) -> ToggleAction<T> {
        ToggleAction::OpenSubmenu
    }
}
