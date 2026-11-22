//! Стенд снимков на ПК (сессия 14, P4b): настоящий бэкенд показа с видоискателем и снимками (RADV, headless, полная
//! validation) поверх `continuity_rig`. Кадры — однородные ячейки с РАЗНЫМИ значениями, поэтому по сырым данным
//! снимка видно, какой именно кадр снят.
//!
//! Записей показа драйвера (`VK_GOOGLE_display_timing`) на ПК нет: стенд подаёт их сам по журналу показов
//! видоискателя (`present_history` диагностики), как их подал бы драйвер.

#![allow(
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "стенд тестов: падение — сигнал о провале"
)]

use std::sync::{Arc, Mutex, PoisonError};

use super::continuity_rig::{
    CellValues, MAIN, Rig, SIZE_A, StillSetup, bggr_sample, capture_hints, passport, scene_neutral, test_continuity,
};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::PixelSize;
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::lens_calibration::{OpticalAxis, UncalibratedReason};
use camera_contract::viewfinder_crop::{CropAspect, ZoomFactor};
use camera_contract::zoom::{
    LensGroup, LensZoomPoint, PreviewMirror, ZoomCatalog, ZoomFraming, ZoomRange, ZoomRatio, ZoomStop, ZoomStopKind,
};
use gpu_vulkan::kit_test_support::run_one_shot_blocking_traced;
use gpu_vulkan::raw_mosaic_preview::ValidArea;
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::exposure::ExposureGain;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::run_reference;
use isp_model::sensor::SensorDescription;
use isp_vulkan::photo::{
    DevelopRequest, PhotoConfig, PhotoDeveloper, RawCapturer, RgbaReadback, SliceOutcome, quantize_display_encoded,
};
use raw_frame_stream::StreamEpoch;
use render_loop::backend::{DisplayBackend, PastPresentation};
use render_loop::config::{StillDevelopConfig, ViewfinderToneConfig};
use render_loop::still::{StillBegin, StillCommand, StillCommandOutcome, StillEventSink};
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
use still_contract::develop::DevelopEvent;
use still_contract::plan::{FrameSelection, PrimaryFormat, RawOutput, ShutterPress, StillId, StillOutputs, StillPlan};

/// Приёмник событий снимка.
#[derive(Default)]
pub struct Sink(pub Mutex<Vec<DevelopEvent>>);

impl StillEventSink for Sink {
    fn deliver(&self, event: DevelopEvent) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

impl Sink {
    pub fn events(&self) -> std::sync::MutexGuard<'_, Vec<DevelopEvent>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn finals(&self) -> usize {
        self.events().iter().filter(|event| event.is_final()).count()
    }

    pub fn has_secured(&self) -> bool {
        self.events()
            .iter()
            .any(|event| matches!(event, DevelopEvent::Secured { .. }))
    }
}

fn lens(camera: &str, role: LensRole) -> LensZoomPoint {
    LensZoomPoint {
        role,
        camera: camera_id(camera),
        native_ratio: ZoomRatio::new(1.0).expect("коэффициент"),
        equivalent_focal_mm: 24.0,
        optical_axis: OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::NotReported,
        },
        active_array: SIZE_A,
        preview_mirror: PreviewMirror::None,
        max_digital_zoom: ZoomFactor::new(10.0).expect("зум"),
    }
}

pub fn catalog() -> Arc<ZoomCatalog> {
    let lens = lens(MAIN, LensRole::Wide);
    Arc::new(ZoomCatalog {
        back: Some(LensGroup {
            facing: LensFacing::Back,
            stops: vec![ZoomStop {
                ratio: lens.native_ratio,
                kind: ZoomStopKind::Lens(LensRole::Wide),
            }],
            lenses: vec![lens],
            range: ZoomRange {
                min: ZoomRatio::new(1.0).expect("коэффициент"),
                max: ZoomRatio::new(10.0).expect("коэффициент"),
            },
        }),
        front: None,
    })
}

pub fn plan(id: u64, press: MonotonicNanos, outputs: StillOutputs) -> StillPlan {
    StillPlan {
        id: StillId(id),
        press: ShutterPress { at: press },
        selection: FrameSelection::ShownAtPress,
        framing: ZoomFraming {
            aspect: CropAspect::SensorNative,
            facing: LensFacing::Back,
            ratio: ZoomRatio::new(1.0).expect("коэффициент"),
            catalog: catalog(),
        },
        orientation: DeviceOrientation::Portrait,
        outputs,
    }
}

pub const JPEG: StillOutputs = StillOutputs {
    primary: PrimaryFormat::Jpeg,
    raw: RawOutput::Skip,
};
pub const JPEG_AND_DNG: StillOutputs = StillOutputs {
    primary: PrimaryFormat::Jpeg,
    raw: RawOutput::Dng,
};
pub const HEIF: StillOutputs = StillOutputs {
    primary: PrimaryFormat::Heif,
    raw: RawOutput::Skip,
};

/// Ячейка кадра номер `index`: значения различаются, чтобы по сырым данным узнавался кадр.
pub fn cell(index: usize) -> CellValues {
    let step = u16::try_from(index).expect("номер кадра") * 9;
    CellValues {
        red: 300 + step,
        green: 250 + step,
        blue: 200 + step,
    }
}

/// Стенд: видоискатель + снимки + показанные кадры.
pub struct StillBench {
    pub rig: Rig,
    pub epoch: StreamEpoch,
    pub passport: Arc<StreamPassport>,
    /// Метки показанных кадров по порядку показа и их ячейки.
    pub cells: Vec<CellValues>,
    injected_timings: usize,
}

pub fn setup(config: StillDevelopConfig, ring_frames: u32) -> Option<StillBench> {
    let rig = Rig::with_stills(
        test_continuity(render_loop::config::RevealExposureCheck::Ignore),
        false,
        Some(StillSetup { config, ring_frames }),
    )?;
    let passport = Arc::new(passport(MAIN, SIZE_A));
    let epoch = rig.begin(&passport);
    Some(StillBench {
        rig,
        epoch,
        passport,
        cells: Vec::new(),
        injected_timings: 0,
    })
}

impl StillBench {
    /// Кадр показа: предлагает кадр с ячейкой `cell(n)`, показывает его, продвигает снимки.
    pub fn frame(&mut self) {
        let index = self.cells.len();
        self.cells.push(cell(index));
        self.rig.offer(self.epoch, cell(index));
        self.rig.tick();
        self.rig.backend.advance_stills();
    }

    pub fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame();
        }
    }

    /// Кадр показа без нового кадра камеры (перекомпоновка): снимки продвигаются.
    pub fn idle_frame(&mut self) {
        self.rig.tick();
        self.rig.backend.advance_stills();
    }

    /// Подаёт движку записи показа драйвера для всех показов, о которых видоискатель уже сообщил: показ номер `i`
    /// (в порядке показа) появился на экране в `10·i` мс. Возвращает сколько показов известно.
    pub fn inject_timings(&mut self) -> usize {
        let camera = self.rig.backend.diagnostics().camera.expect("диагностика камеры");
        let new: Vec<PastPresentation> = camera
            .present_history
            .iter()
            .enumerate()
            .skip(self.injected_timings)
            .map(|(index, sample)| PastPresentation {
                present_id: sample.present_id,
                frame_index: index as u64,
                actual_present: shown_at(index),
            })
            .collect();
        self.injected_timings = camera.present_history.len();
        self.rig.backend.inject_past_presentations(&new);
        self.injected_timings
    }

    /// Начинает снимок.
    pub fn begin(&mut self, plan: StillPlan) -> Arc<Sink> {
        let sink = Arc::new(Sink::default());
        let events: Arc<dyn StillEventSink> = Arc::clone(&sink) as Arc<dyn StillEventSink>;
        let outcome = self
            .rig
            .backend
            .apply_still_command(StillCommand::Begin(StillBegin { plan, events }));
        assert!(matches!(outcome, StillCommandOutcome::Accepted), "{outcome:?}");
        sink
    }

    /// Кадры показа, пока `done` не станет истиной (не больше `limit`); каждый — с новым кадром камеры.
    pub fn frames_until(&mut self, limit: usize, mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..limit {
            if done() {
                return true;
            }
            self.frame();
        }
        done()
    }
}

/// Когда показ номер `index` появился на экране (стенд).
pub fn shown_at(index: usize) -> MonotonicNanos {
    MonotonicNanos::from_nanos(10_000_000 * i64::try_from(index).expect("номер показа"))
}

/// Нажатие на 3 мс позже появления показа `index`: на экране тогда стоит именно он.
pub fn press_during(index: usize) -> MonotonicNanos {
    MonotonicNanos::from_nanos(shown_at(index).as_nanos() + 3_000_000)
}

/// Сырые отсчёты кадра `cell` целиком (RAW16, little-endian, строками).
pub fn raw_bytes(cell: CellValues, size: PixelSize) -> Vec<u8> {
    (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| bggr_sample(cell, x, y)))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Параметры ISP снимка так, как их собирает рендер: Quality, охват sRGB, тон видоискателя, нейтраль нашей белой.
pub fn expected_params(passport: &StreamPassport) -> (IspParams, SensorDescription) {
    let sensor = SensorDescription::from_stream_passport(passport, &passport.color_calibration).expect("сенсор");
    let frame = FrameInputs::from_capture_hints(&sensor, &capture_hints()).expect("уровни");
    let neutral = CameraNeutral::new(scene_neutral()).expect("нейтраль");
    let controls = ProcessingControls {
        tone: ViewfinderToneConfig::default().curve_settings(),
        exposure_gain: ExposureGain::UNITY,
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, DisplayGamut::Srgb)
    };
    (
        IspParams::resolve(&sensor, &frame, &controls).expect("параметры"),
        sensor,
    )
}

/// Эталон `isp-model`: RGBA8 кадра `cell` (альфа 255), квантованный правилом снимка.
pub fn reference_rgba(params: &IspParams, cell: CellValues, size: PixelSize) -> Vec<u8> {
    let samples: Vec<u16> = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| bggr_sample(cell, x, y)))
        .collect();
    let raw = RawMosaic::new(size, BayerPattern::BGGR, samples).expect("мозаика");
    run_reference(&raw, params)
        .expect("эталон")
        .display_encoded
        .pixels()
        .iter()
        .flat_map(|pixel| {
            [
                quantize_display_encoded(pixel[0]),
                quantize_display_encoded(pixel[1]),
                quantize_display_encoded(pixel[2]),
                255,
            ]
        })
        .collect()
}

/// Конвейер `isp_vulkan::photo` напрямую (без рендера) на тех же параметрах: захват и проявление целиком.
pub fn photo_pipeline_rgba(
    context: &gpu_vulkan::context::GpuContext,
    params: &IspParams,
    cell: CellValues,
    size: PixelSize,
    config: &PhotoConfig,
) -> Vec<u8> {
    let texels: Vec<u16> = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| bggr_sample(cell, x, y)))
        .collect();
    let source = SyntheticRaw16::new(context, size.width, size.height, &texels).expect("источник");
    let mosaic = source.source();
    let mut capturer = RawCapturer::new(context, config).expect("захватчик");
    let mut pending = None;
    let area = ValidArea {
        left: 0,
        top: 0,
        width: size.width,
        height: size.height,
    };
    let run = run_one_shot_blocking_traced(context, |command_buffer| {
        pending = Some(
            capturer
                .record_capture(command_buffer, &mosaic, area)
                .map_err(|error| gpu_vulkan::error::GpuError::InvalidState {
                    what: error.to_string(),
                })?,
        );
        Ok(())
    })
    .expect("копия");
    let mut pending = pending.expect("записана");
    pending.note_submitted(&run.work);
    let raw = capturer
        .complete_after_fence(pending, &run.proof)
        .expect("подтверждение");
    let mut developer = PhotoDeveloper::new(context, config, size.width).expect("проявитель");
    developer
        .begin(&DevelopRequest {
            raw: &raw,
            params,
            crop: camera_contract::geometry::PixelRect { left: 0, top: 0, size },
            readback: RgbaReadback::NotNeeded,
        })
        .expect("начало");
    loop {
        let mut outcome = None;
        let run = run_one_shot_blocking_traced(context, |command_buffer| {
            outcome = Some(developer.record_next_slice(command_buffer).map_err(|error| {
                gpu_vulkan::error::GpuError::InvalidState {
                    what: error.to_string(),
                }
            })?);
            Ok(())
        })
        .expect("полоса");
        match outcome.expect("итог полосы") {
            SliceOutcome::Finished => break,
            SliceOutcome::Recorded { .. } => {
                developer
                    .note_slice_submitted(&run.work)
                    .expect("отметка работы полосы");
                developer.slice_completed(&run.proof).expect("подтверждение полосы");
            }
        }
    }
    let photo = developer.finish().expect("снимок");
    photo.gpu_image().read_back_blocking().expect("чтение картинки")
}
