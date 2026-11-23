//! Приборы на кадре показа камеры (ревью сессии 14h) на offscreen-цели, которую можно ПРОЧИТАТЬ: настоящий ISP,
//! компоновщик, рендер UI и `ScopePass` на реальном драйвере с полной validation.
//!
//! Доказывается:
//! * итоговые пиксели цели: картинка прибора, нарисованная UI как живая картинка, равна эталону `scope-model`
//!   (счётчики по показанным кодам → `render`), наложенному «прямой альфой поверх» на пиксели видоискателя под ней;
//!   вне её цель равна кадру без приборов;
//! * кадр без изображения ISP считается в `frames_skipped_no_input` и ничего не пишет; кадр, скрытый удержанием или
//!   чёрным, — в `frames_skipped_not_shown` (проверка на уровне записи приборов с явным состоянием показа: сквозной
//!   кадр с удержанием проверен в `tests/vulkan_scopes_gating.rs`).

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

use std::sync::Arc;

use camera_contract::capabilities::{LensShading, RawSupport, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::frame_passport::{
    CaptureColorHints, FrameCaptureDetails, FrameCaptureMeta, FrameSequence, RawFramePassport, StreamPassport,
};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use gpu_vulkan::backdrop::SubmissionSerial as BackdropSerial;
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use gpu_vulkan::source_window::HorizontalMirror;
use isp_model::color::chromaticity::Chromaticity;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::params::IspParams;
use isp_vulkan::viewfinder::{IspStageSet, OutputAllocation, ViewfinderGeometry};
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{
    HorizontalOrder, ResolvedScope, SampleStride, ScopeModelSettings, ScopePicture, ScopeRule, ShownCodes, accumulate,
    render,
};
use shell_contract::clock::MonotonicNanos;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, DrawList, ImageClip, ImageFit, ImageItem, ImageSource, LiveImage};
use ui_draw::frame_source::{BackdropContinuity, UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;
use ui_draw::light::LightDirection;
use ui_draw::transform::Affine2;

use super::screen_tests::{Stand, context_or_skip, pixel, rect};
use super::{FrameTarget, NewIspFrame, ShowFrameAs};
use crate::config::ScopesConfig;
use crate::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use crate::vulkan_backend::camera::SubmissionSerial;
use crate::vulkan_backend::camera::isp_params::IspParamsCache;
use crate::vulkan_backend::camera::ui_state::{UiOverlayFrame, UiOverlayState};

/// RAW-кадр стенда: 256×192 (после поворота сенсора на 90° содержимое 96×128; сетка AWB 32×24 помещается).
const RAW_WIDTH: u32 = 256;
const RAW_HEIGHT: u32 = 192;
const BLACK_LEVEL: u16 = 64;
const WHITE_LEVEL: u16 = 1023;
/// Допуск итоговых пикселей цели в единицах 8-битного кода: fp16 картинки прибора (≈ 0,5 кода), округление записи
/// цели Rgba8 и смешивания (по ≈ 1 коду) и квантование фона. Замеренный максимум (0,71 кода) печатается тестом.
const COMPOSITE_TOLERANCE_CODES: f32 = 1.5;
/// Прямоугольник картинки прибора на цели 100×200: 64×48 в (20, 10), целиком внутри видоискателя, 1 тексель = 1 пиксель.
const SCOPE_RECT: (u32, u32, u32, u32) = (20, 10, 64, 48);

/// Маленькая модель приборов (эталон на CPU считается мгновенно) и шаг отсчётов 1: меряется каждый пиксель.
fn small_scopes_config() -> ScopesConfig {
    ScopesConfig {
        model: ScopeModelSettings {
            trace_columns: 48,
            trace_levels: 64,
            vectorscope_size: 64,
            sample_stride: SampleStride::try_new(1).expect("шаг"),
            ..ScopeModelSettings::default()
        },
        ..ScopesConfig::default()
    }
}

fn waveform_request() -> ScopeRequest {
    ScopeRequest {
        kind: ScopeKind::Waveform,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(SCOPE_RECT.2, SCOPE_RECT.3).expect("размер"),
    }
}

fn stream_passport() -> Arc<StreamPassport> {
    let size = PixelSize::new(RAW_WIDTH, RAW_HEIGHT);
    let color_calibration = match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    };
    Arc::new(StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw16,
        buffer_size: size,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect { left: 0, top: 0, size },
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(size),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Bggr,
        },
        static_levels: SensorLevels {
            black_level_pattern: [u32::from(BLACK_LEVEL); 4],
            white_level: u32::from(WHITE_LEVEL),
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration,
    })
}

/// Параметры ISP кадра: дневной свет, охват sRGB (цель стенда — sRGB).
fn isp_params_of(passport: &Arc<StreamPassport>) -> IspParams {
    let frame = RawFramePassport {
        stream: Arc::clone(passport),
        sequence: FrameSequence(1),
        sensor_timestamp: SensorTimestamp::from_nanos(10),
        capture: FrameCaptureMeta::Paired(FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(1),
                sensor_timestamp: Some(SensorTimestamp::from_nanos(10)),
                exposure_time: None,
                sensitivity_iso: None,
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color: CaptureColorHints::default(),
        }),
    };
    let mut cache = IspParamsCache::for_demosaic(DemosaicMethod::Preview);
    cache.prepare(&frame, Chromaticity::D65, DisplayGamut::Srgb);
    cache.current().cloned().expect("параметры ISP разрешены")
}

/// Сюжет с плавным градиентом по обеим осям (фон не однородный: ошибочное смешивание или сдвиг заметны).
fn gradient_raw_texels() -> Vec<u16> {
    let span = u32::from(WHITE_LEVEL - BLACK_LEVEL) * 6 / 10;
    let total = RAW_WIDTH + 2 * RAW_HEIGHT;
    (0..RAW_HEIGHT)
        .flat_map(|y| {
            (0..RAW_WIDTH).map(move |x| {
                let level = (x + 2 * y) * span / total;
                BLACK_LEVEL + u16::try_from(level).expect("уровень помещается в u16")
            })
        })
        .collect()
}

/// Источник UI: картинка цветового прибора ровно в `SCOPE_RECT` (1 тексель картинки = 1 пиксель цели).
struct ScopeImageSource;

impl UiFrameSource for ScopeImageSource {
    fn name(&self) -> &'static str {
        "scope-image-test"
    }

    fn produce_frame(&mut self, _request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        let (left, top, width, height) = SCOPE_RECT;
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        list.items.push(DrawItem::Image(ImageItem {
            source: ImageSource::Live(LiveImage::ColorScope),
            // Преобразование ставит ЦЕНТР картинки.
            transform: Affine2::translate(left as f32 + width as f32 * 0.5, top as f32 + height as f32 * 0.5),
            size_px: PxSize::new(width as f32, height as f32),
            fit: ImageFit::Stretch,
            clip: ImageClip::None,
            opacity: Opacity::OPAQUE,
        }));
        Ok(list)
    }
}

/// Записывает кадр видоискателя (ISP → компоновка в верхней половине цели) и, если `with_ui`, UI поверх.
/// `serial` — номер сабмита (для подложки стекла и барьера приборов).
fn record_viewfinder_frame(
    pipelines: &mut super::CameraPipelines,
    target: &FrameTarget,
    raw: &gpu_vulkan::raw_mosaic_preview::MosaicSource,
    params: &IspParams,
    passport: &StreamPassport,
    ui: Option<&mut UiOverlayState>,
    serial: u64,
) -> Result<(), gpu_vulkan::error::GpuError> {
    let valid_area = passport.valid_area;
    let geometry = ViewfinderGeometry {
        valid_area,
        crop: valid_area,
        sensor_orientation: passport.sensor_orientation,
        allocation: OutputAllocation::Exact,
    };
    let recording = pipelines.record_new_frame(
        target,
        &NewIspFrame {
            submission: BackdropSerial(serial),
            source: raw,
            params,
            geometry,
            stages: IspStageSet::ALL,
            frame_number: serial,
            show: ShowFrameAs::Live,
            mirror: HorizontalMirror::Off,
        },
        None,
        None,
        rect(0.0, 0.0, 1.0, 0.5),
    );
    assert!(recording.isp.is_ok(), "ISP записан: {:?}", recording.isp);
    if let Some(state) = ui {
        let failures = pipelines.record_ui_overlay(
            target,
            &mut UiOverlayFrame {
                state,
                present_time: MonotonicNanos::from_nanos(1_000),
            },
            &recording.screen,
            SubmissionSerial(serial),
            BackdropContinuity::Live,
        );
        assert_eq!(failures, 0, "метки времени UI записаны");
    }
    Ok(())
}

/// Эталон: цвет прибора `scope_pixel` (прямая альфа) поверх фона `background` (8-битный код) → код цели.
fn composite_over(scope_pixel: [f32; 4], background: [u8; 4]) -> [f32; 3] {
    let alpha = scope_pixel[3];
    [0, 1, 2]
        .map(|channel| (scope_pixel[channel] * alpha + f32::from(background[channel]) / 255.0 * (1.0 - alpha)) * 255.0)
}

/// Картинка прибора, посчитанная CPU-эталоном по ПОКАЗАННОМУ изображению видоискателя (его читают с GPU).
fn cpu_scope_picture(stand: &Stand, config: &ScopesConfig, request: &ScopeRequest) -> ScopePicture {
    let image = stand
        .pipelines
        .read_live_picture()
        .expect("чтение изображения видоискателя")
        .expect("изображение видоискателя есть");
    let shown = ShownCodes::from_display_rgba(image.width, image.height, &image.pixels, HorizontalOrder::AsStored)
        .expect("коды");
    let rule = ResolvedScope::resolve(request, &config.model, DisplayGamut::Srgb).expect("правило");
    let counters = accumulate(&shown, config.model.sample_stride, &rule as &dyn ScopeRule).expect("счётчики");
    render(&counters, &rule, &config.model.render, request.image_size).expect("картинка прибора")
}

/// Итоговые пиксели цели: внутри прямоугольника — картинка прибора поверх видоискателя, снаружи — кадр без приборов.
#[test]
fn composited_pixels_equal_the_cpu_scope_picture_over_the_viewfinder_and_the_rest_of_the_frame_is_untouched() {
    let Some(context) = context_or_skip() else { return };
    let config = small_scopes_config();
    let request = waveform_request();
    let passport = stream_passport();
    let params = isp_params_of(&passport);
    let raw_frame = SyntheticRaw16::new(&context, RAW_WIDTH, RAW_HEIGHT, &gradient_raw_texels()).expect("RAW");
    let raw = raw_frame.source();
    let mut stand = Stand::with_scopes(&context, &config);
    let mut ui = UiOverlayState::new(8);
    assert_eq!(
        ui.apply(UiOverlayCommand::Install(Box::new(ScopeImageSource))),
        UiOverlayOutcome::Applied
    );
    stand.pipelines.prepare_ui_overlay().expect("рендер UI");

    // Кадр A: тот же кадр видоискателя без приборов и без UI — фон.
    let without_scopes =
        stand.frame(|pipelines, target| record_viewfinder_frame(pipelines, target, &raw, &params, &passport, None, 1));
    // Кадр B: приборы включены (между кадрами), прибор считает показанный кадр, UI рисует его картинку.
    stand.pipelines.prepare_scopes_between_frames(Some(request));
    let with_scopes = stand.frame(|pipelines, target| {
        record_viewfinder_frame(pipelines, target, &raw, &params, &passport, Some(&mut ui), 2)
    });

    let events = stand.pipelines.take_scope_events();
    assert_eq!(events.counters.passes_recorded, 1, "прибор записан на показанном кадре");
    assert_eq!(events.counters.record_failures, 0);
    assert_eq!(ui.snapshot().live_images_drawn, 1, "картинка прибора нарисована UI");

    let scope_picture = cpu_scope_picture(&stand, &config, &request);
    let (left, top, width, height) = SCOPE_RECT;
    let mut worst = 0.0_f32;
    let mut changed_pixels = 0_u32;
    for y in top..top + height {
        for x in left..left + width {
            let expected = composite_over(
                scope_picture
                    .pixel(x - left, y - top)
                    .expect("пиксель картинки прибора"),
                pixel(&without_scopes, x, y),
            );
            let actual = pixel(&with_scopes, x, y);
            for channel in 0..3 {
                worst = worst.max((f32::from(actual[channel]) - expected[channel]).abs());
            }
            changed_pixels += u32::from(actual != pixel(&without_scopes, x, y));
        }
    }
    println!("наибольшее расхождение итоговых пикселей: {worst:.2} кода (допуск {COMPOSITE_TOLERANCE_CODES})");
    assert!(
        worst <= COMPOSITE_TOLERANCE_CODES,
        "итог цели расходится с эталоном: {worst} > {COMPOSITE_TOLERANCE_CODES}"
    );
    assert!(
        changed_pixels > 100,
        "прибор что-то нарисовал поверх видоискателя ({changed_pixels} пикселей изменено)"
    );
    // Вне прямоугольника цель равна кадру без приборов, бит в бит.
    let mut outside_differences = 0_u32;
    for y in 0..without_scopes.height {
        for x in 0..without_scopes.width {
            let inside = (left..left + width).contains(&x) && (top..top + height).contains(&y);
            outside_differences += u32::from(!inside && pixel(&with_scopes, x, y) != pixel(&without_scopes, x, y));
        }
    }
    assert_eq!(outside_differences, 0, "вне прямоугольника приборов кадр не изменился");
    context.wait_idle().expect("простой GPU");
    assert_eq!(context.validation_report().errors, 0);
}

/// Кадр без изображения ISP и без подложки считается «нет входа» и ничего не пишет; закрытые приборы не считают ничего.
#[test]
fn a_new_frame_without_an_isp_image_is_counted_as_no_input_and_records_nothing() {
    let Some(context) = context_or_skip() else { return };
    let mut stand = Stand::with_scopes(&context, &small_scopes_config());
    let facts = |show| super::scope_pass::ScopedFrameFacts {
        show,
        submission: BackdropSerial(1),
        mirror: HorizontalMirror::Off,
    };

    // Приборы закрыты: ни один пропуск не считается.
    stand.frame(|pipelines, target| {
        pipelines.record_scopes_for_frame(target, &facts(ShowFrameAs::Live));
        Ok(())
    });
    assert!(
        stand.pipelines.take_scope_events().is_empty(),
        "закрытые приборы ничего не считают"
    );

    // Приборы открыты, ISP ещё не писал ни одного кадра: «нет входа», проход не записан, отказа нет.
    stand.pipelines.prepare_scopes_between_frames(Some(waveform_request()));
    stand.pipelines.take_scope_events();
    stand.frame(|pipelines, target| {
        pipelines.record_scopes_for_frame(target, &facts(ShowFrameAs::Live));
        Ok(())
    });
    let events = stand.pipelines.take_scope_events();
    assert_eq!(events.counters.frames_skipped_no_input, 1);
    assert_eq!(
        (
            events.counters.passes_recorded,
            events.counters.frames_skipped_not_shown,
            events.counters.frames_skipped_not_ready,
            events.counters.record_failures
        ),
        (0, 0, 0, 0),
        "ничего не записано и никаких других пропусков"
    );

    // Скрытый удержанием или чёрный новый кадр: считается как «не показан» (решение 189), до проверки входа.
    stand.frame(|pipelines, target| {
        pipelines.record_scopes_for_frame(target, &facts(ShowFrameAs::Held { magnification: 1.0 }));
        pipelines.record_scopes_for_frame(target, &facts(ShowFrameAs::Black));
        Ok(())
    });
    let events = stand.pipelines.take_scope_events();
    assert_eq!(events.counters.frames_skipped_not_shown, 2);
    assert_eq!(
        events.counters.frames_skipped_no_input, 0,
        "вход не проверялся: кадр не показан"
    );
    assert_eq!(events.counters.passes_recorded, 0);
}
