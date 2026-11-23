//! Намерения экрана: экран ничего не считает и не пишет на диск, он только шлёт их исполнителю.

use crate::preset::{PresetId, PresetName, UserPresetId};
use crate::settings::LookSettings;

/// Что владелец хочет сделать с видом или пресетами.
#[derive(Debug, Clone, PartialEq)]
pub enum LookIntent {
    /// «Вид = эти значения»: правки поверх активного пресета (пресет не меняется).
    SetLook(LookSettings),
    /// Выбрать пресет: он становится активным, правки поверх сбрасываются.
    SelectPreset(PresetId),
    /// Сохранить текущий вид как новый свой пресет с этим именем; он становится активным.
    SaveAsNewPreset(PresetName),
    /// Переименовать свой пресет.
    RenamePreset {
        /// Какой пресет.
        preset: UserPresetId,
        /// Новое имя.
        name: PresetName,
    },
    /// Записать текущий вид в свой пресет (перезаписать его значения).
    OverwritePreset(UserPresetId),
    /// Удалить свой пресет.
    DeletePreset(UserPresetId),
    /// Отбросить правки поверх: вернуть значения активного пресета.
    DiscardEdits,
}
