//! Статистика для нашего автобаланса белого (AWB, сессия 08): суммы каналов по плиткам кадра.
//!
//! GPU-видоискатель (сессия 07) считает эти суммы в том же проходе, что и картинку, а этот
//! модуль — эталон на CPU. Контракт целочисленный, поэтому GPU сверяется с эталоном ТОЧНО,
//! а не с допуском: при одинаковых нормированных отсчётах суммы обязаны совпасть до единицы.
//!
//! # Вход ([`AwbStatisticsInput`]: две мозаики одного кадра)
//! * **Нормированная** мозаика после уровней, ДО коррекции виньетирования ([`CameraMosaic`]): чёрный 0, белый
//!   сенсора 1. По ней решается ПЕРЕСВЕТ: пересвечен ли отсчёт, определяет сам сенсор, а не наше усиление.
//!   Тёмный угол с усилением ×5–9 не «теряет цвет», даже если после усиления значение выше порога.
//! * **Скорректированная** мозаика после виньетирования, до баланса белого ([`ShadingCorrectedMosaic`]):
//!   по ней считаются СУММЫ (иначе тёмные углы тянули бы оценку цвета и света). Значения могут быть > 1
//!   (усиление до ×9 и уровни сверху не обрезают); квантование оставляет запас `sum_headroom` (по умолчанию 16).
//!   Если стадия пропущена (нет калибровки или выключена), обе мозаики равны.
//!
//! Это ровно то, что шейдер держит до и после стадии виньетирования.
//!
//! # Единица — ячейка 2×2 (суперпиксель)
//! Ячейка `(sx, sy)` — пиксели `(2·sx … 2·sx+1, 2·sy … 2·sy+1)`; цвет каждого из четырёх
//! отсчётов задаёт [`crate::cfa::BayerPattern`] кадра. Изображение ячеек — `(width/2) × (height/2)` (мозаика
//! всегда чётного размера).
//!
//! 1. **Пересвет.** Ячейка исключается (считается в `saturated_cells`), если хотя бы один из
//!    четырёх НОРМИРОВАННЫХ (до виньетирования) отсчётов НЕ меньше порога: `!(v < saturation_threshold)`. Именно такая запись,
//!    а не `v >= threshold`: NaN тоже исключает ячейку (неизвестный свет не должен окрашивать
//!    оценку), и в GLSL выражение `!(v < t)` даёт для NaN тот же ответ.
//! 2. **Квантование.** Каждый отсчёт оставшейся ячейки:
//!    ```text
//!    q = u32( floor( clamp(v, 0.0, headroom) * 4095.0 + 0.5 ) )      — v СКОРРЕКТИРОВАННЫЙ, всё в f32
//!    ```
//!    Масштаб 4095 на единицу света сохранён (потребители делят сумму на `used · 4095`), а потолок —
//!    `headroom` (по умолчанию 16: больше наибольшего усиления калибровки ×9,2), не 1: иначе яркие скорректированные
//!    углы недосчитывались бы. Среднее по плитке теперь может быть > 1.
//!    Округление «половина вверх». Умножение и сложение — две отдельные операции f32 с
//!    округлением каждой: в шейдере выражение обязано быть `precise` (SPIR-V `NoContraction`),
//!    иначе компилятор сольёт его в FMA и редкие значения у границы `.5` округлятся иначе.
//!    Vulkan гарантирует для `OpFMul`/`OpFAdd` правильное округление, `floor` и `clamp` точны.
//! 3. **Плитка.** Сетка `columns × rows` над изображением ячеек:
//!    ```text
//!    tile_x = (sx · columns) / cells_width     — целочисленное деление u32
//!    tile_y = (sy · rows)    / cells_height
//!    ```
//!    Если размер не делится на сетку, плитки отличаются на одну ячейку; порядок плиток в
//!    буфере — по строкам: `index = tile_y · columns + tile_x`.
//! 4. **Накопление в плитке** (u32): `sum_r += q(R)`, `sum_g += q(G₁) + q(G₂)` (оба зелёных),
//!    `sum_b += q(B)`, `used_cells += 1`; исключённая ячейка — только `saturated_cells += 1`.
//!
//! # Почему 12 бит (`QUANT_MAX = 4095`) и запас по сумме
//! AWB нужен средний цвет плитки из тысяч ячеек: шаг 1/4095 в 16 раз мельче шага 8-битного
//! выхода, а усреднение по плитке делает ошибку квантования ничтожной. Худший случай суммы
//! `sum_g = 2 · q_max · ячеек ≤ u32::MAX`, где `q_max = floor(headroom · 4095 + 0,5)`: при `headroom = 16` — до 32 775
//! ячеек на плитку; сетка 32×24 на RAW 4080×3060 даёт плитки по ~4 100 ячеек (запас ~8×), а на наибольшем
//! поддерживаемом кадре [`MAX_SUPPORTED_MOSAIC`] — 16 384. Настройки, при которых плитка может переполниться,
//! отклоняются при проверке ([`AwbStatisticsSettings::check_for_mosaic`]), а не обрезаются молча.

use camera_contract::geometry::PixelSize;

use crate::cfa::RgbChannel;
use crate::image::{CameraMosaic, ShadingCorrectedMosaic};

/// Верхнее значение квантованного отсчёта: 12 бит (см. модуль).
pub const QUANT_MAX: u32 = 4095;

/// Тот же масштаб в f32 — множитель формулы квантования (4095 представимо в f32 точно).
pub const QUANT_SCALE: f32 = 4095.0;

/// Худшая сумма на ячейку при запасе 1,0 — у зелёного (два отсчёта по `QUANT_MAX`).
const WORST_GREEN_SUM_PER_CELL: u32 = 2 * QUANT_MAX;

/// Сколько ячеек выдерживает плитка без переполнения u32 в худшем случае при запасе 1,0 (все отсчёты = 1).
/// Для настоящего запаса — [`max_cells_per_tile`].
pub const MAX_CELLS_PER_TILE: u32 = u32::MAX / WORST_GREEN_SUM_PER_CELL;

/// Запас суммы по умолчанию: потолок квантования в долях белого сенсора. Обязан превышать наибольшее усиление
/// калибровки виньетирования (на OnePlus 11 — ×9,2 в углу камеры «2»).
pub const DEFAULT_SUM_HEADROOM: f32 = 16.0;

/// Верхняя граница `sum_headroom`, которую принимает конфиг: дальше суммы u32 не выдержали бы и одной плитки
/// осмысленного размера.
pub const MAX_SUM_HEADROOM: f32 = 256.0;

/// Наибольшая поддерживаемая мозаика (пиксели): под неё при запуске проверяется, что суммы плиток не
/// переполнятся ([`AwbStatisticsSettings::check_for_mosaic`]). Вдвое больше стороны сенсора OnePlus 11 (4096×3072).
pub const MAX_SUPPORTED_MOSAIC: PixelSize = PixelSize::new(8192, 6144);

/// Наибольшее квантованное значение одного отсчёта при запасе `headroom`: `floor(headroom · 4095 + 0,5)`.
pub fn max_quantized_sample(headroom: f32) -> u32 {
    (headroom * QUANT_SCALE + 0.5).floor() as u32
}

/// Сколько ячеек выдерживает плитка без переполнения u32 при запасе `headroom` (худший случай — все отсчёты на
/// потолке, у зелёного их два на ячейку).
pub fn max_cells_per_tile(headroom: f32) -> u32 {
    let worst_per_cell = 2 * u64::from(max_quantized_sample(headroom));
    u32::try_from(u64::from(u32::MAX) / worst_per_cell.max(1)).unwrap_or(u32::MAX)
}

/// Сетка плиток: столбцы × строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileGrid {
    pub columns: u32,
    pub rows: u32,
}

impl TileGrid {
    /// Число плиток (u64: произведение двух u32 не переполняется).
    pub const fn tile_count(self) -> u64 {
        self.columns as u64 * self.rows as u64
    }
}

/// Настройки статистики AWB (конфиг).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwbStatisticsSettings {
    /// Сетка плиток над изображением ячеек 2×2.
    pub grid: TileGrid,
    /// Порог пересвета в нормированном свете ДО баланса белого (1,0 = белый сенсора), (0, 1].
    pub saturation_threshold: f32,
    /// Потолок квантования СУММ в долях белого сенсора (после коррекции виньетирования, `[1, 256]`): значения выше
    /// потолка обрезаются. Должен превышать наибольшее усиление калибровки (×9,2 на OnePlus 11); по умолчанию 16.
    #[serde(default = "default_sum_headroom")]
    pub sum_headroom: f32,
}

fn default_sum_headroom() -> f32 {
    DEFAULT_SUM_HEADROOM
}

impl Default for AwbStatisticsSettings {
    /// - Сетка 32×24: соотношение 4:3, как у сенсора; на 4080×3060 RAW (ячейки 2040×1530) это
    ///   плитки ~64×64 ячейки — достаточно мелко, чтобы отделить небо и тень, и достаточно
    ///   крупно, чтобы шум усреднился. Уточняется в 08.
    /// - Порог 0,95: у самого белого отсчёт нелинеен (колено насыщения сенсора), а усиление
    ///   баланса белого потом сдвинет каналы — ячейку, близкую к белому хотя бы в одном
    ///   канале, считаем потерявшей цвет. Запас 5 % — оценка до реальных кадров 08.
    fn default() -> Self {
        Self {
            grid: TileGrid { columns: 32, rows: 24 },
            saturation_threshold: 0.95,
            sum_headroom: DEFAULT_SUM_HEADROOM,
        }
    }
}

/// Настройки статистики не подходят к кадру.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidAwbStatistics {
    #[error("сетка {columns}×{rows}: столбцов и строк должно быть хотя бы по одному")]
    EmptyGrid { columns: u32, rows: u32 },
    #[error("порог пересвета {threshold} должен быть конечным и лежать в (0, 1]")]
    SaturationThresholdOutOfRange { threshold: f32 },
    #[error("запас суммы {headroom} должен быть конечным и лежать в [1, {MAX_SUM_HEADROOM}]")]
    SumHeadroomOutOfRange { headroom: f32 },
    #[error("нормированная и скорректированная мозаики разного размера: {normalized:?} и {shading_corrected:?}")]
    InputSizeMismatch {
        normalized: PixelSize,
        shading_corrected: PixelSize,
    },
    #[error("сетка {columns}×{rows} мельче изображения ячеек {cells_width}×{cells_height}: были бы плитки без ячеек")]
    GridFinerThanCells {
        columns: u32,
        rows: u32,
        cells_width: u32,
        cells_height: u32,
    },
    #[error(
        "сетка {columns}×{rows} слишком крупная: плитка до {cells_per_tile} ячеек, суммы u32 \
         выдерживают {max_cells}"
    )]
    TileTooLarge {
        columns: u32,
        rows: u32,
        cells_per_tile: u64,
        max_cells: u32,
    },
    #[error("индекс плитки не помещается в u32: ячеек {cells_width}×{cells_height}, сетка {columns}×{rows}")]
    TileIndexOverflow {
        columns: u32,
        rows: u32,
        cells_width: u32,
        cells_height: u32,
    },
}

impl AwbStatisticsSettings {
    /// Проверяет настройки для мозаики размера `mosaic_size` — так же, как это нужно GPU-стороне
    /// при создании буфера статистики (до первого кадра, а не на нём).
    pub fn check_for_mosaic(&self, mosaic_size: PixelSize) -> Result<(), InvalidAwbStatistics> {
        let threshold = self.saturation_threshold;
        if !(threshold.is_finite() && threshold > 0.0 && threshold <= 1.0) {
            return Err(InvalidAwbStatistics::SaturationThresholdOutOfRange { threshold });
        }
        let headroom = self.sum_headroom;
        if !(headroom.is_finite() && (1.0..=MAX_SUM_HEADROOM).contains(&headroom)) {
            return Err(InvalidAwbStatistics::SumHeadroomOutOfRange { headroom });
        }
        let TileGrid { columns, rows } = self.grid;
        if columns == 0 || rows == 0 {
            return Err(InvalidAwbStatistics::EmptyGrid { columns, rows });
        }
        let cells = cells_size(mosaic_size);
        if columns > cells.width || rows > cells.height {
            return Err(InvalidAwbStatistics::GridFinerThanCells {
                columns,
                rows,
                cells_width: cells.width,
                cells_height: cells.height,
            });
        }
        // Шейдер считает `sx · columns` в u32 — произведение для последней ячейки обязано влезть.
        let widest_product = u64::from(cells.width - 1) * u64::from(columns);
        let tallest_product = u64::from(cells.height - 1) * u64::from(rows);
        if widest_product > u64::from(u32::MAX) || tallest_product > u64::from(u32::MAX) {
            return Err(InvalidAwbStatistics::TileIndexOverflow {
                columns,
                rows,
                cells_width: cells.width,
                cells_height: cells.height,
            });
        }
        // При делении `floor(s · n / N)` плитка шириной не больше `ceil(N / n)` ячеек.
        let cells_per_tile = u64::from(cells.width.div_ceil(columns)) * u64::from(cells.height.div_ceil(rows));
        let max_cells = max_cells_per_tile(headroom);
        if cells_per_tile > u64::from(max_cells) {
            return Err(InvalidAwbStatistics::TileTooLarge {
                columns,
                rows,
                cells_per_tile,
                max_cells,
            });
        }
        Ok(())
    }

    /// Проверка при запуске: настройки выдерживают наибольший поддерживаемый кадр ([`MAX_SUPPORTED_MOSAIC`]) —
    /// суммы u32 при худшем потолке не переполнятся ни на каком кадре. Явная ошибка, а не обрезание.
    pub fn check_for_largest_supported_frame(&self) -> Result<(), InvalidAwbStatistics> {
        self.check_for_mosaic(MAX_SUPPORTED_MOSAIC)
    }
}

/// Суммы одной плитки — пять u32 в этом порядке (`#[repr(C)]`: так же лежат в буфере GPU).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize)]
pub struct AwbTile {
    /// Сумма квантованных красных отсчётов принятых ячеек.
    pub sum_r: u32,
    /// Сумма ОБОИХ зелёных отсчётов принятых ячеек (поэтому до `2 · 4095` на ячейку).
    pub sum_g: u32,
    /// Сумма квантованных синих отсчётов принятых ячеек.
    pub sum_b: u32,
    /// Ячейки, вошедшие в суммы.
    pub used_cells: u32,
    /// Ячейки, исключённые из-за пересвета (или NaN).
    pub saturated_cells: u32,
}

/// Итог по всему кадру: u64, потому что сумма всех плиток u32 уже не помещается.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize)]
pub struct AwbTotals {
    pub sum_r: u64,
    pub sum_g: u64,
    pub sum_b: u64,
    pub used_cells: u64,
    pub saturated_cells: u64,
}

/// Число плиток не совпадает с сеткой (readback GPU не того размера).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("сетка {columns}×{rows} требует {expected} плиток, передано {actual}")]
pub struct TileCountMismatch {
    pub columns: u32,
    pub rows: u32,
    pub expected: u64,
    pub actual: u64,
}

/// Статистика кадра: плитки по строкам сетки.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AwbStatistics {
    grid: TileGrid,
    tiles: Vec<AwbTile>,
}

impl AwbStatistics {
    /// Статистика из готовых плиток (readback GPU для точной сверки с эталоном).
    pub fn from_tiles(grid: TileGrid, tiles: Vec<AwbTile>) -> Result<Self, TileCountMismatch> {
        // usize → u64 без потерь на поддерживаемых платформах (64 бита).
        let actual = tiles.len() as u64;
        if actual != grid.tile_count() {
            return Err(TileCountMismatch {
                columns: grid.columns,
                rows: grid.rows,
                expected: grid.tile_count(),
                actual,
            });
        }
        Ok(Self { grid, tiles })
    }

    pub fn grid(&self) -> TileGrid {
        self.grid
    }

    /// Все плитки по строкам: `index = row · columns + column`.
    pub fn tiles(&self) -> &[AwbTile] {
        &self.tiles
    }

    /// Плитка (column, row); `None` за пределами сетки.
    pub fn tile(&self, column: u32, row: u32) -> Option<&AwbTile> {
        if column >= self.grid.columns || row >= self.grid.rows {
            return None;
        }
        self.tiles.get(tile_index(self.grid, column, row))
    }

    /// Суммы всего кадра в u64.
    pub fn totals(&self) -> AwbTotals {
        self.tiles.iter().fold(AwbTotals::default(), |mut totals, tile| {
            totals.sum_r += u64::from(tile.sum_r);
            totals.sum_g += u64::from(tile.sum_g);
            totals.sum_b += u64::from(tile.sum_b);
            totals.used_cells += u64::from(tile.used_cells);
            totals.saturated_cells += u64::from(tile.saturated_cells);
            totals
        })
    }
}

/// Квантует отсчёт с запасом 1,0 (потолок — белый сенсора): 0…`QUANT_MAX`.
pub fn quantize_sample(normalized: f32) -> u32 {
    quantize_sample_with_headroom(normalized, 1.0)
}

/// Квантует скорректированный отсчёт: `floor(clamp(v, 0, headroom) · 4095 + 0,5)` (формула модуля, f32 как в
/// шейдере). NaN сюда не попадает: ячейку с NaN раньше исключает проверка пересвета.
pub fn quantize_sample_with_headroom(corrected: f32, headroom: f32) -> u32 {
    let scaled = corrected.clamp(0.0, headroom) * QUANT_SCALE;
    // `floor(x + 0.5)` в 0…headroom·4095 + 0,5 — целое, сужение до u32 точное (запас ≤ 256 → < 2^21).
    (scaled + 0.5).floor() as u32
}

/// Вход эталона статистики: ДВЕ мозаики одного кадра (см. модуль) — по какой судить пересвет и по какой считать суммы.
#[derive(Debug, Clone, Copy)]
pub struct AwbStatisticsInput<'a> {
    clipping_judged_on: &'a CameraMosaic,
    sums_taken_from: &'a ShadingCorrectedMosaic,
}

impl<'a> AwbStatisticsInput<'a> {
    /// `normalized` — после уровней, ДО виньетирования (по ней — пересвет); `shading_corrected` — после
    /// виньетирования (по ней — суммы). Размеры и раскладка CFA обязаны совпадать.
    pub fn new(
        normalized: &'a CameraMosaic,
        shading_corrected: &'a ShadingCorrectedMosaic,
    ) -> Result<Self, InvalidAwbStatistics> {
        if normalized.size() != shading_corrected.size() || normalized.pattern() != shading_corrected.pattern() {
            return Err(InvalidAwbStatistics::InputSizeMismatch {
                normalized: normalized.size(),
                shading_corrected: shading_corrected.size(),
            });
        }
        Ok(Self {
            clipping_judged_on: normalized,
            sums_taken_from: shading_corrected,
        })
    }
}

/// Плитка ячейки `(sx, sy)` на изображении ячеек `cells` (формула модуля, целые u32).
/// Предусловие: настройки прошли [`AwbStatisticsSettings::check_for_mosaic`], поэтому
/// произведения не переполняются, а `cells` не пустой.
pub fn tile_of_cell(cell_x: u32, cell_y: u32, cells: PixelSize, grid: TileGrid) -> (u32, u32) {
    (cell_x * grid.columns / cells.width, cell_y * grid.rows / cells.height)
}

/// Размер изображения ячеек 2×2 для мозаики.
pub const fn cells_size(mosaic_size: PixelSize) -> PixelSize {
    PixelSize::new(mosaic_size.width / 2, mosaic_size.height / 2)
}

/// Эталон: статистика AWB (контракт — см. модуль): пересвет — по нормированной мозаике до виньетирования,
/// суммы — по скорректированной до баланса белого.
pub fn compute_awb_statistics(
    input: AwbStatisticsInput<'_>,
    settings: &AwbStatisticsSettings,
) -> Result<AwbStatistics, InvalidAwbStatistics> {
    let mosaic = input.sums_taken_from;
    settings.check_for_mosaic(mosaic.size())?;
    let grid = settings.grid;
    let cells = cells_size(mosaic.size());
    // Плиток не больше, чем ячеек (проверено выше): буфер не больше самого кадра.
    // `tile_index(grid, 0, rows)` = rows · columns — ровно число плиток.
    let mut tiles = vec![AwbTile::default(); tile_index(grid, 0, grid.rows)];
    for cell_y in 0..cells.height {
        for cell_x in 0..cells.width {
            let (column, row) = tile_of_cell(cell_x, cell_y, cells, grid);
            // Индекс всегда в буфере: `sx < cells.width` ⇒ `sx·columns/cells.width < columns`
            // (то же для строк), поэтому выход за границу здесь — баг формулы, а не данных.
            let tile = &mut tiles[tile_index(grid, column, row)];
            accumulate_cell(tile, input, (cell_x, cell_y), settings);
        }
    }
    Ok(AwbStatistics { grid, tiles })
}

/// Смещения четырёх отсчётов внутри ячейки в порядке позиций квадрата 2×2 (см. [`crate::cfa`]).
const CELL_OFFSETS: [(u32, u32); 4] = [(0, 0), (1, 0), (0, 1), (1, 1)];

/// Добавляет ячейку `(cell_x, cell_y)` в плитку: пересвеченная (по НОРМИРОВАННЫМ отсчётам) только считается,
/// остальная квантуется (по СКОРРЕКТИРОВАННЫМ) в суммы своих каналов.
fn accumulate_cell(
    tile: &mut AwbTile,
    input: AwbStatisticsInput<'_>,
    (cell_x, cell_y): (u32, u32),
    settings: &AwbStatisticsSettings,
) {
    let pattern = input.sums_taken_from.pattern();
    // Отсчёт и его цвет по раскладке CFA: какой из четырёх — красный, решает только `pattern`.
    let positions = CELL_OFFSETS.map(|(offset_x, offset_y)| (cell_x * 2 + offset_x, cell_y * 2 + offset_y));
    let saturated = positions
        .iter()
        .any(|&(x, y)| is_saturated_sample(input.clipping_judged_on.sample(x, y), settings.saturation_threshold));
    if saturated {
        tile.saturated_cells += 1;
        return;
    }
    for (x, y) in positions {
        let quantized = quantize_sample_with_headroom(input.sums_taken_from.sample(x, y), settings.sum_headroom);
        match pattern.channel_at(x, y) {
            RgbChannel::Red => tile.sum_r += quantized,
            RgbChannel::Green => tile.sum_g += quantized,
            RgbChannel::Blue => tile.sum_b += quantized,
        }
    }
    tile.used_cells += 1;
}

/// Отсчёт пересвечен: `!(v < t)` (см. модуль). Через `partial_cmp`, чтобы NaN явно попадал в
/// «пересвечен», как `!(v < t)` в GLSL, а не `v >= t` (для NaN — «не пересвечен»).
fn is_saturated_sample(sample: f32, saturation_threshold: f32) -> bool {
    !matches!(
        sample.partial_cmp(&saturation_threshold),
        Some(std::cmp::Ordering::Less)
    )
}

/// Индекс плитки в буфере по строкам (u32 → usize без потерь на 64-битных платформах).
fn tile_index(grid: TileGrid, column: u32, row: u32) -> usize {
    row as usize * grid.columns as usize + column as usize
}

#[cfg(test)]
#[path = "statistics_tests.rs"]
mod tests;
