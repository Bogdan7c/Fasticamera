//! Равенство списков рисования — основа решения «кадр не изменился, рисовать не нужно» (решение 214, сессия 16a).
//!
//! Поток рендера сравнивает список нового кадра с тем, что уже показан: совпал — тик не рисуется. Поэтому каждое
//! поле, от которого зависят пиксели, обязано менять результат сравнения, а одинаковые списки — быть равными.
//! Тест на каждый вид элемента меняет по одному полю и проверяет, что список перестал быть равным исходному.

use crate::color::{DisplayColor, Opacity};
use crate::draw_list::{
    DrawItem, DrawList, GlassGroupItem, GlassMember, IconItem, ImageClip, ImageFit, ImageItem, PressState, ShapeItem,
    StrokeStyle, TextItem,
};
use crate::geometry::{PxPoint, PxSize};
use crate::glass::{GlassStyle, ShadowParamsPx};
use crate::icons::IconId;
use crate::light::LightDirection;
use crate::picture::{PictureEncoding, PictureSize, UiPicture};
use crate::shape::ShapeGeometry;
use crate::text::TextRun;
use crate::transform::Affine2;

fn color(red: f32) -> DisplayColor {
    DisplayColor {
        r: red,
        g: 0.5,
        b: 0.25,
        a: 1.0,
    }
}

fn list_of(item: DrawItem) -> DrawList {
    let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
    list.items.push(item);
    list
}

/// Исходный список равен своей копии и перестаёт быть равным при каждой из правок `changes`.
fn assert_every_change_is_noticed(base: DrawItem, changes: Vec<(&str, DrawItem)>) {
    let base_list = list_of(base);
    assert_eq!(base_list, base_list.clone(), "копия списка равна исходному");
    for (what, changed) in changes {
        assert_ne!(
            base_list,
            list_of(changed),
            "изменение «{what}» не замечено сравнением списков"
        );
    }
}

fn shape() -> ShapeItem {
    ShapeItem {
        geometry: ShapeGeometry::Circle { diameter: 40.0 },
        transform: Affine2::translate(10.0, 20.0),
        fill: color(0.9),
        stroke: Some(StrokeStyle {
            width_px: 2.0,
            color: color(0.1),
        }),
        shadow: Some(ShadowParamsPx {
            radius_px: 4.0,
            alpha: 0.3,
            offset_px: 2.0,
        }),
        opacity: Opacity::OPAQUE,
    }
}

#[test]
fn every_shape_field_that_changes_pixels_changes_the_comparison() {
    let half = Opacity::new(0.5).expect("прозрачность");
    let changes = vec![
        (
            "размер круга",
            ShapeItem {
                geometry: ShapeGeometry::Circle { diameter: 41.0 },
                ..shape()
            },
        ),
        (
            "вид фигуры",
            ShapeItem {
                geometry: ShapeGeometry::Capsule {
                    size: PxSize::new(40.0, 40.0),
                },
                ..shape()
            },
        ),
        (
            "положение",
            ShapeItem {
                transform: Affine2::translate(10.0, 21.0),
                ..shape()
            },
        ),
        (
            "поворот",
            ShapeItem {
                transform: Affine2::rotate(0.1),
                ..shape()
            },
        ),
        (
            "заливка",
            ShapeItem {
                fill: color(0.8),
                ..shape()
            },
        ),
        (
            "обводка снята",
            ShapeItem {
                stroke: None,
                ..shape()
            },
        ),
        (
            "толщина обводки",
            ShapeItem {
                stroke: Some(StrokeStyle {
                    width_px: 3.0,
                    color: color(0.1),
                }),
                ..shape()
            },
        ),
        (
            "цвет обводки",
            ShapeItem {
                stroke: Some(StrokeStyle {
                    width_px: 2.0,
                    color: color(0.2),
                }),
                ..shape()
            },
        ),
        (
            "тень снята",
            ShapeItem {
                shadow: None,
                ..shape()
            },
        ),
        (
            "радиус тени",
            ShapeItem {
                shadow: Some(ShadowParamsPx {
                    radius_px: 5.0,
                    alpha: 0.3,
                    offset_px: 2.0,
                }),
                ..shape()
            },
        ),
        (
            "прозрачность",
            ShapeItem {
                opacity: half,
                ..shape()
            },
        ),
    ];
    assert_every_change_is_noticed(
        DrawItem::Shape(shape()),
        changes
            .into_iter()
            .map(|(what, item)| (what, DrawItem::Shape(item)))
            .collect(),
    );
}

fn icon() -> IconItem {
    IconItem {
        icon: IconId::ALL[0],
        transform: Affine2::translate(5.0, 6.0),
        size_px: PxSize::new(24.0, 24.0),
        color: color(1.0),
        opacity: Opacity::OPAQUE,
    }
}

#[test]
fn every_icon_field_that_changes_pixels_changes_the_comparison() {
    let mut changes = vec![
        (
            "положение",
            IconItem {
                transform: Affine2::translate(5.0, 7.0),
                ..icon()
            },
        ),
        (
            "размер",
            IconItem {
                size_px: PxSize::new(25.0, 24.0),
                ..icon()
            },
        ),
        (
            "цвет",
            IconItem {
                color: color(0.9),
                ..icon()
            },
        ),
        (
            "прозрачность",
            IconItem {
                opacity: Opacity::new(0.4).expect("прозрачность"),
                ..icon()
            },
        ),
    ];
    if IconId::ALL.len() > 1 {
        changes.push((
            "другая иконка",
            IconItem {
                icon: IconId::ALL[1],
                ..icon()
            },
        ));
    }
    assert_every_change_is_noticed(
        DrawItem::Icon(icon()),
        changes
            .into_iter()
            .map(|(what, item)| (what, DrawItem::Icon(item)))
            .collect(),
    );
}

fn picture(width: u32) -> UiPicture {
    let size = PictureSize::new(width, 2);
    let bytes = vec![200_u8; usize::try_from(width * 2 * 4).expect("размер")];
    UiPicture::from_rgba8(size, PictureEncoding::Srgb, bytes).expect("картинка")
}

fn image(source: UiPicture) -> ImageItem {
    ImageItem {
        source: source.into(),
        transform: Affine2::translate(1.0, 2.0),
        size_px: PxSize::new(30.0, 20.0),
        fit: ImageFit::Cover,
        clip: ImageClip::Circle,
        opacity: Opacity::OPAQUE,
    }
}

#[test]
fn every_image_field_that_changes_pixels_changes_the_comparison() {
    let shown = picture(4);
    let base = image(shown.clone());
    let changes = vec![
        // Те же пиксели, но новая версия картинки (новая миниатюра снимка): сравнение идёт по версии, не по байтам.
        ("новая версия картинки", image(picture(4))),
        (
            "живая картинка вместо снимка",
            ImageItem {
                source: crate::draw_list::LiveImage::ColorScope.into(),
                ..image(shown.clone())
            },
        ),
        (
            "положение",
            ImageItem {
                transform: Affine2::translate(1.0, 3.0),
                ..image(shown.clone())
            },
        ),
        (
            "размер рамки",
            ImageItem {
                size_px: PxSize::new(30.0, 21.0),
                ..image(shown.clone())
            },
        ),
        (
            "вписывание",
            ImageItem {
                fit: ImageFit::Stretch,
                ..image(shown.clone())
            },
        ),
        (
            "форма обрезки",
            ImageItem {
                clip: ImageClip::None,
                ..image(shown.clone())
            },
        ),
        (
            "прозрачность",
            ImageItem {
                opacity: Opacity::new(0.7).expect("прозрачность"),
                ..image(shown.clone())
            },
        ),
    ];
    assert_every_change_is_noticed(
        DrawItem::Image(base),
        changes
            .into_iter()
            .map(|(what, item)| (what, DrawItem::Image(item)))
            .collect(),
    );
    // Та же картинка (один номер версии) в другом экземпляре элемента — равна: байты не сравниваются.
    assert_eq!(
        list_of(DrawItem::Image(image(shown.clone()))),
        list_of(DrawItem::Image(image(shown)))
    );
}

fn text() -> TextItem {
    TextItem {
        run: TextRun {
            glyphs: Vec::new(),
            advance_px: 50.0,
            cap_height_px: 10.0,
        },
        transform: Affine2::translate(3.0, 4.0),
        color: color(1.0),
        opacity: Opacity::OPAQUE,
    }
}

#[test]
fn every_text_field_that_changes_pixels_changes_the_comparison() {
    let changes = vec![
        (
            "ширина строки",
            TextItem {
                run: TextRun {
                    advance_px: 51.0,
                    ..text().run
                },
                ..text()
            },
        ),
        (
            "высота заглавных",
            TextItem {
                run: TextRun {
                    cap_height_px: 11.0,
                    ..text().run
                },
                ..text()
            },
        ),
        (
            "положение",
            TextItem {
                transform: Affine2::translate(3.0, 5.0),
                ..text()
            },
        ),
        (
            "цвет",
            TextItem {
                color: color(0.5),
                ..text()
            },
        ),
        (
            "прозрачность",
            TextItem {
                opacity: Opacity::new(0.6).expect("прозрачность"),
                ..text()
            },
        ),
    ];
    assert_every_change_is_noticed(
        DrawItem::Text(text()),
        changes
            .into_iter()
            .map(|(what, item)| (what, DrawItem::Text(item)))
            .collect(),
    );
}

fn glass() -> GlassGroupItem {
    let size = PxSize::new(80.0, 40.0);
    GlassGroupItem {
        params: GlassStyle::default().resolve(size, 2.0),
        members: vec![GlassMember {
            geometry: ShapeGeometry::Capsule { size },
            transform: Affine2::translate(100.0, 200.0),
            press: PressState::RELEASED,
        }],
        merge_radius_px: 8.0,
        effect: 1.0,
        opacity: Opacity::OPAQUE,
    }
}

#[test]
fn every_glass_field_that_changes_pixels_changes_the_comparison() {
    let base = glass();
    let mut blurrier = glass();
    blurrier.params.blur_radius_px += 1.0;
    let mut stronger = glass();
    stronger.params.refraction_strength_px += 1.0;
    let mut moved = glass();
    moved.members[0].transform = Affine2::translate(100.0, 201.0);
    let mut resized = glass();
    resized.members[0].geometry = ShapeGeometry::Capsule {
        size: PxSize::new(81.0, 40.0),
    };
    let mut pressed = glass();
    pressed.members[0].press = PressState {
        amount: 0.5,
        touch_point_local: PxPoint::ORIGIN,
    };
    let mut touched_elsewhere = glass();
    touched_elsewhere.members[0].press = PressState {
        amount: 0.0,
        touch_point_local: PxPoint { x: 3.0, y: 0.0 },
    };
    let mut second_member = glass();
    second_member.members.push(second_member.members[0]);
    let mut merged = glass();
    merged.merge_radius_px = 9.0;
    let mut faded_in = glass();
    faded_in.effect = 0.5;
    let mut translucent = glass();
    translucent.opacity = Opacity::new(0.5).expect("прозрачность");

    let changes = vec![
        ("радиус размытия", blurrier),
        ("сила преломления", stronger),
        ("положение формы", moved),
        ("размер формы", resized),
        ("нажатие формы", pressed),
        ("точка касания", touched_elsewhere),
        ("вторая форма в группе", second_member),
        ("радиус слияния", merged),
        ("материализация", faded_in),
        ("прозрачность группы", translucent),
    ];
    assert_every_change_is_noticed(
        DrawItem::Glass(base),
        changes
            .into_iter()
            .map(|(what, item)| (what, DrawItem::Glass(item)))
            .collect(),
    );
}

#[test]
fn light_order_and_count_of_items_change_the_comparison() {
    let first = DrawItem::Shape(shape());
    let second = DrawItem::Shape(ShapeItem {
        fill: color(0.2),
        ..shape()
    });
    let mut base = DrawList::new(LightDirection::from_angle_degrees(45.0));
    base.items = vec![first.clone(), second.clone()];
    assert_eq!(base, base.clone());

    let mut other_light = base.clone();
    other_light.light = LightDirection::from_angle_degrees(46.0);
    assert_ne!(base, other_light, "блик от наклона телефона");

    let mut swapped = base.clone();
    swapped.items = vec![second.clone(), first.clone()];
    assert_ne!(base, swapped, "порядок рисования: последний сверху");

    let mut fewer = base.clone();
    fewer.items.pop();
    assert_ne!(base, fewer, "элемент пропал");

    let mut more = base.clone();
    more.items.push(first);
    assert_ne!(base, more, "элемент появился");
}

#[test]
fn a_list_with_a_not_a_number_is_never_equal_to_itself_so_it_is_always_redrawn() {
    // Безопасное направление ошибки: нечисловое значение делает сравнение «не равно», а не «равно», тик рисуется.
    let broken = list_of(DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::Circle { diameter: f32::NAN },
        ..shape()
    }));
    assert_ne!(broken, broken.clone());
}

/// Замер цены сравнения на экране калибровки (4 группы стекла, 118 фигур, 28 строк, 11 иконок, картинка).
/// Худший случай — равные списки (сравнение проходит весь список). Запуск:
/// `cargo test -p ui-draw --release --lib -- --ignored --nocapture comparison_cost`.
#[test]
#[ignore = "замер цены, а не проверка: запускать вручную в release с --nocapture"]
fn comparison_cost_of_a_calibration_sized_list() {
    use crate::atlas::AtlasRegion;
    use crate::geometry::PxRect;
    use crate::text::PlacedGlyph;

    let glyph = PlacedGlyph {
        region: AtlasRegion {
            uv_min: [0.1, 0.1],
            uv_max: [0.2, 0.2],
        },
        quad: PxRect {
            origin: PxPoint::new(0.0, -8.0),
            size: PxSize::new(8.0, 10.0),
        },
    };
    let mut items = Vec::new();
    for _ in 0..4 {
        items.push(DrawItem::Glass(glass()));
    }
    for index in 0..118 {
        items.push(DrawItem::Shape(ShapeItem {
            transform: Affine2::translate(index as f32, 3.0),
            ..shape()
        }));
    }
    for index in 0..28 {
        items.push(DrawItem::Text(TextItem {
            run: TextRun {
                glyphs: vec![glyph.clone(); 10],
                advance_px: 80.0,
                cap_height_px: 10.0,
            },
            transform: Affine2::translate(index as f32, 5.0),
            ..text()
        }));
    }
    for _ in 0..11 {
        items.push(DrawItem::Icon(icon()));
    }
    items.push(DrawItem::Image(image(picture(4))));
    let mut first = DrawList::new(LightDirection::from_angle_degrees(45.0));
    first.items = items;
    let second = first.clone();

    const ROUNDS: u32 = 100_000;
    let started = std::time::Instant::now();
    let mut equal = 0_u32;
    for _ in 0..ROUNDS {
        if std::hint::black_box(&first) == std::hint::black_box(&second) {
            equal += 1;
        }
    }
    let per_comparison = started.elapsed() / ROUNDS;
    assert_eq!(equal, ROUNDS);
    println!(
        "сравнение равных списков ({} элементов): {} нс",
        first.items.len(),
        per_comparison.as_nanos()
    );
}
