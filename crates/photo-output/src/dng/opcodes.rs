//! `OpcodeList2` с `GainMap` по плоскостям CFA: описание коррекции виньетирования, применённой к кадру.
//!
//! Решение 8 («честный RAW»): отсчёты DNG остаются нетронутыми, а поправка, которую наш ISP применил
//! при проявлении снимка, едет в файле описанием. Конвертер (Adobe, dng_validate, Darktable и др.),
//! выполнив список, проявит DNG так же ровно. Так же поступает платформенный `DngCreator`: четыре `GainMap`,
//! по одному на плоскость CFA.
//!
//! Спецификация: DNG 1.7, раздел «Opcode Lists» (формат списка и записи), «OpcodeList2» (список работает над
//! сырыми отсчётами ПОСЛЕ линеаризации, до демозаика), «GainMap» (опкод №9).
//!
//! # Формат (DNG 1.7, «Opcode Lists»)
//! Список ВСЕГДА big-endian, независимо от порядка байт TIFF (у нас файл little-endian):
//! `u32` число опкодов, затем для каждого: `u32` ID, 4 байта версии, `u32` флаги, `u32` длина параметров, параметры.
//!
//! # Параметры `GainMap` (ID 9)
//! `Top, Left, Bottom, Right` (область, в координатах записанного изображения), `Plane, Planes`, `RowPitch,
//! ColPitch`, `MapPointsV, MapPointsH` (все `u32`), `MapSpacingV, MapSpacingH, MapOriginV, MapOriginH` (`f64`,
//! в долях области), `MapPlanes` (`u32`), далее `MapPointsV × MapPointsH × MapPlanes` значений `f32`
//! по строкам: усиление, на которое умножается отсчёт.

use camera_contract::geometry::PixelSize;
use camera_contract::lens_shading::{GAIN_CHANNELS, LensShadingGainGrid};

use super::cfa::{CfaColor, CfaQuad};

/// ID опкода `GainMap` (DNG 1.7, «GainMap»).
const GAIN_MAP_OPCODE_ID: u32 = 9;
/// Версия опкода `GainMap`: 1.3.0.0 (появился в DNG 1.3).
pub const GAIN_MAP_OPCODE_VERSION: [u8; 4] = [1, 3, 0, 0];
/// Флаги опкода (DNG 1.7, «Opcode Lists»): бит 0 — «необязательный», читатель, не знающий опкода, его пропускает.
/// Так пишет и `DngCreator`: без коррекции DNG остаётся годным, просто с темными углами.
const OPTIONAL_OPCODE_FLAG: u32 = 1;
/// Шаг решётки плоскости CFA: квадрат 2×2 (`RowPitch = ColPitch = 2`).
const CFA_PITCH: u32 = 2;
/// Плоскостей в карте усилений одного опкода: одна (по одному опкоду на плоскость CFA).
const MAP_PLANES_PER_OPCODE: u32 = 1;
/// Плоскость изображения, которой касается опкод: CFA — одна плоскость 0 (`Planes = 1`).
const IMAGE_PLANE: u32 = 0;
/// Все четыре позиции квадрата 2×2 в порядке строк: (смещение по строкам, смещение по столбцам).
const CFA_PHASES: [(u32, u32); 4] = [(0, 0), (0, 1), (1, 0), (1, 1)];

/// Список нельзя построить.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OpcodeListError {
    /// Размер параметров опкода не помещается в `u32` (нереальная сетка).
    #[error("параметры GainMap не помещаются в u32")]
    ParametersTooLong,
}

/// Номер канала карты `[R, G_even, G_odd, B]` для позиции квадрата 2×2 записанного изображения.
///
/// Цвет позиции берётся из узора CFA ЗАПИСАННОГО изображения (с учётом нечётного начала полезной области),
/// а зелёный различается по чётности строки: `G_even` — зелёный чётных строк полезной области, и она же —
/// строки записанного изображения (DNG хранит ровно полезную область).
fn map_channel_for_phase(image_quad: CfaQuad, row_phase: u32, column_phase: u32) -> usize {
    match image_quad[(row_phase * 2 + column_phase) as usize] {
        CfaColor::Red => 0,
        CfaColor::Green if row_phase == 0 => 1,
        CfaColor::Green => 2,
        CfaColor::Blue => 3,
    }
}

/// `OpcodeList2` из четырёх `GainMap` (по одному на позицию квадрата 2×2) для изображения `image_size`.
///
/// `image_quad` — узор CFA записанного изображения (как в теге `CFAPattern`).
pub fn gain_map_opcode_list(
    grid: &LensShadingGainGrid,
    image_quad: CfaQuad,
    image_size: PixelSize,
) -> Result<Vec<u8>, OpcodeListError> {
    let mut list = Vec::new();
    list.extend_from_slice(&u32::try_from(CFA_PHASES.len()).unwrap_or(u32::MAX).to_be_bytes());
    for (row_phase, column_phase) in CFA_PHASES {
        let channel = map_channel_for_phase(image_quad, row_phase, column_phase);
        let parameters = gain_map_parameters(grid, channel, (row_phase, column_phase), image_size);
        let length = u32::try_from(parameters.len()).map_err(|_| OpcodeListError::ParametersTooLong)?;
        list.extend_from_slice(&GAIN_MAP_OPCODE_ID.to_be_bytes());
        list.extend_from_slice(&GAIN_MAP_OPCODE_VERSION);
        list.extend_from_slice(&OPTIONAL_OPCODE_FLAG.to_be_bytes());
        list.extend_from_slice(&length.to_be_bytes());
        list.extend_from_slice(&parameters);
    }
    Ok(list)
}

/// Параметры одного `GainMap`: область — всё изображение, шаг 2×2 от позиции `(row, column)`,
/// карта — канал `channel` сетки.
fn gain_map_parameters(
    grid: &LensShadingGainGrid,
    channel: usize,
    (row_phase, column_phase): (u32, u32),
    image_size: PixelSize,
) -> Vec<u8> {
    debug_assert!(channel < GAIN_CHANNELS, "канал карты вне [R, G_even, G_odd, B]");
    let (rows, columns) = (grid.rows(), grid.columns());
    let mut bytes = Vec::with_capacity(76 + grid.points().len() * 4);
    // Область и плоскости: Top, Left, Bottom, Right, Plane, Planes, RowPitch, ColPitch.
    for value in [
        row_phase,
        column_phase,
        image_size.height,
        image_size.width,
        IMAGE_PLANE,
        MAP_PLANES_PER_OPCODE,
        CFA_PITCH,
        CFA_PITCH,
        rows,
        columns,
    ] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    // Сетка накрывает область целиком: крайние точки — в крайних пикселях (соглашение `LENS_SHADING_MAP`
    // Camera2: шаг 1/(точек − 1) доли области), начало карты — угол области.
    for value in [1.0 / f64::from(rows - 1), 1.0 / f64::from(columns - 1), 0.0, 0.0] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&MAP_PLANES_PER_OPCODE.to_be_bytes());
    for point in grid.points() {
        bytes.extend_from_slice(&point[channel].to_be_bytes());
    }
    bytes
}
