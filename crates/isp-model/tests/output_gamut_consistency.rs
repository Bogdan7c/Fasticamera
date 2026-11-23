//! Решение 176: «обычные цвета на экране (Display P3) и в файле (sRGB) совпадают».
//!
//! Паритеты сверяют выход P3 и выход sRGB каждый со своим эталоном, но не друг с другом. Здесь один и тот же свет
//! сцены проходит основу тона дважды — с профилем sRGB и с профилем Display P3, — оба выхода переводятся
//! (линейный свет своих основных → XYZ D65 → Lab) и сравниваются по ΔE2000. Проверяется ТОЛЬКО основа тона
//! (вход в `W` → основа → `W → цвета выхода` → мягкое сжатие охвата выхода → обрезка): слой вида нейтрален
//! и стоит после основы, 8-битное округление в расчёт не входит.
//!
//! Модели: AgX, ACES, filmic (совпадение обещано) и прежняя кривая (`Legacy`: справка, совпадение не обещано —
//! она работает в цветах выхода, поэтому охват входит в саму кривую).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "интеграционный тест: падение — способ сообщить о провале, таблица печатается для владельца"
)]

use isp_model::color::chromaticity::Primaries;
use isp_model::color::matrix::Matrix3;
use isp_model::color::working_space::WORKING_PRIMARIES;
use isp_model::gamut::{GamutCompression, GamutCompressionSettings};
use isp_model::image::DisplayGamut;
use isp_model::metrics::{DisplayToLab, delta_e_2000};
use isp_model::tone::aces::{AcesBase, AcesSettings};
use isp_model::tone::agx::AgxBase;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::filmic::{FilmicBase, FilmicSettings};
use isp_model::tone::working_stage::WorkingToneBase;
use isp_model::tone::{ToneCurve, ToneCurveSettings};

/// Экспозиции сцены относительно среднего серого, ступени.
const EXPOSURES_EV: [f64; 3] = [-2.0, 0.0, 2.0];

/// Порог «обычных» цветов: ΔE2000 ≤ 0,5. Разность 1,0 — едва заметная (JND) для пары рядом на ровном поле;
/// 0,5 — вдвое ниже: невидимо даже в сравнении бок о бок.
const ORDINARY_DELTA_E_LIMIT: f64 = 0.5;

/// Допуск на «канал вышел за 1», когда решаем, сработало ли сжатие охвата (округление f32 матриц).
const GAMUT_EDGE_TOLERANCE: f64 = 1e-4;

/// Тип входа (для таблицы и для порогов).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    ColorChecker,
    SaturationRamp,
    GrayScale,
}

/// Один вход: свет в линейном sRGB при `e = 0` (до масштаба экспозиции).
struct SceneInput {
    kind: InputKind,
    name: String,
    /// Линейный sRGB, уже нормированный так, что 0,18 серого = средний серый (масштаб применяется позже).
    linear_srgb_at_grey: [f64; 3],
}

fn srgb_decode(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn from_srgb8(red: f64, green: f64, blue: f64) -> [f64; 3] {
    [red, green, blue].map(|channel| srgb_decode(channel / 255.0))
}

fn luminance(color: [f64; 3]) -> f64 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

/// 24 патча ColorChecker Classic (2005), sRGB 8 бит по публикации BabelColor / X-Rite. Отражение — линейный
/// sRGB патча: серый патч 5 (122, 122, 121) ≈ 0,19 — «средний серый» сцены.
fn color_checker() -> Vec<SceneInput> {
    const PATCHES: [(&str, [f64; 3]); 24] = [
        ("dark skin", [115.0, 82.0, 68.0]),
        ("light skin", [194.0, 150.0, 130.0]),
        ("blue sky", [98.0, 122.0, 157.0]),
        ("foliage", [87.0, 108.0, 67.0]),
        ("blue flower", [133.0, 128.0, 177.0]),
        ("bluish green", [103.0, 189.0, 170.0]),
        ("orange", [214.0, 126.0, 44.0]),
        ("purplish blue", [80.0, 91.0, 166.0]),
        ("moderate red", [193.0, 90.0, 99.0]),
        ("purple", [94.0, 60.0, 108.0]),
        ("yellow green", [157.0, 188.0, 64.0]),
        ("orange yellow", [224.0, 163.0, 46.0]),
        ("blue", [56.0, 61.0, 150.0]),
        ("green", [70.0, 148.0, 73.0]),
        ("red", [175.0, 54.0, 60.0]),
        ("yellow", [231.0, 199.0, 31.0]),
        ("magenta", [187.0, 86.0, 149.0]),
        ("cyan", [8.0, 133.0, 161.0]),
        ("white", [243.0, 243.0, 242.0]),
        ("neutral 8", [200.0, 200.0, 200.0]),
        ("neutral 6.5", [160.0, 160.0, 160.0]),
        ("neutral 5", [122.0, 122.0, 121.0]),
        ("neutral 3.5", [85.0, 85.0, 85.0]),
        ("black", [52.0, 52.0, 52.0]),
    ];
    PATCHES
        .iter()
        .map(|(name, rgb)| SceneInput {
            kind: InputKind::ColorChecker,
            name: format!("CC {name}"),
            linear_srgb_at_grey: from_srgb8(rgb[0], rgb[1], rgb[2]),
        })
        .collect()
}

/// Рампа насыщенности: 6 оттенков (вершины куба sRGB), смесь с серым той же яркости, чистота 25…90 %.
fn saturation_ramp() -> Vec<SceneInput> {
    const HUES: [(&str, [f64; 3]); 6] = [
        ("R", [1.0, 0.0, 0.0]),
        ("Y", [1.0, 1.0, 0.0]),
        ("G", [0.0, 1.0, 0.0]),
        ("C", [0.0, 1.0, 1.0]),
        ("B", [0.0, 0.0, 1.0]),
        ("M", [1.0, 0.0, 1.0]),
    ];
    const PURITIES: [f64; 4] = [0.25, 0.5, 0.75, 0.9];
    let mut inputs = Vec::new();
    for (name, vertex) in HUES {
        let vertex_luminance = luminance(vertex);
        for purity in PURITIES {
            // Яркость пятна равна яркости среднего серого 0,18 при любой чистоте: меняется только цвет.
            let color = vertex.map(|channel| 0.18 * (purity * channel / vertex_luminance + (1.0 - purity)));
            inputs.push(SceneInput {
                kind: InputKind::SaturationRamp,
                name: format!("ramp {name} {:.0}%", purity * 100.0),
                linear_srgb_at_grey: color,
            });
        }
    }
    inputs
}

/// Серая шкала: отражения 2…90 %.
fn gray_scale() -> Vec<SceneInput> {
    [0.02, 0.05, 0.09, 0.18, 0.36, 0.6, 0.9]
        .iter()
        .map(|&level| SceneInput {
            kind: InputKind::GrayScale,
            name: format!("gray {:.0}%", level * 100.0),
            linear_srgb_at_grey: [level; 3],
        })
        .collect()
}

fn all_inputs() -> Vec<SceneInput> {
    let mut inputs = color_checker();
    inputs.extend(saturation_ramp());
    inputs.extend(gray_scale());
    inputs
}

/// Модель основы, собранная под ОДИН профиль выхода: свет сцены (линейный sRGB) → линейный свет выхода + признаки.
trait ModelUnderTest {
    fn name(&self) -> &'static str;
    /// Свет сцены в линейном sRGB (уже с экспозицией) → (выход 0…1 в основных цветах гаммы, сработало ли сжатие
    /// охвата выхода или подгонка filmic в этой гамме).
    fn render(&self, gamut: DisplayGamut, scene_linear_srgb: [f64; 3]) -> Rendered;
    /// Серый сцены, который модель переводит в средний серый экрана.
    fn scene_middle_gray(&self) -> f64;
}

struct Rendered {
    output: [f32; 3],
    /// Выходное сжатие охвата изменило цвет (или обрезка отрезала часть).
    output_gamut_engaged: bool,
    /// Основа дала разный свет в `W` для двух гамм выхода (у filmic — `fit_to_output_gamut`, стоит до слоя вида).
    base_depends_on_gamut: bool,
}

/// Модели в `W` (Rec. 2020): AgX, ACES, filmic — общий контракт [`WorkingToneBase`].
struct WorkingModel {
    name: &'static str,
    middle_gray: f64,
    for_srgb: Box<dyn WorkingToneBase>,
    for_p3: Box<dyn WorkingToneBase>,
}

impl WorkingModel {
    fn base_for(&self, gamut: DisplayGamut) -> &dyn WorkingToneBase {
        match gamut {
            DisplayGamut::Srgb => self.for_srgb.as_ref(),
            DisplayGamut::DisplayP3 => self.for_p3.as_ref(),
        }
    }
}

/// Матрица `A → B` по основным цветам (общий белый D65).
fn primaries_matrix(from: &Primaries, to: &Primaries) -> Matrix3 {
    to.rgb_to_xyz()
        .unwrap()
        .inverse()
        .unwrap()
        .mul(&from.rgb_to_xyz().unwrap())
}

fn apply_f64(matrix: &Matrix3, color: [f64; 3]) -> [f64; 3] {
    matrix.apply(color)
}

impl ModelUnderTest for WorkingModel {
    fn name(&self) -> &'static str {
        self.name
    }

    fn scene_middle_gray(&self) -> f64 {
        self.middle_gray
    }

    fn render(&self, gamut: DisplayGamut, scene_linear_srgb: [f64; 3]) -> Rendered {
        let base = self.base_for(gamut);
        let to_working = primaries_matrix(&Primaries::SRGB, &WORKING_PRIMARIES);
        let scene_working = apply_f64(&to_working, scene_linear_srgb).map(|channel| channel as f32);
        let display_working = base.map_scene_pixel(scene_working, 1.0);
        let output = base.to_output(display_working);
        // «Сжатие/обрезка сработали»: выход после `to_output` отличается от голой матрицы `W → выход`
        // (канал вне 0…1 обрезается, у границы охвата сжатие двигает цвет).
        let to_output = primaries_matrix(&WORKING_PRIMARIES, &gamut.primaries());
        let raw = apply_f64(&to_output, display_working.map(f64::from));
        let engaged = (0..3).any(|channel| (raw[channel] - f64::from(output[channel])).abs() > GAMUT_EDGE_TOLERANCE);
        // Подгонка filmic зависит от гаммы выхода: основа sRGB и основа P3 дают РАЗНЫЙ `W`-свет.
        let other = self.base_for(match gamut {
            DisplayGamut::Srgb => DisplayGamut::DisplayP3,
            DisplayGamut::DisplayP3 => DisplayGamut::Srgb,
        });
        let other_display = other.map_scene_pixel(scene_working, 1.0);
        let fit_differs = (0..3).any(|channel| (other_display[channel] - display_working[channel]).abs() > 1e-6);
        Rendered {
            output,
            output_gamut_engaged: engaged,
            base_depends_on_gamut: fit_differs,
        }
    }
}

/// Прежняя кривая: работает в цветах выхода (`W = выход`): сжатие охвата ACES RGC → кривая → обрезка.
struct LegacyModel {
    compression: GamutCompression,
    curve: ToneCurve,
    middle_gray: f64,
}

impl ModelUnderTest for LegacyModel {
    fn name(&self) -> &'static str {
        "legacy"
    }

    fn scene_middle_gray(&self) -> f64 {
        self.middle_gray
    }

    fn render(&self, gamut: DisplayGamut, scene_linear_srgb: [f64; 3]) -> Rendered {
        let to_gamut = primaries_matrix(&Primaries::SRGB, &gamut.primaries());
        let scene = apply_f64(&to_gamut, scene_linear_srgb).map(|channel| channel as f32);
        let compressed = self.compression.compress_pixel(scene);
        let engaged = (0..3).any(|channel| (compressed[channel] - scene[channel]).abs() > 1e-6);
        let output = self.curve.map_pixel(compressed).map(|channel| channel.clamp(0.0, 1.0));
        Rendered {
            output,
            output_gamut_engaged: engaged,
            base_depends_on_gamut: false,
        }
    }
}

fn models() -> Vec<Box<dyn ModelUnderTest>> {
    let agx = AgxSettings::default();
    let aces = AcesSettings::default();
    let filmic = FilmicSettings::default();
    let tone = ToneCurveSettings::default();
    vec![
        Box::new(WorkingModel {
            name: "agx",
            middle_gray: f64::from(agx.scene_middle_gray),
            for_srgb: Box::new(rc_agx(AgxBase::resolve(&agx, DisplayGamut::Srgb).unwrap())),
            for_p3: Box::new(rc_agx(AgxBase::resolve(&agx, DisplayGamut::DisplayP3).unwrap())),
        }),
        Box::new(WorkingModel {
            name: "aces",
            middle_gray: f64::from(aces.scene_middle_gray),
            for_srgb: Box::new(rc_aces(AcesBase::resolve(&aces, DisplayGamut::Srgb).unwrap())),
            for_p3: Box::new(rc_aces(AcesBase::resolve(&aces, DisplayGamut::DisplayP3).unwrap())),
        }),
        Box::new(WorkingModel {
            name: "filmic",
            middle_gray: f64::from(filmic.scene_middle_gray),
            for_srgb: Box::new(rc_filmic(FilmicBase::resolve(&filmic, DisplayGamut::Srgb).unwrap())),
            for_p3: Box::new(rc_filmic(
                FilmicBase::resolve(&filmic, DisplayGamut::DisplayP3).unwrap(),
            )),
        }),
        Box::new(LegacyModel {
            compression: GamutCompression::resolve(&GamutCompressionSettings::default()).unwrap(),
            curve: ToneCurve::resolve(&tone).unwrap(),
            middle_gray: 0.18,
        }),
    ]
}

/// `Arc<T>` не реализует контракт сам; тонкие обёртки отдают контракт `T` через `Arc`.
macro_rules! arc_wrapper {
    ($wrapper:ident, $function:ident, $base:ty) => {
        struct $wrapper(std::sync::Arc<$base>);
        fn $function(base: std::sync::Arc<$base>) -> $wrapper {
            $wrapper(base)
        }
        impl WorkingToneBase for $wrapper {
            fn output_gamut(&self) -> DisplayGamut {
                self.0.output_gamut()
            }
            fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
                self.0.xyz_d50_to_working(xyz_d50)
            }
            fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
                self.0.map_scene_pixel(scene, white)
            }
            fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
                self.0.to_output(display_working)
            }
        }
    };
}
arc_wrapper!(AgxArc, rc_agx, AgxBase);
arc_wrapper!(AcesArc, rc_aces, AcesBase);
arc_wrapper!(FilmicArc, rc_filmic, FilmicBase);

/// Результат сравнения одного входа при одной экспозиции.
struct Measurement {
    model: &'static str,
    kind: InputKind,
    input: String,
    exposure_ev: f64,
    delta_e: f64,
    /// Сжатие охвата выхода (или обрезка) сработало хотя бы в одном из двух профилей.
    output_compression: bool,
    /// Свет основы в `W` зависит от гаммы выхода (подгонка filmic).
    gamut_fit: bool,
}

impl Measurement {
    /// «Обычный» цвет: ни выходное сжатие, ни подгонка охвата его не тронули.
    fn is_ordinary(&self) -> bool {
        !self.output_compression && !self.gamut_fit
    }
}

fn measure(model: &dyn ModelUnderTest) -> Vec<Measurement> {
    let srgb_lab = DisplayToLab::new(DisplayGamut::Srgb).unwrap();
    let p3_lab = DisplayToLab::new(DisplayGamut::DisplayP3).unwrap();
    let mut results = Vec::new();
    for input in all_inputs() {
        for exposure_ev in EXPOSURES_EV {
            // 0,18 серого сцены → средний серый модели; экспозиция умножает свет на 2^e.
            let scale = model.scene_middle_gray() / 0.18 * exposure_ev.exp2();
            let scene = input.linear_srgb_at_grey.map(|channel| channel * scale);
            let in_srgb = model.render(DisplayGamut::Srgb, scene);
            let in_p3 = model.render(DisplayGamut::DisplayP3, scene);
            results.push(Measurement {
                model: model.name(),
                kind: input.kind,
                input: input.name.clone(),
                exposure_ev,
                delta_e: delta_e_2000(srgb_lab.convert(in_srgb.output), p3_lab.convert(in_p3.output)),
                output_compression: in_srgb.output_gamut_engaged || in_p3.output_gamut_engaged,
                gamut_fit: in_srgb.base_depends_on_gamut || in_p3.base_depends_on_gamut,
            });
        }
    }
    results
}

fn max_delta_e<'a>(rows: impl Iterator<Item = &'a Measurement>) -> Option<&'a Measurement> {
    rows.max_by(|left, right| left.delta_e.total_cmp(&right.delta_e))
}

/// Таблица для владельца: по модели и типу входа — наибольший ΔE2000 у «обычных» и у задетых сжатием/подгонкой.
fn format_table(rows: &[Measurement]) -> String {
    let mut table = String::new();
    for kind in [InputKind::ColorChecker, InputKind::SaturationRamp, InputKind::GrayScale] {
        for ordinary in [true, false] {
            let group = || {
                rows.iter()
                    .filter(|row| row.kind == kind && row.is_ordinary() == ordinary)
            };
            let Some(worst) = max_delta_e(group()) else { continue };
            let fit_count = group().filter(|row| row.gamut_fit).count();
            table.push_str(&format!(
                "{:7} {:15} {:9} n={:3} (fit filmic: {:2}) max dE2000 = {:6.3} at {} e={:+}\n",
                worst.model,
                format!("{kind:?}"),
                if ordinary { "ordinary" } else { "compressed" },
                group().count(),
                fit_count,
                worst.delta_e,
                worst.input,
                worst.exposure_ev
            ));
        }
    }
    table
}

#[test]
fn ordinary_colors_match_between_srgb_and_display_p3_in_every_model() {
    for model in models().iter().filter(|model| model.name() != "legacy") {
        let rows = measure(model.as_ref());
        eprintln!("{}", format_table(&rows));
        let ordinary_count = rows.iter().filter(|row| row.is_ordinary()).count();
        let compressed_count = rows.len() - ordinary_count;
        // Классификация не вырождена: есть и обычные цвета, и задетые сжатием (иначе тест ничего не меряет).
        assert!(
            ordinary_count > 50,
            "{}: обычных цветов мало ({ordinary_count})",
            model.name()
        );
        assert!(compressed_count > 0, "{}: нет цветов у границы охвата", model.name());
        let worst = max_delta_e(rows.iter().filter(|row| row.is_ordinary())).unwrap();
        assert!(
            worst.delta_e <= ORDINARY_DELTA_E_LIMIT,
            "{}: обычный цвет «{}» при e={:+} расходится между sRGB и Display P3 на ΔE2000 = {:.3}",
            model.name(),
            worst.input,
            worst.exposure_ev,
            worst.delta_e
        );
    }
}

/// Серое остаётся серым в обоих профилях одинаково (белая точка D65 общая): расхождение — только округление f32.
#[test]
fn gray_scale_is_identical_between_profiles_in_every_model_including_legacy() {
    for model in models() {
        let rows = measure(model.as_ref());
        let worst = max_delta_e(rows.iter().filter(|row| row.kind == InputKind::GrayScale)).unwrap();
        assert!(
            worst.delta_e <= 0.01,
            "{}: серый «{}» при e={:+} расходится на ΔE2000 = {:.4}",
            model.name(),
            worst.input,
            worst.exposure_ev,
            worst.delta_e
        );
    }
}

/// Справка (без утверждений о совпадении): прежняя кривая и таблицы всех моделей по запросу `--nocapture`.
#[test]
fn print_reference_table_for_all_models() {
    for model in models() {
        let rows = measure(model.as_ref());
        eprintln!("{}", format_table(&rows));
        assert!(
            rows.iter().all(|row| row.delta_e.is_finite()),
            "{}: ΔE не число",
            model.name()
        );
    }
}
