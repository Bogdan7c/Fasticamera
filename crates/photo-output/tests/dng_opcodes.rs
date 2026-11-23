//! `OpcodeList2` с `GainMap` в DNG: коррекция виньетирования, применённая к кадру, описана в файле по плоскостям
//! CFA (DNG 1.7: «Opcode Lists», «GainMap»), сырые отсчёты не тронуты, `DNGBackwardVersion` поднят только тогда.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "тесты")]

mod support;

use std::sync::Arc;

use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::lens_shading::LensShadingGainGrid;
use photo_output::dng::{DngInputs, write_dng};
use still_contract::lens_shading::{AppliedLensShading, LensShadingNotAppliedReason};
use still_contract::shot::StillFrameFacts;
use support::opcode_list::parse_gain_map_list;
use support::still_fixture::{camera_description, capture_moment, main_camera_stream, still_facts, synthetic_raw};
use support::tiff_reader::ParsedTiff;

const OPCODE_LIST_2: u16 = 51009;
const OPCODE_LIST_1: u16 = 51008;
const OPCODE_LIST_3: u16 = 51022;
const DNG_BACKWARD_VERSION: u16 = 50707;
const RAW_SIZE: PixelSize = PixelSize::new(16, 12);
const GRID_COLUMNS: u32 = 4;
const GRID_ROWS: u32 = 3;

/// Сетка 4×3, где КАЖДОЕ число уникально: `1 + точка + канал/16` (двоичные дроби, точно в f32).
/// Перепутанный канал, точка или порядок строк — другое число.
fn asymmetric_grid() -> LensShadingGainGrid {
    let points = (0..GRID_COLUMNS * GRID_ROWS)
        .map(|point| {
            let base = 1.0 + point as f32;
            [base, base + 0.0625, base + 0.125, base + 0.1875]
        })
        .collect();
    LensShadingGainGrid::new(GRID_COLUMNS, GRID_ROWS, points).unwrap()
}

fn facts_with(left: u32, top: u32, lens_shading: AppliedLensShading) -> StillFrameFacts {
    let area = PixelRect {
        left,
        top,
        size: RAW_SIZE,
    };
    let crop = PixelRect {
        left: left + 2,
        top: top + 4,
        size: PixelSize::new(10, 6),
    };
    let mut facts = still_facts(main_camera_stream(area), crop);
    facts.lens_shading = lens_shading;
    facts
}

fn dng_bytes(facts: &StillFrameFacts) -> Vec<u8> {
    let (camera, moment, raw) = (camera_description(), capture_moment(), synthetic_raw(RAW_SIZE));
    let mut bytes = Vec::new();
    write_dng(
        &DngInputs {
            facts,
            raw: &raw,
            camera: &camera,
            captured_at: &moment,
        },
        &mut bytes,
    )
    .unwrap();
    bytes
}

/// Канал карты, который должен лежать в опкоде для позиции (строка, столбец) квадрата 2×2, если
/// узор записанного изображения `quad` (по строкам): R → 0, G чётной строки → 1, G нечётной → 2, B → 3.
fn expected_channel(quad: [char; 4], row: usize, column: usize) -> usize {
    match (quad[row * 2 + column], row) {
        ('R', _) => 0,
        ('G', 0) => 1,
        ('G', _) => 2,
        ('B', _) => 3,
        other => panic!("неизвестный цвет {other:?}"),
    }
}

fn assert_four_planes_match_grid(origin: (u32, u32), image_quad: [char; 4]) {
    let grid = asymmetric_grid();
    let facts = facts_with(origin.0, origin.1, AppliedLensShading::Applied(Arc::new(grid.clone())));
    let bytes = dng_bytes(&facts);
    let tiff = ParsedTiff::open(&bytes);
    let ifd = tiff.first_ifd();

    let opcodes = parse_gain_map_list(ifd.entry(OPCODE_LIST_2).bytes());
    assert_eq!(opcodes.len(), 4, "по GainMap на каждую позицию квадрата CFA");
    for (index, opcode) in opcodes.iter().enumerate() {
        let (row, column) = (index / 2, index % 2);
        let context = format!("позиция ({row}, {column}), начало области {origin:?}");
        assert_eq!(opcode.version, [1, 3, 0, 0], "{context}: версия опкода");
        assert_eq!(opcode.flags, 1, "{context}: опкод необязательный");
        assert_eq!(
            (opcode.top, opcode.left),
            (row as u32, column as u32),
            "{context}: смещение фазы"
        );
        assert_eq!(
            (opcode.bottom, opcode.right),
            (RAW_SIZE.height, RAW_SIZE.width),
            "{context}: область до края записанного изображения"
        );
        assert_eq!((opcode.plane, opcode.planes), (0, 1), "{context}");
        assert_eq!((opcode.row_pitch, opcode.col_pitch), (2, 2), "{context}");
        assert_eq!(
            (opcode.points_vertical, opcode.points_horizontal),
            (GRID_ROWS, GRID_COLUMNS),
            "{context}: точек по вертикали и горизонтали"
        );
        assert_eq!(opcode.spacing_vertical, 1.0 / f64::from(GRID_ROWS - 1), "{context}");
        assert_eq!(
            opcode.spacing_horizontal,
            1.0 / f64::from(GRID_COLUMNS - 1),
            "{context}"
        );
        assert_eq!(
            (opcode.origin_vertical, opcode.origin_horizontal),
            (0.0, 0.0),
            "{context}"
        );
        assert_eq!(opcode.map_planes, 1, "{context}");

        let channel = expected_channel(image_quad, row, column);
        let expected: Vec<f32> = grid.points().iter().map(|point| point[channel]).collect();
        assert_eq!(
            opcode.gains, expected,
            "{context}: карта должна быть каналом {channel} сетки"
        );
    }
}

#[test]
fn applied_grid_is_written_as_four_gain_maps_one_per_cfa_plane() {
    // Область с чётным началом: узор BGGR остаётся BGGR.
    assert_four_planes_match_grid((0, 0), ['B', 'G', 'G', 'R']);
}

#[test]
fn odd_valid_area_origin_shifts_which_channel_each_plane_takes() {
    // Нечётное начало (1, 3): записанный узор — RGGB; те же фазы берут другие каналы карты.
    assert_four_planes_match_grid((1, 3), ['R', 'G', 'G', 'B']);
    // Нечётная строка, чётный столбец: GBRG→ позиции сдвигаются только по строкам.
    assert_four_planes_match_grid((0, 1), ['G', 'R', 'B', 'G']);
}

#[test]
fn opcode_list_raises_backward_version_and_leaves_raw_samples_untouched() {
    let facts = facts_with(0, 0, AppliedLensShading::Applied(Arc::new(asymmetric_grid())));
    let bytes = dng_bytes(&facts);
    let tiff = ParsedTiff::open(&bytes);
    let ifd = tiff.first_ifd();
    assert_eq!(ifd.entry(DNG_BACKWARD_VERSION).bytes(), &[1, 3, 0, 0]);
    assert_eq!(ifd.entry(OPCODE_LIST_2).type_code, 7, "OpcodeList2 — UNDEFINED");
    assert!(ifd.maybe(OPCODE_LIST_1).is_none() && ifd.maybe(OPCODE_LIST_3).is_none());
    assert_eq!(
        tiff.strip_data(&ifd),
        synthetic_raw(RAW_SIZE).le_bytes(),
        "честный RAW: отсчёты не масштабируются"
    );
}

#[test]
fn not_applied_writes_no_opcode_list_and_keeps_backward_version() {
    for reason in [
        LensShadingNotAppliedReason::NotCalibrated,
        LensShadingNotAppliedReason::DisabledByControl,
    ] {
        let facts = facts_with(0, 0, AppliedLensShading::NotApplied(reason));
        let bytes = dng_bytes(&facts);
        let tiff = ParsedTiff::open(&bytes);
        let ifd = tiff.first_ifd();
        assert!(ifd.maybe(OPCODE_LIST_2).is_none(), "{reason:?}: списка нет");
        assert_eq!(ifd.entry(DNG_BACKWARD_VERSION).bytes(), &[1, 1, 0, 0], "{reason:?}");
    }
}
