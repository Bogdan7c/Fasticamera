//! Иконки в атласе: у каждой есть непустая область, штрих сетки и заливка даёт ожидаемые значения SDF.

// Это тест: вспомогательные функции падают через expect/panic, как и сами проверки.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use ui_draw::atlas::sdf_atlas;
use ui_draw::icons::{IconId, icon};

/// Доступ к байтам области иконки: (столбец, строка) внутри ячейки.
struct IconTexels {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}

impl IconTexels {
    fn of(id: IconId) -> Self {
        let atlas = sdf_atlas();
        let region = icon(id).region;
        let (atlas_width, atlas_height) = (atlas.width as f32, atlas.height as f32);
        Self {
            left: (region.uv_min[0] * atlas_width).round() as usize,
            top: (region.uv_min[1] * atlas_height).round() as usize,
            width: ((region.uv_max[0] - region.uv_min[0]) * atlas_width).round() as usize,
            height: ((region.uv_max[1] - region.uv_min[1]) * atlas_height).round() as usize,
        }
    }

    fn at(&self, column: usize, row: usize) -> u8 {
        sdf_atlas().texels[(self.top + row) * sdf_atlas().width as usize + self.left + column]
    }

    fn any_inside(&self) -> bool {
        (0..self.height).any(|row| (0..self.width).any(|column| self.at(column, row) < 128))
    }

    fn any_outside(&self) -> bool {
        (0..self.height).any(|row| (0..self.width).any(|column| self.at(column, row) > 128))
    }
}

#[test]
fn every_icon_has_a_non_empty_region_with_both_inside_and_outside_texels() {
    assert_eq!(IconId::ALL.len(), 6);
    for id in IconId::ALL {
        let glyph = icon(id);
        assert!(
            glyph.region.uv_min[0] >= 0.0 && glyph.region.uv_max[0] <= 1.0,
            "{id:?}: u вне 0..1"
        );
        assert!(
            glyph.region.uv_min[1] >= 0.0 && glyph.region.uv_max[1] <= 1.0,
            "{id:?}: v вне 0..1"
        );
        assert!(glyph.region.uv_max[0] > glyph.region.uv_min[0] && glyph.region.uv_max[1] > glyph.region.uv_min[1]);
        let texels = IconTexels::of(id);
        assert!(texels.any_inside(), "{id:?}: в иконке нет внутренних текселей");
        assert!(texels.any_outside(), "{id:?}: в иконке нет внешних текселей");
        // Ячейка квадратная; рамка в долях квадрата viewBox выступает на поле расстояний.
        assert_eq!(texels.width, texels.height);
        assert!(glyph.quad_in_square.origin.x < 0.0 && glyph.quad_in_square.max_x() > 1.0);
    }
}

#[test]
fn stroke_grid_is_inside_on_the_lines_and_outside_between_them() {
    let manifest = support::load_manifest();
    let spread = manifest.atlas.spread_texels as f64;
    let scale = f64::from(manifest.atlas.icon_texels) / 24.0;
    let texels = IconTexels::of(IconId::Grid);
    // Координаты viewBox → ячейка: масштаб и сдвиг на поле расстояний.
    let cell = |unit: f64| (spread + unit * scale).floor() as usize;

    // Горизонталь y = 9 и вертикаль x = 9 (линии шириной 2): точки на них — внутри штриха.
    assert!(
        texels.at(cell(12.0), cell(9.0)) < 128,
        "на горизонтали y=9 между вертикалями"
    );
    assert!(
        texels.at(cell(9.0), cell(12.0)) < 128,
        "на вертикали x=9 между горизонталями"
    );
    // Центр средней клетки (12, 12) — между линиями: снаружи штриха.
    assert!(texels.at(cell(12.0), cell(12.0)) > 128, "середина клетки сетки");
    // Глубоко в штрихе значение ближе к 128 - (полуширина штриха), клетка — заметно выше 128.
    assert!(
        texels.at(cell(12.0), cell(12.0)) > 200,
        "в центре клетки далеко от линий"
    );
    // Левая граница ячейки далеко от формы.
    assert_eq!(texels.at(0, texels.height / 2), 255);
}

#[test]
fn filled_sun_has_a_hollow_center_and_a_solid_ring() {
    let texels = IconTexels::of(IconId::Exposure);
    let center = texels.width / 2;
    // Phosphor «sun»: кольцо радиусом 48–64 из 256 → 12–16 текселей при 64 текселях иконки.
    assert!(texels.at(center, center) > 128, "центр солнца — полость кольца");
    assert!(texels.at(center + 14, center) < 128, "середина кольца — внутри формы");
}

#[test]
fn symmetric_chevron_is_mirrored_and_flash_icons_differ() {
    let chevron = IconTexels::of(IconId::MenuChevron);
    for row in 0..chevron.height {
        for column in 0..chevron.width / 2 {
            let mirrored = chevron.width - 1 - column;
            let difference = i32::from(chevron.at(column, row)) - i32::from(chevron.at(mirrored, row));
            assert!(
                difference.abs() <= 4,
                "шеврон несимметричен в ({column}, {row}): {difference}"
            );
        }
    }
    let on = IconTexels::of(IconId::FlashOn);
    let off = IconTexels::of(IconId::FlashOff);
    let differing = (0..on.height)
        .flat_map(|row| (0..on.width).map(move |column| (column, row)))
        .filter(|&(column, row)| on.at(column, row) != off.at(column, row))
        .count();
    assert!(
        differing > 200,
        "вспышка включена и выключена выглядят одинаково: различий {differing}"
    );
}
