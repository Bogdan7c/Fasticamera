//! Рендер реального RAW эталоном ISP в 16-битный PPM — посмотреть картинку глазами.
//!
//! Кадр готовит `tools/isp-reference/make_real_crops.py --full-frame-dir <каталог>` (только вне
//! репозитория). Запуск:
//!
//! ```text
//! cargo run -p isp-model --release --example render_capture -- <каталог> full-main-lamp out.ppm \
//!     [--demosaic preview|quality] [--gamut srgb|display-p3] [--scene-middle-gray 0.09] [--contrast 1.35] \
//!     [--tone-base legacy|agx|aces|filmic]
//! ```
//!
//! Это инструмент разработчика, не часть продукта: ошибки ввода — паника с понятным текстом.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "утилита разработчика"
)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::run_reference_encoded;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use support::real_capture::load_real_capture;

/// Аргументы командной строки.
struct RenderArgs {
    dir: PathBuf,
    name: String,
    output: PathBuf,
    demosaic: DemosaicMethod,
    gamut: DisplayGamut,
    scene_middle_gray: Option<f32>,
    contrast: Option<f32>,
    tone_base: ToneBaseChoice,
}

/// Основа тона с командной строки.
#[derive(Clone, Copy)]
enum ToneBaseChoice {
    Legacy,
    Agx,
    Aces,
    Filmic,
}

fn parse_args() -> RenderArgs {
    let mut positional = Vec::new();
    let mut demosaic = DemosaicMethod::Quality;
    let mut gamut = DisplayGamut::Srgb;
    let mut scene_middle_gray = None;
    let mut contrast = None;
    let mut tone_base = ToneBaseChoice::Legacy;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| panic!("у {arg} нет значения"));
        match arg.as_str() {
            "--demosaic" => {
                demosaic = match value().as_str() {
                    "preview" => DemosaicMethod::Preview,
                    "quality" => DemosaicMethod::Quality,
                    other => panic!("демозаик {other}: ожидалось preview или quality"),
                }
            }
            "--gamut" => {
                gamut = match value().as_str() {
                    "srgb" => DisplayGamut::Srgb,
                    "display-p3" => DisplayGamut::DisplayP3,
                    other => panic!("гамма {other}: ожидалось srgb или display-p3"),
                }
            }
            "--scene-middle-gray" => scene_middle_gray = Some(value().parse().expect("число")),
            "--tone-base" => {
                tone_base = match value().as_str() {
                    "legacy" => ToneBaseChoice::Legacy,
                    "agx" => ToneBaseChoice::Agx,
                    "aces" => ToneBaseChoice::Aces,
                    "filmic" => ToneBaseChoice::Filmic,
                    other => panic!("основа {other}: ожидалось legacy, agx, aces или filmic"),
                }
            }
            "--contrast" => contrast = Some(value().parse().expect("число")),
            _ => positional.push(arg),
        }
    }
    let [dir, name, output] = <[String; 3]>::try_from(positional)
        .unwrap_or_else(|_| panic!("использование: render_capture <каталог> <имя фикстуры> <out.ppm> [опции]"));
    RenderArgs {
        dir: dir.into(),
        name,
        output: output.into(),
        demosaic,
        gamut,
        scene_middle_gray,
        contrast,
        tone_base,
    }
}

/// PPM P6 с maxval 65535: отсчёты big-endian (формат netpbm).
fn write_ppm16(path: &PathBuf, width: u32, height: u32, pixels: &[[f32; 3]]) {
    const MAX_VALUE: f32 = 65535.0;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).expect("создать PPM"));
    write!(file, "P6\n{width} {height}\n65535\n").expect("заголовок PPM");
    for pixel in pixels {
        for channel in pixel {
            let value = (channel.clamp(0.0, 1.0) * MAX_VALUE).round() as u16;
            file.write_all(&value.to_be_bytes()).expect("запись PPM");
        }
    }
}

/// Средняя насыщенность HSV `(max − min)/max` по пикселям с max > 0.
fn mean_hsv_saturation(pixels: &[[f32; 3]]) -> f64 {
    let (sum, count) = pixels.iter().fold((0.0_f64, 0_u64), |(sum, count), pixel| {
        let max = pixel[0].max(pixel[1]).max(pixel[2]);
        let min = pixel[0].min(pixel[1]).min(pixel[2]);
        if max > 0.0 {
            (sum + f64::from((max - min) / max), count + 1)
        } else {
            (sum, count)
        }
    });
    sum / count.max(1) as f64
}

fn main() {
    let args = parse_args();
    let capture = load_real_capture(&args.dir, &args.name);
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    let mut controls = ProcessingControls::with_defaults(capture.neutral, args.demosaic, args.gamut);
    if let Some(gray) = args.scene_middle_gray {
        controls.tone.scene_middle_gray = gray;
    }
    if let Some(contrast) = args.contrast {
        controls.tone.contrast = contrast;
    }
    let base = match args.tone_base {
        ToneBaseChoice::Legacy => ToneBase::Legacy,
        ToneBaseChoice::Agx => ToneBase::Agx(AgxSettings::default()),
        ToneBaseChoice::Aces => ToneBase::Aces(AcesSettings::default()),
        ToneBaseChoice::Filmic => ToneBase::Filmic(FilmicSettings::default()),
    };
    let params = IspParams::resolve_with_base(&capture.sensor, &frame, &controls, &base).expect("параметры");
    let started = Instant::now();
    let encoded = run_reference_encoded(&capture.raw, &params).expect("конвейер");
    let elapsed = started.elapsed();
    let size = encoded.size();
    write_ppm16(&args.output, size.width, size.height, encoded.pixels());
    println!(
        "средняя насыщенность HSV выхода: {:.4}",
        mean_hsv_saturation(encoded.pixels())
    );
    println!(
        "{}×{} {:?} {:?}: белая точка {:?}, {:.0} K, {:?}; эталон {:.2} с",
        size.width,
        size.height,
        args.demosaic,
        args.gamut,
        params.color.white().white,
        params.color.white().temperature.0,
        params.color.white().convergence,
        elapsed.as_secs_f64(),
    );
}
