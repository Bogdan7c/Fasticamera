//! Матрицы сцен для `glass_fast_paths_exact`: формы групп стекла, материалы (по ветке быстрых путей на запись),
//! сочетания прозрачности и материализации, сетка фигур. Вынесено из теста, чтобы файлы оставались читаемыми.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "общий модуль теста; в тестах падение — способ сообщить о провале"
)]

use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, ShapeItem, StrokeStyle};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::{EdgeProfile, GlassStyle, ShadowParamsPx};
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;
use ui_kit::tokens::{GlassRole, GlassTokens};

use crate::support::{empty_list, glass_group};

/// Плотность «пикселей на pt» стилей в матрице (как у `glass_group` помощников тестов).
pub const MATRIX_PX_PER_PT: f32 = 3.0;
/// Плотность экрана 1080 px на 440 pt (продукт).
pub const PRODUCT_PX_PER_PT: f32 = 1080.0 / 440.0;

pub fn opacity(value: f32) -> Opacity {
    Opacity::new(value).unwrap()
}

// ---------------------------------------------------------------------------------------------------------------
// Матрица стекла: формы × материалы × (прозрачность, материализация).
// ---------------------------------------------------------------------------------------------------------------

/// Форма группы: как расставлены члены и какое слияние.
pub struct ShapeCase {
    pub name: &'static str,
    pub build: fn(&GlassStyle) -> GlassGroupItem,
}

pub fn rounded(width: f32, height: f32, radius: f32) -> ShapeGeometry {
    ShapeGeometry::RoundedRect {
        size: PxSize::new(width, height),
        corner: CornerSpec::continuous(radius),
    }
}

pub fn shape_cases() -> Vec<ShapeCase> {
    vec![
        ShapeCase {
            name: "одиночный скруглённый прямоугольник",
            build: |style| glass_group(style, &[(rounded(200.0, 120.0, 32.0), PxPoint::new(200.0, 150.0))], 0.0),
        },
        ShapeCase {
            name: "капсула",
            build: |style| {
                glass_group(
                    style,
                    &[(
                        ShapeGeometry::Capsule {
                            size: PxSize::new(220.0, 90.0),
                        },
                        PxPoint::new(200.0, 150.0),
                    )],
                    0.0,
                )
            },
        },
        ShapeCase {
            name: "круг",
            build: |style| {
                glass_group(
                    style,
                    &[(ShapeGeometry::Circle { diameter: 110.0 }, PxPoint::new(200.0, 150.0))],
                    0.0,
                )
            },
        },
        ShapeCase {
            name: "слияние двух (радиус 24)",
            build: |style| {
                glass_group(
                    style,
                    &[
                        (rounded(120.0, 80.0, 28.0), PxPoint::new(130.0, 150.0)),
                        (ShapeGeometry::Circle { diameter: 80.0 }, PxPoint::new(235.0, 150.0)),
                    ],
                    24.0,
                )
            },
        },
        ShapeCase {
            name: "слияние трёх (радиус 30)",
            build: |style| {
                let circle = ShapeGeometry::Circle { diameter: 90.0 };
                glass_group(
                    style,
                    &[
                        (circle, PxPoint::new(130.0, 150.0)),
                        (circle, PxPoint::new(200.0, 150.0)),
                        (circle, PxPoint::new(270.0, 165.0)),
                    ],
                    30.0,
                )
            },
        },
        ShapeCase {
            name: "два члена без слияния (радиус 0)",
            build: |style| {
                glass_group(
                    style,
                    &[
                        (rounded(120.0, 80.0, 28.0), PxPoint::new(130.0, 150.0)),
                        (ShapeGeometry::Circle { diameter: 80.0 }, PxPoint::new(235.0, 150.0)),
                    ],
                    0.0,
                )
            },
        },
        ShapeCase {
            name: "повёрнутый и растянутый прямоугольник",
            build: |style| {
                let mut group = glass_group(style, &[(rounded(160.0, 90.0, 24.0), PxPoint::new(0.0, 0.0))], 0.0);
                group.members[0].transform = Affine2::scale(1.3, 0.8)
                    .then(Affine2::rotate(0.5))
                    .then(Affine2::translate(200.0, 150.0));
                group
            },
        },
        ShapeCase {
            name: "у края экрана (часть за краем)",
            build: |style| {
                glass_group(
                    style,
                    &[
                        (rounded(160.0, 100.0, 30.0), PxPoint::new(40.0, 30.0)),
                        (rounded(120.0, 90.0, 26.0), PxPoint::new(380.0, 280.0)),
                    ],
                    0.0,
                )
            },
        },
    ]
}

/// Материал: правка стиля/параметров группы. Каждая запись нацелена на свою ветку быстрых путей.
pub struct MaterialCase {
    pub name: &'static str,
    pub apply: fn(&mut GlassGroupItem),
}

/// Пересчёт параметров группы из стиля по размеру первого члена (как делает строитель списка рисования).
pub fn restyle(group: &mut GlassGroupItem, style: &GlassStyle, px_per_pt: f32) {
    group.params = style.resolve(group.members[0].geometry.extent(), px_per_pt);
}

/// Нажатие члена: масштаб запекается в преобразование (как у `merged_pair` в `glass_parity`), точка касания локальная.
pub fn press_member(group: &mut GlassGroupItem, member: usize, amount: f32, touch_local: PxPoint) {
    let scale = group.params.press.scale_for(amount);
    let current = group.members[member].transform;
    group.members[member].transform = Affine2::uniform_scale(scale).then(current);
    group.members[member].press.amount = amount;
    group.members[member].press.touch_point_local = touch_local;
}

pub fn material_cases() -> Vec<MaterialCase> {
    vec![
        MaterialCase {
            name: "кнопка по умолчанию",
            apply: |_| {},
        },
        MaterialCase {
            name: "панель продукта (блюр 8 pt, тон, затемнение)",
            apply: |group| {
                let tokens = GlassTokens::default();
                restyle(group, tokens.style_for(GlassRole::Panel), PRODUCT_PX_PER_PT);
            },
        },
        MaterialCase {
            name: "Frost продукта (без линзы, тона, тени)",
            apply: |group| {
                let tokens = GlassTokens::default();
                restyle(group, tokens.style_for(GlassRole::Frost), PRODUCT_PX_PER_PT);
            },
        },
        MaterialCase {
            name: "чистая линза",
            apply: |group| restyle(group, &crate::support::lens_only_style(), MATRIX_PX_PER_PT),
        },
        MaterialCase {
            name: "дисперсия 0",
            apply: |group| group.params.dispersion = 0.0,
        },
        MaterialCase {
            name: "дисперсия 0,5 (предел)",
            apply: |group| group.params.dispersion = 0.5,
        },
        MaterialCase {
            name: "преломление 0",
            apply: |group| group.params.refraction_strength_px = 0.0,
        },
        MaterialCase {
            name: "преломление сильное, зона узкая",
            apply: |group| {
                group.params.refraction_strength_px = 70.0;
                group.params.lens_zone_px = 6.0;
            },
        },
        MaterialCase {
            name: "профиль «четверть окружности»",
            apply: |group| group.params.edge_profile = EdgeProfile::QuarterCircle,
        },
        MaterialCase {
            name: "резкий фон (радиус размытия 0)",
            apply: |group| group.params.blur_radius_px = 0.0,
        },
        MaterialCase {
            name: "радиус размытия 1 px (меньше 1,5, lod 0)",
            apply: |group| group.params.blur_radius_px = 1.0,
        },
        MaterialCase {
            name: "размытие: целый уровень 1 (3 px)",
            apply: |group| group.params.blur_radius_px = 3.0,
        },
        MaterialCase {
            name: "размытие: целый уровень 2 (6 px)",
            apply: |group| group.params.blur_radius_px = 6.0,
        },
        MaterialCase {
            name: "размытие: целый уровень 4 (24 px)",
            apply: |group| group.params.blur_radius_px = 24.0,
        },
        MaterialCase {
            name: "размытие: дробный уровень (10 px)",
            apply: |group| group.params.blur_radius_px = 10.0,
        },
        MaterialCase {
            name: "размытие: дробный уровень (19,64 px, как панель)",
            apply: |group| group.params.blur_radius_px = 19.636_364,
        },
        MaterialCase {
            name: "размытие: больше пирамиды (5000 px)",
            apply: |group| group.params.blur_radius_px = 5000.0,
        },
        MaterialCase {
            name: "тень выключена",
            apply: |group| group.params.shadow.alpha = 0.0,
        },
        MaterialCase {
            name: "тень с радиусом 0 (резкая)",
            apply: |group| {
                group.params.shadow = ShadowParamsPx {
                    radius_px: 0.0,
                    alpha: 0.6,
                    offset_px: 10.0,
                };
            },
        },
        MaterialCase {
            name: "тень большая и сильная",
            apply: |group| {
                group.params.shadow = ShadowParamsPx {
                    radius_px: 40.0,
                    alpha: 1.0,
                    offset_px: 20.0,
                };
            },
        },
        MaterialCase {
            name: "блик выключен",
            apply: |group| group.params.specular.alpha = 0.0,
        },
        MaterialCase {
            name: "блик: показатель 0,1 и широкий ободок",
            apply: |group| {
                group.params.specular.exponent = 0.1;
                group.params.specular.rim_width_px = 40.0;
            },
        },
        MaterialCase {
            name: "блик: показатель 16",
            apply: |group| group.params.specular.exponent = 16.0,
        },
        MaterialCase {
            name: "блик: ободок нулевой ширины",
            apply: |group| group.params.specular.rim_width_px = 0.0,
        },
        MaterialCase {
            name: "тёмный край выключен",
            apply: |group| group.params.dark_edge.strength = 0.0,
        },
        MaterialCase {
            name: "тёмный край сильный и широкий",
            apply: |group| {
                group.params.dark_edge.strength = 1.0;
                group.params.dark_edge.width_px = 50.0;
            },
        },
        MaterialCase {
            name: "без плёнки тона",
            apply: |group| group.params.tint_level = 0.0,
        },
        MaterialCase {
            name: "плёнка тона полная",
            apply: |group| {
                group.params.tint_level = 1.0;
                group.params.tint_alpha_at_full = 1.0;
            },
        },
        MaterialCase {
            name: "без затемнения",
            apply: |group| group.params.dimming.max_alpha = 0.0,
        },
        MaterialCase {
            name: "ни тона, ни затемнения (средняя яркость не нужна)",
            apply: |group| {
                group.params.tint_level = 0.0;
                group.params.dimming.max_alpha = 0.0;
            },
        },
        MaterialCase {
            name: "затемнение ступенькой (нижняя = верхняя яркость)",
            apply: |group| {
                group.params.dimming.luminance_low = 0.5;
                group.params.dimming.luminance_high = 0.5;
            },
        },
        MaterialCase {
            name: "насыщенность 1",
            apply: |group| group.params.saturation = 1.0,
        },
        MaterialCase {
            name: "насыщенность 4",
            apply: |group| group.params.saturation = 4.0,
        },
        MaterialCase {
            name: "нажатие 0,9, касание внутри",
            apply: |group| press_member(group, 0, 0.9, PxPoint::new(10.0, -8.0)),
        },
        MaterialCase {
            name: "нажатие 0,9, касание далеко снаружи",
            apply: |group| press_member(group, 0, 0.9, PxPoint::new(900.0, 900.0)),
        },
        MaterialCase {
            name: "нажатие 0,3, без свечения",
            apply: |group| {
                group.params.press.glow_alpha = 0.0;
                press_member(group, 0, 0.3, PxPoint::new(0.0, 0.0));
            },
        },
    ]
}

/// Сочетания (прозрачность группы, материализация): непрозрачно и не появляется, полупрозрачно, рампа
/// материализации (0 < effect < 0,2), ровно на границе рампы, ноль.
pub const OPACITY_EFFECT_COMBOS: [(f32, f32); 8] = [
    (1.0, 1.0),
    (0.6, 1.0),
    (1.0, 0.5),
    (1.0, 0.1),
    (1.0, 0.19),
    (1.0, 0.2),
    (0.3, 0.1),
    (1.0, 0.0),
];

pub fn glass_case_list(shape: &ShapeCase, material: &MaterialCase, combo: (f32, f32)) -> DrawList {
    let mut group = (shape.build)(&GlassStyle::default());
    (material.apply)(&mut group);
    group.opacity = opacity(combo.0);
    group.effect = combo.1;
    let mut list = empty_list();
    list.items.push(DrawItem::Glass(group));
    list
}

// ---------------------------------------------------------------------------------------------------------------
// Фигуры (`ui_shape.frag`): заливка × обводка × тень × прозрачность × геометрия × преобразование.
// ---------------------------------------------------------------------------------------------------------------

pub fn shape_geometries() -> [ShapeGeometry; 4] {
    [
        rounded(60.0, 40.0, 14.0),
        ShapeGeometry::RoundedRect {
            size: PxSize::new(56.0, 56.0),
            corner: CornerSpec::circular(10.0),
        },
        ShapeGeometry::Capsule {
            size: PxSize::new(64.0, 30.0),
        },
        ShapeGeometry::Circle { diameter: 50.0 },
    ]
}

pub fn shape_transforms(center: PxPoint) -> [Affine2; 2] {
    [
        Affine2::translate(center.x, center.y),
        Affine2::scale(1.2, 0.8)
            .then(Affine2::rotate(0.6))
            .then(Affine2::translate(center.x, center.y)),
    ]
}

/// Сетка из фигур: каждая клетка — своё сочетание заливки, обводки, тени, прозрачности.
pub fn shape_grid_lists() -> Vec<DrawList> {
    let fills = [
        DisplayColor::rgb(0.9, 0.3, 0.2),
        DisplayColor::new(0.2, 0.6, 0.9, 0.5),
        DisplayColor::new(0.1, 0.1, 0.1, 0.0),
    ];
    let strokes = [
        None,
        Some(StrokeStyle {
            width_px: 3.0,
            color: DisplayColor::rgb(1.0, 1.0, 1.0),
        }),
        Some(StrokeStyle {
            width_px: 5.0,
            color: DisplayColor::new(0.0, 0.0, 0.0, 0.6),
        }),
    ];
    let shadows = [
        None,
        Some(ShadowParamsPx {
            radius_px: 0.0,
            alpha: 0.5,
            offset_px: 6.0,
        }),
        Some(ShadowParamsPx {
            radius_px: 12.0,
            alpha: 0.8,
            offset_px: 4.0,
        }),
    ];
    let opacities = [1.0_f32, 0.5];
    let mut cells = Vec::new();
    for geometry in shape_geometries() {
        for transform_index in 0..2 {
            for fill in fills {
                for stroke in strokes {
                    for shadow in shadows {
                        for item_opacity in opacities {
                            cells.push((geometry, transform_index, fill, stroke, shadow, item_opacity));
                        }
                    }
                }
            }
        }
    }
    // 5 × 4 клеток по 80 × 75 px на кадр.
    cells
        .chunks(20)
        .map(|chunk| {
            let mut list = empty_list();
            for (cell, (geometry, transform_index, fill, stroke, shadow, item_opacity)) in chunk.iter().enumerate() {
                let center = PxPoint::new(40.0 + (cell % 5) as f32 * 80.0, 38.0 + (cell / 5) as f32 * 75.0);
                list.items.push(DrawItem::Shape(ShapeItem {
                    geometry: *geometry,
                    transform: shape_transforms(center)[*transform_index],
                    fill: *fill,
                    stroke: *stroke,
                    shadow: *shadow,
                    opacity: opacity(*item_opacity),
                }));
            }
            list
        })
        .collect()
}
