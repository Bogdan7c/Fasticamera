//! М1 «реестр режимов съёмки» (`docs/ios-camera/feature-map.md`): карусель строится из реестра
//! **реализованных** режимов (решения 80 и 83), добавление режима — новая запись, а не правка карусели.

use crate::tools::ToolSet;

/// Режим съёмки. Варианты кроме `Photo` — именованные места под будущие режимы: в реестр v1 они
/// не входят, и экран отвергает их выбор явным итогом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureMode {
    Photo,
    /// Видео (план `video`). Не реализовано.
    Video,
    /// Портрет (бэклог). Не реализовано.
    Portrait,
}

/// Описание режима: что показать в карусели и какие инструменты у режима.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeDescriptor {
    pub mode: CaptureMode,
    /// Подпись в карусели, как у iOS — заглавными (UI-текст английский).
    pub label: &'static str,
    /// Значки над кадром и меню Controls этого режима.
    pub tools: ToolSet,
}

/// Реестр режимов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeRegistry {
    descriptors: Vec<ModeDescriptor>,
}

impl ModeRegistry {
    /// Реестр реализованных режимов v1: только Photo с набором инструментов из конфига.
    pub fn implemented(photo_tools: ToolSet) -> Self {
        Self {
            descriptors: vec![ModeDescriptor {
                mode: CaptureMode::Photo,
                label: "PHOTO",
                tools: photo_tools,
            }],
        }
    }

    /// Режимы в порядке карусели слева направо.
    pub fn descriptors(&self) -> &[ModeDescriptor] {
        &self.descriptors
    }

    /// Описание режима; `None` — режим не реализован.
    pub fn descriptor(&self, mode: CaptureMode) -> Option<&ModeDescriptor> {
        self.descriptors.iter().find(|descriptor| descriptor.mode == mode)
    }

    /// Режим по умолчанию при запуске (первый в реестре).
    pub fn default_mode(&self) -> CaptureMode {
        self.descriptors
            .first()
            .map_or(CaptureMode::Photo, |descriptor| descriptor.mode)
    }
}
