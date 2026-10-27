//! Буферы статистики AWB по слотам кадров и их чтение в [`AwbStatistics`] `isp-model`.
//!
//! Шейдер копит суммы атомиками в буфер слота; раскладка — плитки по строкам сетки, каждая
//! `AwbTile` = 5 × u32 `{sum_r, sum_g, sum_b, used_cells, saturated_cells}` (`#[repr(C)]` в `isp-model`).
//! Буфер обнуляется командой в начале каждой записи слота (`record_clear_for_compute`) и читается
//! CPU только после fence слота (контракт `ViewfinderIsp::collect`).
//!
//! Статистика считается по КАДРИРОВАНИЮ (то, что видит пользователь), в ориентации сенсора: плитка
//! `(column, row)` — это `tile_of_cell` над ячейками кадрирования, как `compute_awb_statistics` над
//! мозаикой кадрирования. Поворот на экран к статистике не применяется.
//!
//! # Совпадение с эталоном CPU
//! Нормировка уровней в шейдере — обычное деление (`OpFDiv`, по спецификации Vulkan до 2,5 ULP; точнее
//! не делаем ради скорости). Поэтому: на RADV суммы совпадают с `compute_awb_statistics` ТОЧНО при
//! делителе-степени двойки; на любом устройстве гарантия — расходиться (на ±1 шаг квантования суммы или
//! флаг пересвета) могут только ячейки, чей нормированный отсчёт в пределах 3 ULP от границы квантования
//! или порога пересвета (тесты `tests/viewfinder_awb.rs`). Для AWB это шум ниже 1/4095 на ячейку.

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use isp_model::statistics::{AwbStatistics, AwbTile, TileGrid};

use super::error::ViewfinderError;

/// u32 на плитку (поля `AwbTile`).
const WORDS_PER_TILE: usize = 5;
/// Байт на плитку.
const BYTES_PER_TILE: usize = WORDS_PER_TILE * std::mem::size_of::<u32>();

/// Размер буфера статистики сетки, байт.
pub(crate) fn tile_buffer_bytes(grid: TileGrid) -> Result<u64, ViewfinderError> {
    grid.tile_count()
        .checked_mul(BYTES_PER_TILE as u64)
        .ok_or_else(|| ViewfinderError::InvalidConfig {
            what: format!("сетка AWB {grid:?} слишком велика для буфера"),
        })
}

/// Буферы статистики — по одному на слот кадра.
pub(crate) struct AwbTileBuffers {
    buffers: Vec<HostBuffer>,
    grid: TileGrid,
}

impl AwbTileBuffers {
    pub fn new(context: &GpuContext, frame_slots: usize, grid: TileGrid) -> Result<Self, ViewfinderError> {
        let size_bytes = tile_buffer_bytes(grid)?;
        let buffers = (0..frame_slots)
            .map(|_| HostBuffer::new(context, size_bytes, HostBufferUsage::ReadWriteStorage))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { buffers, grid })
    }

    /// Буфер слота (слот проверен вызывающим).
    pub fn slot(&self, frame_slot: usize) -> Result<&HostBuffer, ViewfinderError> {
        self.buffers
            .get(frame_slot)
            .ok_or(ViewfinderError::FrameSlotOutOfRange {
                slot: frame_slot,
                slots: self.buffers.len(),
            })
    }

    /// Читает статистику слота. Контракт: fence последней записи слота дождались.
    pub fn read(&self, frame_slot: usize) -> Result<AwbStatistics, ViewfinderError> {
        let buffer = self.slot(frame_slot)?;
        let length = usize::try_from(buffer.size_bytes()).map_err(|_| ViewfinderError::InvalidConfig {
            what: "буфер статистики не помещается в адресное пространство".to_owned(),
        })?;
        let bytes = buffer.read_bytes(0, length)?;
        Ok(AwbStatistics::from_tiles(self.grid, tiles_from_bytes(&bytes))?)
    }
}

/// Байты буфера → плитки (порядок полей `AwbTile`, родной порядок байт — как пишет GPU).
fn tiles_from_bytes(bytes: &[u8]) -> Vec<AwbTile> {
    bytes
        .chunks_exact(BYTES_PER_TILE)
        .map(|tile| {
            let word = |index: usize| {
                let offset = index * 4;
                u32::from_ne_bytes([tile[offset], tile[offset + 1], tile[offset + 2], tile[offset + 3]])
            };
            AwbTile {
                sum_r: word(0),
                sum_g: word(1),
                sum_b: word(2),
                used_cells: word(3),
                saturated_cells: word(4),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_decode_into_tiles_in_field_order() {
        let words: [u32; 10] = [1, 2, 3, 4, 5, 10, 20, 30, 40, 50];
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_ne_bytes()).collect();
        let tiles = tiles_from_bytes(&bytes);
        assert_eq!(
            tiles,
            vec![
                AwbTile {
                    sum_r: 1,
                    sum_g: 2,
                    sum_b: 3,
                    used_cells: 4,
                    saturated_cells: 5
                },
                AwbTile {
                    sum_r: 10,
                    sum_g: 20,
                    sum_b: 30,
                    used_cells: 40,
                    saturated_cells: 50
                },
            ]
        );
        assert_eq!(tile_buffer_bytes(TileGrid { columns: 32, rows: 24 }), Ok(32 * 24 * 20));
    }
}
