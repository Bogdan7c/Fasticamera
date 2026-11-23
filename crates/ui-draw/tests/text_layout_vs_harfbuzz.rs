//! Наша вёрстка строки против полной вёрстки HarfBuzz (`harfrust`) на подписях камеры (решение 87).
//!
//! Шейпинг здесь написан независимо от кода запекания: свои переменные оси, свои возможности OpenType,
//! свой перевод единиц шрифта в пиксели.

// Это тест: вспомогательные функции падают через expect/panic, как и сами проверки.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use harfrust::{Buffer, Feature, Font, ShapeOptions, ShaperFont, shape};
use std::str::FromStr;
use ui_draw::text::{Figures, FontFace, TextError, TextStyle, layout_line};

/// Подписи камеры из задания; «WAV» нужна ради кернинга.
const CAMERA_LABELS: [&str; 23] = [
    // Имена пресетов владельца — по-русски (кириллица в атласе с 14b): кернинг пар тоже сверяется.
    "Закат",
    "Ёлка Утро",
    "PHOTO",
    "VIDEO",
    "AUTO",
    "AE/AF LOCK",
    "0.5\u{d7}",
    "1\u{d7}",
    "2",
    "+0.7",
    "\u{2212}1.3",
    "ISO 400",
    "1/125",
    "HEIF",
    "RAW",
    "Live",
    "Off",
    "3s",
    "10s",
    "4:3",
    "16:9",
    "1:1",
    "WAV",
];

/// 20 pt × 2,4545 px/pt (`assets.md`).
const SIZE_PX: f32 = 49.1;
const TOLERANCE_PX: f32 = 1e-3;

/// Положение глифа по HarfBuzz: символ и пиксельная позиция пера.
struct ReferenceGlyph {
    character: char,
    pen_px: f32,
}

/// Эталон: полная вёрстка HarfBuzz; возвращает глифы и полную ширину в px.
fn harfbuzz_reference(face: FontFace, figures: Figures, text: &str) -> (Vec<ReferenceGlyph>, f32) {
    let manifest = support::load_manifest();
    let font_bytes = support::load_font_bytes(&manifest);
    let spec = &manifest.faces[FontFace::ALL
        .iter()
        .position(|candidate| *candidate == face)
        .expect("начертание")];
    let default_font = Font::new(font_bytes, 0).expect("harfrust открывает шрифт");
    let font = default_font
        .instance_builder()
        .variations(spec.axes.iter().map(|(tag, value)| (tag.as_str(), *value)))
        .build();
    let mut features: Vec<Feature> = manifest
        .disabled_features
        .iter()
        .map(|tag| Feature::from_str(&format!("-{tag}")).expect("возможность разбирается"))
        .collect();
    if figures == Figures::Tabular {
        features.push(Feature::from_str("tnum").expect("tnum разбирается"));
    }
    let shaper_font = ShaperFont::new(&font);
    let mut buffer = Buffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    shape(&shaper_font, &mut buffer, ShapeOptions::new().features(&features)).expect("шейпинг");

    let units_to_px = SIZE_PX / f32::from(font.units_per_em());
    let mut pen_units = 0;
    let mut glyphs = Vec::new();
    for (info, position) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
        glyphs.push(ReferenceGlyph {
            character: text[info.cluster as usize..]
                .chars()
                .next()
                .expect("кластер указывает на символ"),
            pen_px: (pen_units + position.x_offset) as f32 * units_to_px,
        });
        pen_units += position.x_advance;
    }
    (glyphs, pen_units as f32 * units_to_px)
}

fn style(face: FontFace, figures: Figures) -> TextStyle {
    // Трекинг 0: эталон HarfBuzz трекинга не знает; вёрстку с трекингом проверяет отдельный тест.
    TextStyle {
        face,
        size_px: SIZE_PX,
        tracking_em: 0.0,
        figures,
    }
}

/// Смещение квадрата глифа от пера: берём из вёрстки одиночного символа, где перо в нуле.
fn quad_offset_from_pen(face: FontFace, figures: Figures, character: char) -> f32 {
    let single = layout_line(&character.to_string(), &style(face, figures)).expect("символ в наборе");
    single.glyphs[0].quad.origin.x
}

#[test]
fn camera_labels_match_harfbuzz_width_and_every_glyph_position() {
    for face in FontFace::ALL {
        for figures in [Figures::Proportional, Figures::Tabular] {
            for label in CAMERA_LABELS {
                let run = layout_line(label, &style(face, figures)).expect("подпись верстается");
                let (reference, reference_width) = harfbuzz_reference(face, figures, label);

                assert!(
                    (run.advance_px - reference_width).abs() < TOLERANCE_PX,
                    "{face:?}/{figures:?} «{label}»: ширина {} против HarfBuzz {reference_width}",
                    run.advance_px
                );
                let visible: Vec<&ReferenceGlyph> = reference.iter().filter(|glyph| glyph.character != ' ').collect();
                assert_eq!(run.glyphs.len(), visible.len(), "{face:?} «{label}»: число глифов");
                for (placed, expected) in run.glyphs.iter().zip(visible) {
                    let offset = quad_offset_from_pen(face, figures, expected.character);
                    let pen = placed.quad.origin.x - offset;
                    assert!(
                        (pen - expected.pen_px).abs() < TOLERANCE_PX,
                        "{face:?}/{figures:?} «{label}»: глиф {:?} стоит на {pen}, HarfBuzz — {}",
                        expected.character,
                        expected.pen_px
                    );
                }
            }
        }
    }
}

#[test]
fn kerning_pairs_are_really_applied() {
    for face in FontFace::ALL {
        let advance = |text: &str| {
            layout_line(text, &style(face, Figures::Proportional))
                .expect("в наборе")
                .advance_px
        };
        for pair in ["AV", "WA", "To"] {
            let separate: f32 = pair.chars().map(|ch| advance(&ch.to_string())).sum();
            assert!(
                advance(pair) < separate - 0.5,
                "{face:?} «{pair}»: {} не уже суммы продвижений {separate}",
                advance(pair)
            );
        }
        // Пара без кернинга не меняется: «H» и «I» стоят по продвижениям.
        let separate = advance("H") + advance("I");
        assert!((advance("HI") - separate).abs() < TOLERANCE_PX);
    }
}

#[test]
fn tabular_digits_share_one_width_and_proportional_digits_do_not() {
    for face in FontFace::ALL {
        let widths = |figures| -> Vec<f32> {
            ('0'..='9')
                .map(|digit| {
                    layout_line(&digit.to_string(), &style(face, figures))
                        .expect("цифра в наборе")
                        .advance_px
                })
                .collect()
        };
        let tabular = widths(Figures::Tabular);
        assert!(
            tabular.iter().all(|width| (width - tabular[0]).abs() < TOLERANCE_PX),
            "{face:?}: {tabular:?}"
        );
        let proportional = widths(Figures::Proportional);
        let (narrowest, widest) = proportional.iter().fold((f32::MAX, f32::MIN), |(low, high), width| {
            (low.min(*width), high.max(*width))
        });
        assert!(
            widest - narrowest > 1.0,
            "{face:?}: пропорциональные цифры одной ширины {proportional:?}"
        );
    }
}

#[test]
fn unsupported_characters_are_reported_with_their_character_index() {
    let style = style(FontFace::Regular, Figures::Proportional);
    assert_eq!(
        layout_line("\u{65e5}", &style),
        Err(TextError::UnsupportedCharacter {
            ch: '\u{65e5}',
            index: 0
        })
    );
    // Эмодзи занимает 4 байта, но это один символ: индекс считается в символах.
    assert_eq!(
        layout_line("AB \u{1F600}x", &style),
        Err(TextError::UnsupportedCharacter {
            ch: '\u{1F600}',
            index: 3
        })
    );
    assert!(layout_line("", &style).is_ok_and(|run| run.glyphs.is_empty() && run.advance_px == 0.0));
}

#[test]
fn tracking_adds_space_after_every_glyph_and_cap_height_matches_the_font() {
    let plain = layout_line("PHOTO", &style(FontFace::Semibold, Figures::Proportional)).expect("верстается");
    let tracked_style = TextStyle {
        tracking_em: 0.05,
        ..style(FontFace::Semibold, Figures::Proportional)
    };
    let tracked = layout_line("PHOTO", &tracked_style).expect("верстается");
    let extra = 5.0 * 0.05 * SIZE_PX;
    assert!((tracked.advance_px - plain.advance_px - extra).abs() < TOLERANCE_PX);
    // Второй глиф сдвинут ровно на один трекинг относительно нетрекованной вёрстки.
    let shift = tracked.glyphs[1].quad.origin.x - plain.glyphs[1].quad.origin.x;
    assert!((shift - 0.05 * SIZE_PX).abs() < TOLERANCE_PX, "{shift}");

    // Высота заглавных — из OS/2 шрифта, независимо считанная skrifa.
    let manifest = support::load_manifest();
    let bytes = support::load_font_bytes(&manifest);
    let font = skrifa::FontRef::new(&bytes).expect("skrifa открывает шрифт");
    let spec = &manifest.faces[1];
    let location =
        skrifa::MetadataProvider::axes(&font).location(spec.axes.iter().map(|(tag, value)| (tag.as_str(), *value)));
    let metrics = skrifa::MetadataProvider::metrics(&font, skrifa::instance::Size::unscaled(), &location);
    let expected = metrics.cap_height.expect("sCapHeight") / f32::from(metrics.units_per_em) * SIZE_PX;
    assert!(
        (plain.cap_height_px - expected).abs() < TOLERANCE_PX,
        "{} против {expected}",
        plain.cap_height_px
    );
    assert!(plain.cap_height_px > 0.6 * SIZE_PX * 0.7 && plain.cap_height_px < SIZE_PX);
}

#[test]
fn space_has_an_advance_but_no_glyph() {
    let run = layout_line("A A", &style(FontFace::Regular, Figures::Proportional)).expect("верстается");
    assert_eq!(run.glyphs.len(), 2);
    let gap = run.glyphs[1].quad.origin.x - run.glyphs[0].quad.origin.x;
    let single = layout_line("A", &style(FontFace::Regular, Figures::Proportional)).expect("верстается");
    assert!(
        gap > single.advance_px,
        "между «A» стоит пробел: {gap} против {}",
        single.advance_px
    );
}
