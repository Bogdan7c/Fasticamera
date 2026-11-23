//! Блокирующее чтение результатов приборов для ПК-тестов (фича `test-support`).
//!
//! Ожидания GPU здесь — суть: на горячем пути их нет. Вызывать после того, как отправленный `record` завершился
//! (тест дождался fence). Чтение ничего не меняет в состоянии приборов.

use gpu_vulkan::kit_test_support::read_storage_image_rgba;
use scope_contract::ScopeImageSize;
use scope_model::ScopeCounters;

use super::color_scopes::ColorScopes;
use super::error::ScopeError;

/// Картинка прибора, прочитанная на CPU: `[r, g, b, a]` построчно сверху вниз, прямая альфа.
#[derive(Debug, Clone, PartialEq)]
pub struct PictureReadback {
    /// Размер картинки.
    pub size: ScopeImageSize,
    /// Пиксели.
    pub texels: Vec<[f32; 4]>,
}

impl ColorScopes {
    /// Читает счётчики последней записи: ровно `cell_count` ячеек её сетки.
    pub fn read_back_counters_blocking(&self) -> Result<ScopeCounters, ScopeError> {
        let grid = self.last_grid().ok_or_else(|| ScopeError::CounterReadback {
            what: "кадров приборов ещё не записано".to_owned(),
        })?;
        let bytes = self.counters_buffer().read_back_blocking()?;
        let counts: Vec<u32> = bytes
            .chunks_exact(4)
            .take(grid.cell_count())
            .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        ScopeCounters::from_counts(grid, counts).map_err(|error| ScopeError::CounterReadback {
            what: error.to_string(),
        })
    }

    /// Читает картинку прибора последней записи.
    pub fn read_back_picture_blocking(&self) -> Result<PictureReadback, ScopeError> {
        let picture = self.picture_target().ok_or_else(|| ScopeError::CounterReadback {
            what: "картинка прибора ещё не создана".to_owned(),
        })?;
        let texels = read_storage_image_rgba(picture.storage(), picture.current_use())?;
        Ok(PictureReadback {
            size: picture.size(),
            texels,
        })
    }
}
