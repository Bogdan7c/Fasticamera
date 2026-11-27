//! Положение телефона и приборы: waveform и parade показывают сцену так, как её видит глаз, а вектороскоп
//! от положения не зависит. Наблюдаемое свойство: поверхность, повёрнутая как телефон, даёт ТЕ ЖЕ
//! счётчики, что и сцена «глазами» при портретном чтении.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::picture;
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::trace_rules::ParadeRule;
use scope_model::{
    HorizontalOrder, ResolvedScope, SampleCount, SamplePosition, SampleStride, ScopeCounters, ScopeModelSettings,
    ScopeRule, ShownCodes, accumulate,
};

const EYE_WIDTH: u32 = 24;
const EYE_HEIGHT: u32 = 10;

/// Сцена глазами: яркость растёт слева направо (рампа), цвет по вертикали разный — чтобы потеря оси видна.
fn eye_scene(eye_x: u32, eye_y: u32) -> [u16; 3] {
    let ramp = (eye_x * 1023 / (EYE_WIDTH - 1)) as u16;
    [ramp, (eye_y * 100) as u16, 1023 - ramp]
}

/// Поверхность экрана (портретная раскладка), на которой эта сцена лежит при положении телефона `scan`.
fn surface_for(scan: ScopeScan) -> ShownCodes {
    let (w, h) = (EYE_WIDTH, EYE_HEIGHT);
    match scan {
        ScopeScan::Upright => picture(w, h, eye_scene),
        // Сцена слева направо идёт по поверхности сверху вниз; верх сцены — справа на поверхности.
        ScopeScan::RotatedClockwise => picture(h, w, |sx, sy| eye_scene(sy, h - 1 - sx)),
        ScopeScan::UpsideDown => picture(w, h, |sx, sy| eye_scene(w - 1 - sx, h - 1 - sy)),
        // Сцена слева направо — по поверхности снизу вверх; верх сцены — слева на поверхности.
        ScopeScan::RotatedCounterClockwise => picture(h, w, |sx, sy| eye_scene(w - 1 - sy, sx)),
    }
}

fn counters_of(
    surface: &ShownCodes,
    scan: ScopeScan,
    rule_of: impl Fn(ScopeScan) -> Box<dyn ScopeRule>,
) -> ScopeCounters {
    accumulate(surface, SampleStride::EVERY_PIXEL, rule_of(scan).as_ref()).unwrap()
}

fn waveform(scan: ScopeScan) -> Box<dyn ScopeRule> {
    // RGB waveform — то же правило трёх плоскостей, что parade (решение 265): положение читается одинаково.
    // Потолок столбцов не меньше длины взгляда: столбец сетки = номер отсчёта вдоль взгляда (решение 322).
    Box::new(ParadeRule::new(EYE_WIDTH, 64).unwrap().with_scan(scan))
}

fn parade(scan: ScopeScan) -> Box<dyn ScopeRule> {
    Box::new(ParadeRule::new(EYE_WIDTH, 64).unwrap().with_scan(scan))
}

#[test]
fn upright_waveform_puts_the_ramp_along_x() {
    // Серая рампа слева направо: яркость зависит только от столбца.
    let gray = picture(EYE_WIDTH, EYE_HEIGHT, |x, _| [(x * 1023 / (EYE_WIDTH - 1)) as u16; 3]);
    let counters = counters_of(&gray, ScopeScan::Upright, waveform);
    // Левый столбец сетки — тёмный конец рампы (низ прибора), правый — светлый (верх).
    let rows_of = |column: u32| -> Vec<u32> {
        (0..64)
            .filter(|&row| counters.count(0, row, column).unwrap() > 0)
            .collect()
    };
    assert!(
        rows_of(0).iter().all(|&row| row >= 48),
        "левый столбец тёмный: {:?}",
        rows_of(0)
    );
    assert!(
        rows_of(EYE_WIDTH - 1).iter().all(|&row| row <= 15),
        "правый столбец светлый: {:?}",
        rows_of(EYE_WIDTH - 1)
    );
}

#[test]
fn every_scan_shows_the_scene_as_the_eye_sees_it() {
    for (name, rule_of) in [
        ("waveform", waveform as fn(ScopeScan) -> Box<dyn ScopeRule>),
        ("parade", parade),
    ] {
        let reference = counters_of(&surface_for(ScopeScan::Upright), ScopeScan::Upright, rule_of);
        for scan in ScopeScan::ALL {
            let counters = counters_of(&surface_for(scan), scan, rule_of);
            assert_eq!(counters, reference, "{name}: {scan:?} должен видеть ту же сцену");
        }
    }
}

#[test]
fn ignoring_the_scan_on_a_rotated_surface_would_show_a_different_picture() {
    // Негативный контроль: поворот действительно что-то меняет, тест выше не тавтология.
    for scan in [ScopeScan::RotatedClockwise, ScopeScan::RotatedCounterClockwise] {
        let reference = counters_of(&surface_for(ScopeScan::Upright), ScopeScan::Upright, waveform);
        let misread = counters_of(&surface_for(scan), ScopeScan::Upright, waveform);
        assert_ne!(misread, reference, "{scan:?} прочитан как портрет");
    }
    // Вверх ногами: рампа развёрнута зеркально, значит прочтённая как портрет она иная.
    let flipped = counters_of(&surface_for(ScopeScan::UpsideDown), ScopeScan::Upright, waveform);
    assert_ne!(
        flipped,
        counters_of(&surface_for(ScopeScan::Upright), ScopeScan::Upright, waveform)
    );
}

#[test]
fn mirror_applies_to_the_stored_picture_and_the_scan_reads_the_mirrored_result() {
    // Буфер = зеркало поверхности (фронтальная камера); показанная картинка получается разворотом строк, и
    // уже её читают по положению телефона.
    for scan in ScopeScan::ALL {
        let shown = surface_for(scan);
        let stored: Vec<[f32; 3]> = {
            let mut values = Vec::new();
            for y in 0..shown.height() {
                for x in (0..shown.width()).rev() {
                    let code = shown.pixel(x, y).unwrap();
                    values.push(code.map(|c| f32::from(c) / 1023.0));
                }
            }
            values
        };
        let rebuilt =
            ShownCodes::from_display_rgb(shown.width(), shown.height(), &stored, HorizontalOrder::Mirrored).unwrap();
        assert_eq!(rebuilt, shown, "{scan:?}: зеркало применено к буферу");
        assert_eq!(
            counters_of(&rebuilt, scan, waveform),
            counters_of(&surface_for(ScopeScan::Upright), ScopeScan::Upright, waveform),
            "{scan:?}: зеркальный буфер читается как сцена"
        );
    }
}

#[test]
fn sample_position_maps_corner_samples_of_the_surface_to_the_eye_horizontal() {
    // Поверхность 6 × 4. Левый-верх (0,0) и правый-низ (5,3).
    let samples = SampleCount::for_window(6, 4, SampleStride::EVERY_PIXEL);
    let at = |x, y, scan| SamplePosition::of_sample(x, y, samples, scan);
    assert_eq!(
        at(0, 0, ScopeScan::Upright),
        SamplePosition {
            eye_index: 0,
            eye_samples: 6
        }
    );
    assert_eq!(
        at(5, 3, ScopeScan::Upright),
        SamplePosition {
            eye_index: 5,
            eye_samples: 6
        }
    );
    assert_eq!(
        at(0, 0, ScopeScan::RotatedClockwise),
        SamplePosition {
            eye_index: 0,
            eye_samples: 4
        }
    );
    assert_eq!(
        at(5, 3, ScopeScan::RotatedClockwise),
        SamplePosition {
            eye_index: 3,
            eye_samples: 4
        }
    );
    assert_eq!(
        at(0, 0, ScopeScan::UpsideDown),
        SamplePosition {
            eye_index: 5,
            eye_samples: 6
        }
    );
    assert_eq!(
        at(5, 3, ScopeScan::UpsideDown),
        SamplePosition {
            eye_index: 0,
            eye_samples: 6
        }
    );
    assert_eq!(
        at(0, 0, ScopeScan::RotatedCounterClockwise),
        SamplePosition {
            eye_index: 3,
            eye_samples: 4
        }
    );
    assert_eq!(
        at(5, 3, ScopeScan::RotatedCounterClockwise),
        SamplePosition {
            eye_index: 0,
            eye_samples: 4
        }
    );
}

fn request(kind: ScopeKind, axes: VectorscopeAxes, scan: ScopeScan) -> ScopeRequest {
    ScopeRequest {
        kind,
        vectorscope_axes: axes,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        scan,
        image_size: ScopeImageSize::try_new(300, 200).unwrap(),
    }
}

#[test]
fn vectorscope_counters_do_not_depend_on_the_scan() {
    let settings = ScopeModelSettings::default();
    let surface = surface_for(ScopeScan::RotatedClockwise);
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let counters_for = |scan| {
            let rule = ResolvedScope::resolve(
                &request(ScopeKind::Vectorscope, axes, scan),
                &settings,
                DisplayGamut::Srgb,
            )
            .unwrap();
            accumulate(&surface, SampleStride::EVERY_PIXEL, &rule).unwrap()
        };
        let reference = counters_for(ScopeScan::Upright);
        assert!(reference.total() > 0, "вектороскоп {axes:?} что-то посчитал");
        for scan in ScopeScan::ALL {
            assert_eq!(counters_for(scan), reference, "{axes:?} {scan:?}");
        }
    }
}

#[test]
fn resolved_waveform_and_parade_carry_the_scan_of_the_request() {
    let settings = ScopeModelSettings::default();
    for kind in [ScopeKind::Waveform, ScopeKind::RgbParade] {
        for scan in ScopeScan::ALL {
            let rule = ResolvedScope::resolve(
                &request(kind, VectorscopeAxes::YCbCrBt709, scan),
                &settings,
                DisplayGamut::Srgb,
            )
            .unwrap();
            assert_eq!(rule.scan(), scan, "{kind:?}");
        }
    }
}
