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
    assert_eq!(IconId::ALL.len(), 20);
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
    // Автовспышка («молния с A») обязана отличаться от обычной молнии: иначе режим не читается.
    let auto = IconTexels::of(IconId::FlashAuto);
    let auto_differing = (0..on.height)
        .flat_map(|row| (0..on.width).map(move |column| (column, row)))
        .filter(|&(column, row)| on.at(column, row) != auto.at(column, row))
        .count();
    assert!(
        auto_differing > 200,
        "автовспышка и вспышка включена выглядят одинаково: различий {auto_differing}"
    );
}

/// Доля текселей ячейки, лежащих внутри формы (значение SDF < 128).
fn inside_fraction(id: IconId) -> f64 {
    let texels = IconTexels::of(id);
    let inside = (0..texels.height)
        .flat_map(|row| (0..texels.width).map(move |column| (column, row)))
        .filter(|&(column, row)| texels.at(column, row) < 128)
        .count();
    inside as f64 / (texels.width * texels.height) as f64
}

/// Иконки экрана камеры v1 (сессия 12) запеклись с реальным покрытием, а не пустой/залитой ячейкой.
#[test]
fn camera_screen_icons_have_real_coverage() {
    for id in [
        IconId::Timer,
        IconId::AspectRatio,
        IconId::Close,
        IconId::Reset,
        IconId::ControlsMenu,
    ] {
        let fraction = inside_fraction(id);
        assert!(
            (0.02..0.60).contains(&fraction),
            "{id:?}: покрытие {fraction:.3} вне разумного диапазона"
        );
    }
}

/// Геометрия новых иконок: крест «×» и шесть точек «⋮⋮» — внутри в нужных местах, снаружи в пустых.
#[test]
fn close_and_controls_menu_have_expected_shapes() {
    let manifest = support::load_manifest();
    let spread = manifest.atlas.spread_texels as f64;
    let scale = f64::from(manifest.atlas.icon_texels) / 256.0; // Phosphor: viewBox 256
    let cell = |unit: f64| (spread + unit * scale).floor() as usize;

    // Close: центр пересечения диагоналей (128,128) внутри, середины сторон (128,40) и (40,128) — снаружи.
    let close = IconTexels::of(IconId::Close);
    assert!(close.at(cell(128.0), cell(128.0)) < 128, "центр креста");
    assert!(close.at(cell(128.0), cell(40.0)) > 128, "над крестом");
    assert!(close.at(cell(40.0), cell(128.0)) > 128, "слева от креста");

    // ControlsMenu (dots-six): центр верхней-левой точки (60,92) внутри, центр иконки (94,128)
    // между рядами и столбцами — снаружи.
    let dots = IconTexels::of(IconId::ControlsMenu);
    assert!(dots.at(cell(60.0), cell(92.0)) < 128, "левая верхняя точка");
    assert!(dots.at(cell(196.0), cell(164.0)) < 128, "правая нижняя точка");
    assert!(dots.at(cell(94.0), cell(128.0)) > 128, "между точками");
}

/// Своя иконка `Level` (сессия 12, пакет P1g): горизонт с разрывом в центре и короткие боковые штрихи.
/// Проверяем форму, а не «что-то запеклось»: линия есть слева и справа, в разрыве пусто, штрихи на краях.
#[test]
fn level_icon_is_a_horizon_with_a_central_gap_and_side_ticks() {
    let manifest = support::load_manifest();
    let spread = manifest.atlas.spread_texels as f64;
    let scale = f64::from(manifest.atlas.icon_texels) / 24.0; // своя иконка: viewBox 24
    let cell = |unit: f64| (spread + unit * scale).floor() as usize;

    let level = IconTexels::of(IconId::Level);
    assert!(level.at(cell(7.5), cell(12.0)) < 128, "левый отрезок горизонта");
    assert!(level.at(cell(16.5), cell(12.0)) < 128, "правый отрезок горизонта");
    assert!(level.at(cell(12.0), cell(12.0)) > 128, "разрыв в центре");
    assert!(level.at(cell(2.0), cell(10.0)) < 128, "левый боковой штрих");
    assert!(level.at(cell(22.0), cell(14.0)) < 128, "правый боковой штрих");
    assert!(level.at(cell(12.0), cell(4.0)) > 128, "над горизонтом пусто");
    let fraction = inside_fraction(IconId::Level);
    assert!(
        (0.02..0.60).contains(&fraction),
        "покрытие {fraction:.3} вне разумного диапазона"
    );
}

/// Своя иконка `ZoomMarkerTriangle` (маркер колеса зума): треугольник основанием вверх, вершиной вниз.
/// Проверяем форму: широко наверху, узко внизу, пусто по бокам внизу.
#[test]
fn zoom_marker_triangle_is_wide_at_the_top_and_narrow_at_the_bottom() {
    let manifest = support::load_manifest();
    let spread = manifest.atlas.spread_texels as f64;
    let scale = f64::from(manifest.atlas.icon_texels) / 24.0; // своя иконка: viewBox 24
    let cell = |unit: f64| (spread + unit * scale).floor() as usize;

    let marker = IconTexels::of(IconId::ZoomMarkerTriangle);
    assert!(marker.at(cell(3.0), cell(1.5)) < 128, "левая часть основания");
    assert!(marker.at(cell(21.0), cell(1.5)) < 128, "правая часть основания");
    assert!(marker.at(cell(12.0), cell(12.0)) < 128, "середина тела");
    assert!(
        marker.at(cell(3.0), cell(20.0)) > 128,
        "слева внизу пусто (вершина узкая)"
    );
    assert!(marker.at(cell(21.0), cell(20.0)) > 128, "справа внизу пусто");
    let fraction = inside_fraction(IconId::ZoomMarkerTriangle);
    assert!(
        (0.35..0.65).contains(&fraction),
        "треугольник — около половины квадрата: {fraction:.3}"
    );
}

/// Иконки панели белого (план white-balance, сессия 02): все шесть запеклись с реальным покрытием, а `sun` —
/// отдельная ячейка с теми же текселями, что и `Exposure` (тот же SVG), поэтому иконки могут разойтись позже.
#[test]
fn white_panel_icons_have_real_coverage_and_sun_matches_exposure() {
    for id in [
        IconId::DeviceMobile,
        IconId::Sun,
        IconId::Cloud,
        IconId::Tree,
        IconId::Lightbulb,
        IconId::House,
    ] {
        let fraction = inside_fraction(id);
        assert!(
            (0.02..0.60).contains(&fraction),
            "{id:?}: покрытие {fraction:.3} вне разумного диапазона"
        );
    }
    let sun = IconTexels::of(IconId::Sun);
    let exposure = IconTexels::of(IconId::Exposure);
    assert_eq!(
        (sun.width, sun.height),
        (exposure.width, exposure.height),
        "один SVG — одинаковый размер ячейки"
    );
    for row in 0..sun.height {
        for column in 0..sun.width {
            assert_eq!(sun.at(column, row), exposure.at(column, row), "({column}, {row})");
        }
    }
    assert_ne!(
        icon(IconId::Sun).region,
        icon(IconId::Exposure).region,
        "у `sun` своя ячейка атласа, а не ссылка на ячейку Exposure"
    );
    // Форма: у телефона вырез экрана — центр иконки внутри корпуса не пустой полностью, лампа и дом различимы.
    let phone = IconTexels::of(IconId::DeviceMobile);
    assert!(
        phone.at(phone.width / 2, phone.height / 2) > 128,
        "внутри корпуса телефона — полость экрана"
    );
    let house = IconTexels::of(IconId::House);
    let bulb = IconTexels::of(IconId::Lightbulb);
    let differing = (0..house.height)
        .flat_map(|row| (0..house.width).map(move |column| (column, row)))
        .filter(|(column, row)| (house.at(*column, *row) < 128) != (bulb.at(*column, *row) < 128))
        .count();
    assert!(differing > 100, "дом и лампа — разные формы: {differing}");
}

/// Атлас после добавления иконок остаётся в пределах, которые Vulkan гарантирует любому устройству
/// (`maxImageDimension2D` не меньше 4096): иначе загрузка текстуры на части телефонов провалится.
#[test]
fn atlas_stays_within_the_texture_side_every_vulkan_device_guarantees() {
    const GUARANTEED_MAX_IMAGE_DIMENSION: u32 = 4096;
    let atlas = sdf_atlas();
    assert!(
        atlas.width <= GUARANTEED_MAX_IMAGE_DIMENSION && atlas.height <= GUARANTEED_MAX_IMAGE_DIMENSION,
        "атлас {}x{} выше гарантированного предела {GUARANTEED_MAX_IMAGE_DIMENSION}",
        atlas.width,
        atlas.height
    );
}
