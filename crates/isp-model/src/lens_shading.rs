//! Стадия 2: коррекция виньетирования по нашей статичной калибровке камеры (яркость и цвет).
//!
//! HAL OnePlus 11 корректирует виньетирование лишь частично (`LENS_SHADING_APPLIED = true`
//! по документации Camera2 — «частично»): углы RAW остаются на 2,2–3 EV темнее центра. Поэтому
//! стадия работает по собственной калибровке камеры ([`camera_contract::lens_shading`]) и от флага
//! вендора не зависит. Покадровую карту HAL не используем вовсе.
//!
//! # От калибровки к карте потока
//! Калибровка снята на сетке над АКТИВНОЙ ОБЛАСТЬЮ сенсора. Поток читает часть области
//! ([`ValidAreaOnSensor`]: 4:3 — всю, 16:9 — центральную полосу; буфер может быть меньше области
//! при биннинге). [`LensShadingGainMap::for_stream`] пересчитывает калибровку в сетку над
//! ПОЛЕЗНОЙ ОБЛАСТЬЮ БУФЕРА (в пикселях буфера):
//! * узел `(c, r)` новой сетки лежит в пикселе полезной области
//!   `vx = (Wv − 1)·c/(columns − 1)`, `vy = (Hv − 1)·r/(rows − 1)` (`Wv × Hv` — размер полезной области);
//! * центр пикселя буфера `v` — это центр пикселя активной области
//!   `a = left + (v + 0,5)·(ширина прямоугольника / Wv) − 0,5` (при совпадении размеров масштаб 1 и
//!   `a = left + v`; при биннинге ×2 центр пикселя буфера лежит между двумя пикселями области);
//!   `a` зажимается в `0 … W_active − 1`;
//! * усиление узла — билинейное значение калибровки в `a`, затем сила: `g' = 1 + (g − 1)·strength`.
//!
//! Для потока 4:3 на весь сенсор карта поэтому совпадает с калибровкой точно.
//!
//! # Эталон на отсчёт (GPU повторяет его дословно)
//! Для отсчёта в пикселе `(x, y)` полезной области `Wv × Hv`:
//! ```text
//! scale_x = (columns − 1) / (Wv − 1)           scale_y = (rows − 1) / (Hv − 1)         — f32, ОДИН раз на кадр
//! grid_x = x · scale_x                         grid_y = y · scale_y                    — f32
//!          (последний пиксель x = Wv − 1: grid_x = columns − 1 ТОЧНО, без остатка округления)
//! left = min(floor(grid_x), columns − 2)       top = min(floor(grid_y), rows − 2)
//! fx = grid_x − left                           fy = grid_y − top
//! upper = p(left, top)·(1 − fx) + p(left + 1, top)·fx
//! lower = p(left, top + 1)·(1 − fx) + p(left + 1, top + 1)·fx
//! gain  = upper·(1 − fy) + lower·fy
//! ```
//! Это выборка [`LensShadingSampling::PerSample`] (фото, качественный конвейер). Видоискатель берёт усиление
//! один раз на ячейку 2×2 ([`LensShadingSampling::PerCellCentre`]): `grid_x = (x0 + 0,5) · scale_x`,
//! `grid_y = (y0 + 0,5) · scale_y`, где `(x0, y0)` — чётный верхний левый отсчёт ячейки; смесь та же, канал —
//! свой у каждого из четырёх отсчётов.
//!
//! Канал точки — по цвету CFA в `(x, y)`: R → 0, B → 3, зелёный → `G_even` (1), если строка `y`
//! полезной области чётна, иначе `G_odd` (2). Строка считается от верха ПОЛЕЗНОЙ области, поэтому
//! её верх на сенсоре обязан быть чётным (иначе фаза CFA и чётность строки калибровки разойдутся) —
//! [`LensShadingGainMap::for_stream`] отвергает нечётный верх.

use std::sync::Arc;

use camera_contract::geometry::PixelSize;
use camera_contract::lens_shading::{GAIN_CHANNELS, LensShadingCalibration, LensShadingGainGrid, ValidAreaOnSensor};

use crate::cfa::RgbChannel;
use crate::demosaic::DemosaicMethod;
use crate::image::{CameraMosaic, ShadingCorrectedMosaic, mosaic_stage};

pub use camera_contract::lens_shading::InvalidGainGrid;

/// Допуск привязки координаты узла к целому числу: ошибка f64 на пути «буфер → сенсор → сетка»
/// порядка 1e-15, и без привязки узлы 4:3 «плыли» бы на последний бит.
const GRID_SNAP_TOLERANCE: f64 = 1e-9;

/// Сила коррекции не в диапазоне 0…1.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("сила коррекции виньетирования должна быть от 0 до 1, а не {requested}")]
pub struct InvalidCorrectionStrength {
    pub requested: f64,
}

/// Сила коррекции: 0 — без коррекции, 1 — полная (по умолчанию). В конфиге — число 0…1,
/// проверяется при разборе.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct CorrectionStrength(f32);

impl TryFrom<f64> for CorrectionStrength {
    type Error = InvalidCorrectionStrength;

    fn try_from(strength: f64) -> Result<Self, Self::Error> {
        Self::new(strength)
    }
}

impl From<CorrectionStrength> for f64 {
    fn from(strength: CorrectionStrength) -> Self {
        f64::from(strength.0)
    }
}

impl CorrectionStrength {
    /// Полная коррекция.
    pub const FULL: Self = Self(1.0);

    /// Проверяет диапазон 0…1 (NaN отвергается).
    pub fn new(strength: f64) -> Result<Self, InvalidCorrectionStrength> {
        if !(0.0..=1.0).contains(&strength) {
            return Err(InvalidCorrectionStrength { requested: strength });
        }
        Ok(Self(strength as f32))
    }

    pub fn value(self) -> f32 {
        self.0
    }

    fn is_full(self) -> bool {
        self.0 == 1.0
    }
}

impl Default for CorrectionStrength {
    fn default() -> Self {
        Self::FULL
    }
}

/// Решение «корректировать ли виньетирование» — часть управления обработкой кадра. Сила — покадровое
/// управление: политика «ослаблять при слабом свете» (бэклог) сможет менять её от кадра к кадру.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LensShadingControl {
    Disabled,
    Enabled(CorrectionStrength),
}

impl Default for LensShadingControl {
    /// По умолчанию — всегда полная коррекция (решение владельца).
    fn default() -> Self {
        Self::Enabled(CorrectionStrength::FULL)
    }
}

/// Карту для потока построить нельзя.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum StreamMapError {
    #[error("полезная область {size:?} слишком мала для карты (нужно хотя бы 2×2)")]
    ValidAreaTooSmall { size: PixelSize },
    #[error("активная область калибровки {calibration:?} не совпадает с активной областью потока {stream:?}")]
    ActiveArrayMismatch { calibration: PixelSize, stream: PixelSize },
    #[error("верх полезной области на сенсоре ({top}) нечётный: чётность строк калибровки не совпадёт с CFA буфера")]
    OddSensorTop { top: u32 },
    #[error("пересчитанная сетка невалидна: {0}")]
    InvalidGrid(#[from] InvalidGainGrid),
}

/// Карта усилений над полезной областью буфера потока (в пикселях буфера). Строится один раз на
/// поток; читают её CPU-эталон и GPU.
#[derive(Debug, Clone, PartialEq)]
pub struct LensShadingGainMap {
    grid: LensShadingGainGrid,
}

impl LensShadingGainMap {
    /// Карта из готовой сетки (над полезной областью буфера).
    pub fn from_grid(grid: LensShadingGainGrid) -> Self {
        Self { grid }
    }

    /// Карта из сырых точек с проверкой (`[R, G_even, G_odd, B]` по строкам).
    pub fn new(columns: u32, rows: u32, points: Vec<[f32; GAIN_CHANNELS]>) -> Result<Self, InvalidGainGrid> {
        LensShadingGainGrid::new(columns, rows, points).map(Self::from_grid)
    }

    pub fn columns(&self) -> u32 {
        self.grid.columns()
    }

    pub fn rows(&self) -> u32 {
        self.grid.rows()
    }

    /// Точки по строкам, в каждой `[R, G_even, G_odd, B]` (вход GPU).
    pub fn points(&self) -> &[[f32; GAIN_CHANNELS]] {
        self.grid.points()
    }

    /// Карта потока (см. модуль): калибровка, пересчитанная на полезную область буфера
    /// размера `valid_area_size`, с силой `strength`.
    pub fn for_stream(
        calibration: &LensShadingCalibration,
        sensor_area: &ValidAreaOnSensor,
        valid_area_size: PixelSize,
        strength: CorrectionStrength,
    ) -> Result<Self, StreamMapError> {
        if calibration.active_array != sensor_area.active_array {
            return Err(StreamMapError::ActiveArrayMismatch {
                calibration: calibration.active_array,
                stream: sensor_area.active_array,
            });
        }
        if valid_area_size.width < 2 || valid_area_size.height < 2 {
            return Err(StreamMapError::ValidAreaTooSmall { size: valid_area_size });
        }
        if !sensor_area.rect.top.is_multiple_of(2) {
            return Err(StreamMapError::OddSensorTop {
                top: sensor_area.rect.top,
            });
        }
        let source = &calibration.grid;
        let (columns, rows) = (source.columns(), source.rows());
        let mut points = Vec::with_capacity(columns as usize * rows as usize);
        for row in 0..rows {
            let source_y = source_grid_coordinate(
                row,
                rows,
                valid_area_size.height,
                sensor_area.rect.top,
                sensor_area.rect.size.height,
                sensor_area.active_array.height,
            );
            for column in 0..columns {
                let source_x = source_grid_coordinate(
                    column,
                    columns,
                    valid_area_size.width,
                    sensor_area.rect.left,
                    sensor_area.rect.size.width,
                    sensor_area.active_array.width,
                );
                let mut gains = [1.0_f32; GAIN_CHANNELS];
                for (channel, gain) in gains.iter_mut().enumerate() {
                    *gain = bilinear_gain(source, source_x, source_y, channel);
                }
                points.push(gains);
            }
        }
        // Билинейная смесь усилений ≥ 1 не опускается ниже 1; конструктор всё равно проверяет.
        Ok(Self::new(columns, rows, points)?.with_strength(strength))
    }

    /// Та же карта с силой `strength`: `g' = 1 + (g − 1)·strength` в каждой точке.
    pub fn with_strength(&self, strength: CorrectionStrength) -> Self {
        if strength.is_full() {
            return self.clone();
        }
        Self {
            grid: self.grid.blended_toward_unity(strength.value()),
        }
    }

    /// Масштабы «пиксель → координата узла» для полезной области `valid_area` (считаются один раз на кадр;
    /// их же GPU получает готовыми в заголовке карты, поэтому умножение даёт одинаковый бит).
    pub fn grid_scales(&self, valid_area: PixelSize) -> GridScales {
        GridScales {
            x: grid_scale(valid_area.width, self.columns()),
            y: grid_scale(valid_area.height, self.rows()),
        }
    }

    /// Усиления всех четырёх каналов карты `[R, G_even, G_odd, B]` в точке пикселя `(x, y)` полезной области:
    /// выборка та же, что у [`Self::gain_at`] (канал на положение точки не влияет). Нужна своему белому точки
    /// ([`crate::white_level`], решение 177).
    pub(crate) fn channel_gains_at(
        &self,
        position: (u32, u32),
        valid_area: PixelSize,
        scales: GridScales,
        sampling: LensShadingSampling,
    ) -> [f32; GAIN_CHANNELS] {
        std::array::from_fn(|channel| self.gain_at(position, valid_area, scales, channel, sampling))
    }

    /// Усиление для пикселя `(x, y)` полезной области `valid_area` в канале карты `map_channel`; точку на сетке
    /// выбирает `sampling` (см. [`LensShadingSampling`]).
    fn gain_at(
        &self,
        (x, y): (u32, u32),
        valid_area: PixelSize,
        scales: GridScales,
        map_channel: usize,
        sampling: LensShadingSampling,
    ) -> f32 {
        let (grid_x, grid_y) = match sampling {
            LensShadingSampling::PerSample => (
                grid_coordinate(x, valid_area.width, self.columns(), scales.x),
                grid_coordinate(y, valid_area.height, self.rows(), scales.y),
            ),
            // Ячейка 2×2 начинается с чётного пикселя полезной области; точка — центр ячейки.
            LensShadingSampling::PerCellCentre => (
                cell_centre_coordinate(x & !1, scales.x),
                cell_centre_coordinate(y & !1, scales.y),
            ),
        };
        bilinear_gain(&self.grid, grid_x, grid_y, map_channel)
    }
}

/// В какой точке полезной области берётся усиление для отсчёта.
///
/// Видоискатель (демозаик `Preview`, суперпиксель 2×2) отдаёт ОДИН пиксель на ячейку, поэтому усиление
/// берётся один раз на ячейку — в её центре: это в четыре раза дешевле на GPU (одна билинейная выборка
/// вместо четырёх; замер на телефоне показал, что покадровый вариант не укладывается в бюджет кадра).
/// Разница с покадровой выборкой — доли процента (центр ячейки отстоит от отсчёта на полпикселя, усиление
/// гладкое; тест `tests/lens_shading_real_flat_field.rs` меряет её на настоящей калибровке). Фото и качественный
/// конвейер (полное разрешение, оффлайн) остаются на выборке по каждому отсчёту — полная точность.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensShadingSampling {
    /// Усиление в точке самого отсчёта `(x, y)`.
    PerSample,
    /// Усиление в центре ячейки 2×2 `(x0 + 0,5; y0 + 0,5)` (`x0`, `y0` — чётный верхний левый отсчёт ячейки),
    /// одно на все четыре отсчёта (канал карты у каждого свой).
    PerCellCentre,
}

impl LensShadingSampling {
    /// Выборка эталонного конвейера для демозаика: `Preview` — по ячейке, `Quality` — по отсчёту.
    pub fn for_demosaic(method: DemosaicMethod) -> Self {
        match method {
            DemosaicMethod::Preview => Self::PerCellCentre,
            DemosaicMethod::Quality => Self::PerSample,
        }
    }
}

/// Масштабы пикселя в координату узла по осям (см. модуль: `(узлов − 1) / (пикселей − 1)` в f32).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridScales {
    /// По горизонтали.
    pub x: f32,
    /// По вертикали.
    pub y: f32,
}

/// Координата узла `index` (из `points` узлов) новой сетки на исходной сетке калибровки.
fn source_grid_coordinate(
    index: u32,
    points: u32,
    valid_extent: u32,
    sensor_start: u32,
    sensor_extent: u32,
    active_extent: u32,
) -> f32 {
    // Положение узла в пикселях полезной области буфера.
    let valid_position = f64::from(valid_extent - 1) * f64::from(index) / f64::from(points - 1);
    // Центр пикселя буфера → центр пикселя активной области (см. модуль).
    let scale = f64::from(sensor_extent) / f64::from(valid_extent);
    let sensor_position =
        (f64::from(sensor_start) + (valid_position + 0.5) * scale - 0.5).clamp(0.0, f64::from(active_extent - 1));
    let grid_position = sensor_position / f64::from(active_extent - 1) * f64::from(points - 1);
    let nearest = grid_position.round();
    let snapped = if (grid_position - nearest).abs() < GRID_SNAP_TOLERANCE {
        nearest
    } else {
        grid_position
    };
    snapped as f32
}

/// Билинейное значение канала в точке сетки `(grid_x, grid_y)` — формула эталона из доки модуля.
fn bilinear_gain(grid: &LensShadingGainGrid, grid_x: f32, grid_y: f32, map_channel: usize) -> f32 {
    let (columns, rows) = (grid.columns(), grid.rows());
    let left = (grid_x.floor() as u32).min(columns - 2);
    let top = (grid_y.floor() as u32).min(rows - 2);
    let fraction_x = grid_x - left as f32;
    let fraction_y = grid_y - top as f32;
    let points = grid.points();
    let point = |column: u32, row: u32| points[(row * columns + column) as usize][map_channel];
    let upper = point(left, top) * (1.0 - fraction_x) + point(left + 1, top) * fraction_x;
    let lower = point(left, top + 1) * (1.0 - fraction_x) + point(left + 1, top + 1) * fraction_x;
    upper * (1.0 - fraction_y) + lower * fraction_y
}

/// Масштаб `(nodes − 1) / (extent − 1)` в f32 (одно деление с правильным округлением на CPU).
/// Для протяжённости < 2 масштаб 0: координата всегда 0.
fn grid_scale(extent: u32, nodes: u32) -> f32 {
    if extent <= 1 {
        return 0.0;
    }
    (nodes - 1) as f32 / (extent - 1) as f32
}

/// Координата центра ячейки 2×2 с чётным верхним левым отсчётом `cell_origin`: `(cell_origin + 0,5) · scale`.
/// Сумма точна в f32 (индекс < 2^23); центр ячейки не доходит до последнего узла ни при каком масштабе, но
/// ограничение `min(floor, nodes − 2)` в `bilinear_gain` остаётся.
fn cell_centre_coordinate(cell_origin: u32, scale: f32) -> f32 {
    (cell_origin as f32 + 0.5) * scale
}

/// Координата пикселя на сетке из `nodes` узлов: `pixel · scale`; последний пиксель — ровно последний узел.
/// Без явного случая `(extent − 1) · scale` мог бы выйти на 1 ULP за последний узел или не дойти до него.
/// Умножение f32 — одна операция с правильным округлением и на CPU, и в SPIR-V (`OpFMul` с `NoContraction`:
/// Vulkan 1.3, раздел «Precision and Operation of SPIR-V Instructions», умножение — не хуже правильного
/// округления IEEE 754), поэтому GPU повторяет координату бит в бит.
fn grid_coordinate(pixel: u32, extent: u32, nodes: u32, scale: f32) -> f32 {
    if extent <= 1 {
        return 0.0;
    }
    if pixel + 1 >= extent {
        return (nodes - 1) as f32;
    }
    pixel as f32 * scale
}

/// Канал точки карты для цвета мозаики в строке `y` полезной области.
fn map_channel(channel: RgbChannel, y: u32) -> usize {
    match channel {
        RgbChannel::Red => 0,
        // G_even — зелёный чётных строк, G_odd — нечётных.
        RgbChannel::Green if y.is_multiple_of(2) => 1,
        RgbChannel::Green => 2,
        RgbChannel::Blue => 3,
    }
}

/// Почему стадия пропущена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensShadingSkipReason {
    /// У потока нет нашей калибровки: углы останутся тёмными.
    NotCalibrated,
    /// Коррекцию выключило управление (конфиг или пользователь).
    DisabledByControl,
}

/// Решение стадии на кадр. Карта общая (`Arc`): клонирование параметров кадра дёшево.
#[derive(Debug, Clone, PartialEq)]
pub enum LensShadingStep {
    Skip(LensShadingSkipReason),
    Apply(Arc<LensShadingGainMap>),
}

impl LensShadingStep {
    /// Решает по карте потока (полной силы; `None` — калибровки нет) и управлению.
    ///
    /// Выключение управлением важнее отсутствия калибровки: явное «выключено» не должно
    /// выглядеть как поломка. При полной силе карта общая с потоком (тот же `Arc`), при другой
    /// строится новая.
    pub fn decide(stream_map: Option<&Arc<LensShadingGainMap>>, control: LensShadingControl) -> Self {
        match (control, stream_map) {
            (LensShadingControl::Disabled, _) => Self::Skip(LensShadingSkipReason::DisabledByControl),
            (LensShadingControl::Enabled(_), None) => Self::Skip(LensShadingSkipReason::NotCalibrated),
            (LensShadingControl::Enabled(strength), Some(map)) if strength.is_full() => Self::Apply(Arc::clone(map)),
            (LensShadingControl::Enabled(strength), Some(map)) => Self::Apply(Arc::new(map.with_strength(strength))),
        }
    }
}

/// Что стадия сделала с кадром (для диагностики и паритета).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensShadingOutcome {
    Skipped(LensShadingSkipReason),
    Applied,
}

/// Где мозаика лежит внутри полезной области: кадр целиком или вырезка (кроп видоискателя,
/// полоса фото).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropInValidArea {
    left: u32,
    top: u32,
    valid_area_size: PixelSize,
}

/// Вырезка не годится для проверки против эталона.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidCrop {
    #[error("начало вырезки ({left}, {top}) нечётное: фаза CFA вырезки разойдётся с полезной областью")]
    OddOrigin { left: u32, top: u32 },
    #[error("вырезка {size:?} с началом ({left}, {top}) выходит за полезную область {valid_area_size:?}")]
    OutsideValidArea {
        left: u32,
        top: u32,
        size: PixelSize,
        valid_area_size: PixelSize,
    },
}

impl CropInValidArea {
    /// Размер полезной области, внутри которой лежит вырезка.
    pub(crate) fn valid_area_size(&self) -> PixelSize {
        self.valid_area_size
    }

    /// Отсчёт `(x, y)` вырезки → пиксель полезной области (карта усилений задана над ней).
    pub(crate) fn valid_area_position(self, x: u32, y: u32) -> (u32, u32) {
        (self.left + x, self.top + y)
    }

    /// Мозаика — вся полезная область (начало в нуле).
    pub fn whole(valid_area_size: PixelSize) -> Self {
        Self {
            left: 0,
            top: 0,
            valid_area_size,
        }
    }

    /// Вырезка с началом `(left, top)` внутри полезной области `valid_area_size`; начало чётное.
    pub fn at(left: u32, top: u32, valid_area_size: PixelSize) -> Result<Self, InvalidCrop> {
        if !left.is_multiple_of(2) || !top.is_multiple_of(2) {
            return Err(InvalidCrop::OddOrigin { left, top });
        }
        Ok(Self {
            left,
            top,
            valid_area_size,
        })
    }
}

/// Стадия 2 для кадра целиком: `pixel · gain(x, y, канал)` или пропуск с причиной.
pub fn apply_lens_shading(
    mosaic: &CameraMosaic,
    step: &LensShadingStep,
) -> (ShadingCorrectedMosaic, LensShadingOutcome) {
    apply_lens_shading_with(mosaic, step, LensShadingSampling::PerSample)
}

/// Стадия 2 для кадра целиком с явным выбором точки выборки усиления.
pub fn apply_lens_shading_with(
    mosaic: &CameraMosaic,
    step: &LensShadingStep,
    sampling: LensShadingSampling,
) -> (ShadingCorrectedMosaic, LensShadingOutcome) {
    apply_to_region(mosaic, step, CropInValidArea::whole(mosaic.size()), sampling)
}

/// Стадия 2 для вырезки полезной области: отсчёт `(x, y)` мозаики — это пиксель
/// `(left + x, top + y)` полезной области. Результат совпадает бит в бит с соответствующим
/// фрагментом применения ко всему кадру.
pub fn apply_lens_shading_at(
    mosaic: &CameraMosaic,
    step: &LensShadingStep,
    crop: CropInValidArea,
) -> Result<(ShadingCorrectedMosaic, LensShadingOutcome), InvalidCrop> {
    apply_lens_shading_sampled(mosaic, step, crop, LensShadingSampling::PerSample)
}

/// То же, с явным выбором точки выборки усиления (видоискатель — [`LensShadingSampling::PerCellCentre`]).
pub fn apply_lens_shading_sampled(
    mosaic: &CameraMosaic,
    step: &LensShadingStep,
    crop: CropInValidArea,
    sampling: LensShadingSampling,
) -> Result<(ShadingCorrectedMosaic, LensShadingOutcome), InvalidCrop> {
    let size = mosaic.size();
    let fits = u64::from(crop.left) + u64::from(size.width) <= u64::from(crop.valid_area_size.width)
        && u64::from(crop.top) + u64::from(size.height) <= u64::from(crop.valid_area_size.height);
    if !fits {
        return Err(InvalidCrop::OutsideValidArea {
            left: crop.left,
            top: crop.top,
            size,
            valid_area_size: crop.valid_area_size,
        });
    }
    Ok(apply_to_region(mosaic, step, crop, sampling))
}

fn apply_to_region(
    mosaic: &CameraMosaic,
    step: &LensShadingStep,
    crop: CropInValidArea,
    sampling: LensShadingSampling,
) -> (ShadingCorrectedMosaic, LensShadingOutcome) {
    match step {
        LensShadingStep::Skip(reason) => (
            mosaic.map_into(mosaic_stage::ShadingCorrected, |_, _, sample| sample),
            LensShadingOutcome::Skipped(*reason),
        ),
        LensShadingStep::Apply(map) => {
            let pattern = mosaic.pattern();
            let valid = crop.valid_area_size;
            let scales = map.grid_scales(valid);
            let corrected = mosaic.map_into(mosaic_stage::ShadingCorrected, |x, y, sample| {
                // Координаты в полезной области; цвет — по месту в мозаике вырезки (начало чётное).
                let (valid_x, valid_y) = (crop.left + x, crop.top + y);
                let channel = map_channel(pattern.channel_at(x, y), valid_y);
                sample * map.gain_at((valid_x, valid_y), valid, scales, channel, sampling)
            });
            (corrected, LensShadingOutcome::Applied)
        }
    }
}

#[cfg(test)]
#[path = "lens_shading_tests.rs"]
mod tests;
