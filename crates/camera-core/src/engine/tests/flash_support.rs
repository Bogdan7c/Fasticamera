//! Стенд снимка со вспышкой: наш AE на настоящем ящике, ящик указаний, наблюдатель итогов и
//! помощники замеров предвспышки.

use camera_contract::auto_exposure::{
    CameraTag, ExposureDirective, ExposureProduct, ExposureProposal, ExposureProposalChannel, MeasurementQuality,
    MeteredArea, ProposalConvergence, RefreshSignal,
};
use camera_contract::capabilities::CameraDescriptor;
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::fixtures::{camera_id, oneplus11_visible_cameras};
use camera_contract::flash::{
    FlashControl, FlashShotId, PreflashLight, PreflashMetering, PreflashSample, PreflashSamples,
};
use camera_contract::ids::CameraId;
use camera_contract::three_a::{DirectiveSlot, MainsFrequency, RequestOrigin};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::three_a_support::*;
use super::*;
use crate::fake::FakeBackendControl;
use crate::flash_status::{FlashPhase, FlashStillObserver, FlashStillReport};
use crate::request_controls::ExposurePlan;
use crate::three_a_own::RawStatsExposure;
use crate::three_a_vendor::VendorFocus;

/// Метка кадра `frame` на шкале `result_meta`.
pub(super) fn frame_time(frame: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(1_000_000_000 + frame * 16_666_667)
}

/// Номер снимка, которым пользуются тесты.
pub(super) const SHOT: FlashShotId = FlashShotId(41);

#[derive(Default)]
struct CountingSignal(AtomicU32);

impl RefreshSignal for CountingSignal {
    fn signal(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Наблюдатель, который запоминает все итоги.
#[derive(Default)]
pub(super) struct RecordingObserver {
    reports: Mutex<Vec<FlashStillReport>>,
}

impl RecordingObserver {
    pub(super) fn reports(&self) -> Vec<FlashStillReport> {
        self.reports.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl FlashStillObserver for RecordingObserver {
    fn flash_still_finished(&self, report: &FlashStillReport) {
        self.reports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(*report);
    }
}

/// Стенд: поток на основной камере под нашим AE, ящики и наблюдатель.
pub(super) struct FlashRig {
    pub(super) rig: Rig,
    pub(super) channel: ExposureProposalChannel,
    pub(super) directives: DirectiveSlot<ExposureDirective>,
    pub(super) observer: Arc<RecordingObserver>,
    signal: Arc<CountingSignal>,
    delivered: u32,
    pub(super) next_frame: i64,
    /// Последний кадр повторяющегося запроса на момент `capture` (запрос с фонариком выдан после него).
    request_frame: i64,
    /// Камера, на которой идёт поток: от неё приходят замеры.
    primary: CameraId,
}

impl FlashRig {
    /// Стенд на основной камере; AE получил цель `target` с·ISO.
    pub(super) fn measured(target: f64) -> Self {
        let mut flash_rig = Self::unmeasured();
        flash_rig.measure(target);
        flash_rig
    }

    /// То же, но AE ещё не принял ни одного замера.
    pub(super) fn unmeasured() -> Self {
        Self::build(CameraEngineConfig::default(), |_| {}, true, (camera_id("2"), wide()))
    }

    /// Основная камера, у которой тест что-то поменял в характеристиках.
    pub(super) fn with_edited_wide(edit: impl FnOnce(&mut CameraDescriptor)) -> Self {
        let mut flash_rig = Self::build(CameraEngineConfig::default(), edit, true, (camera_id("2"), wide()));
        flash_rig.measure(8.0);
        flash_rig
    }

    /// Стенд на камере `camera` (её роль — `target`) со своим конфигом движка; `edit` правит её
    /// характеристики; AE получил цель 8 с·ISO.
    pub(super) fn on_camera(
        config: CameraEngineConfig,
        camera: &str,
        target: CameraTarget,
        edit: impl FnOnce(&mut CameraDescriptor),
    ) -> Self {
        let mut flash_rig = Self::build(config, edit, true, (camera_id(camera), target));
        flash_rig.measure(8.0);
        flash_rig
    }

    /// Стенд без наблюдателя: итог идёт только событием.
    pub(super) fn without_observer(target: f64) -> Self {
        let mut flash_rig = Self::build(CameraEngineConfig::default(), |_| {}, false, (camera_id("2"), wide()));
        flash_rig.measure(target);
        flash_rig
    }

    fn build(
        config: CameraEngineConfig,
        edit: impl FnOnce(&mut CameraDescriptor),
        with_observer: bool,
        (primary, target): (CameraId, CameraTarget),
    ) -> Self {
        let channel = ExposureProposalChannel::new();
        let signal = Arc::new(CountingSignal::default());
        channel
            .refresh_signal()
            .bind(Arc::clone(&signal) as Arc<dyn RefreshSignal>);
        let own = RawStatsExposure::new(config.three_a.exposure, channel.clone(), MainsFrequency::Hz50);
        let directives = DirectiveSlot::new();
        let observer = Arc::new(RecordingObserver::default());
        let mut wiring = ThreeAWiring::vendor(&config.three_a)
            .with_controllers(
                Box::new(own),
                Box::new(VendorFocus::new(config.three_a.focus_marker_grace_frames)),
            )
            .with_exposure_directive_slot(directives.clone());
        if with_observer {
            wiring = wiring.with_flash_observer(Arc::clone(&observer) as Arc<dyn FlashStillObserver>);
        }
        let mut cameras = oneplus11_visible_cameras();
        if let Some(primary_camera) = cameras.iter_mut().find(|camera| camera.id == primary) {
            edit(primary_camera);
        }
        let fake = FakeBackendControl::new();
        fake.set_cameras(cameras);
        let mut rig = Rig::over_fake_with_wiring(config, AppPresence::Foreground, fake, wiring);
        rig.engine.load_catalog(rig.now);
        rig.pump();
        rig.start_streaming_on(target);
        Self {
            rig,
            channel,
            directives,
            observer,
            signal,
            delivered: 0,
            next_frame: 1,
            request_frame: 0,
            primary,
        }
    }

    /// Результат повторяющегося запроса: он ставит метку начала потока, без неё предложения придерживаются.
    pub(super) fn feed_repeating(&mut self) -> i64 {
        let frame = self.next_frame;
        self.next_frame += 1;
        self.rig.feed_result(result_meta(frame, RequestOrigin::Repeating, None));
        self.deliver_pushes();
        frame
    }

    /// Наш AE принял цель `target` на основной камере.
    pub(super) fn measure(&mut self, target: f64) {
        let frame = self.feed_repeating();
        self.channel.publish(ExposureProposal {
            measured_frame: frame_time(frame),
            measured_on: CameraTag::of(&self.primary),
            target: product(target),
            metered_area: MeteredArea::WholeFrame,
            convergence: ProposalConvergence::Settled,
            quality: MeasurementQuality::Measured,
        });
        self.deliver_pushes();
    }

    /// Поток камеры: каждый новый толчок ящика — один `refresh_three_a`.
    pub(super) fn deliver_pushes(&mut self) {
        let pushes = self.signal.0.load(Ordering::SeqCst);
        if pushes > self.delivered {
            self.delivered = pushes;
            self.rig.engine.refresh_three_a(self.rig.now);
            self.rig.pump();
        }
    }

    /// Рендер положил замеры предвспышки.
    pub(super) fn publish_samples(&mut self, samples: PreflashSamples) {
        self.channel.publish_preflash(samples);
        self.deliver_pushes();
    }

    /// Принять команду снимка со вспышкой; итог команды.
    pub(super) fn capture(&mut self, shot: FlashShotId) -> CommandOutcome {
        self.request_frame = self.next_frame - 1;
        self.rig.send(CameraCommand::CaptureFlashStill { shot })
    }

    /// Контролы повторяющегося запроса, который сейчас стоит у бэкенда.
    pub(super) fn repeating_controls(&self) -> crate::request_controls::RequestControls {
        self.rig.fake.repeating_request().expect("запрос идёт").1
    }

    /// Произведение «выдержка × ISO» повторяющегося запроса.
    pub(super) fn repeating_product(&self) -> f64 {
        match self.repeating_controls().exposure {
            ExposurePlan::Sensor { exposure_time, iso } => {
                ExposureProduct::from_sensor(exposure_time, iso).seconds_iso()
            }
            other => panic!("ожидали Sensor: {other:?}"),
        }
    }

    pub(super) fn directive(&self) -> ExposureDirective {
        self.directives.latest().expect("указание опубликовано с начала").1
    }

    pub(super) fn phase(&self) -> FlashPhase {
        self.rig.engine.status().three_a.flash.phase
    }

    /// Инвариант светодиода: фонарик в повторяющемся запросе ⇔ фаза `Preflash` ⇔ указание «предвспышка».
    pub(super) fn assert_led_invariant(&self, context: &str) {
        let in_preflash = matches!(self.phase(), FlashPhase::Preflash { .. });
        if let Some((_, controls)) = self.rig.fake.repeating_request() {
            assert_eq!(
                controls.flash == FlashControl::Torch,
                in_preflash,
                "{context}: фонарик в запросе должен гореть ровно в предвспышке"
            );
        }
        assert_eq!(
            matches!(self.directive().flash.preflash, PreflashMetering::Active { .. }),
            in_preflash,
            "{context}: указание «предвспышка» должно совпадать с фазой"
        );
    }

    /// Сколько ушло разовых запросов со вспышкой.
    pub(super) fn submitted_flash_stills(&mut self) -> usize {
        self.rig
            .fake
            .calls()
            .iter()
            .filter(|call| matches!(call, BackendCall::SubmitFlashStill { .. }))
            .count()
    }

    /// Номер кадра последнего результата повторяющегося запроса: кадр без светодиода, снятый ДО запроса
    /// фонарика (последний кадр до `capture`). Только такой замер без фонарика
    /// годится движку как свет сцены.
    pub(super) fn pre_request_frame(&self) -> i64 {
        self.request_frame
    }

    /// Предвспышка с годными замерами: фон со средним серым `ambient`, фонарик — `torch`.
    /// Замер с фонариком снят на той экспозиции, что стоит в запросе (как у настоящего рендера).
    pub(super) fn publish_good_samples(&mut self, ambient: f64, torch: f64) {
        let primary = self.primary.clone();
        let frame_product = self.repeating_product();
        let ambient_frame = self.pre_request_frame();
        let torch_frame = self.next_frame + 18;
        self.publish_samples(PreflashSamples {
            ambient: Some(sample_on(
                &primary,
                PreflashLight::Ambient,
                ambient_frame,
                frame_product,
                ambient,
                MeasurementQuality::Measured,
            )),
            torch: None,
        });
        self.publish_samples(PreflashSamples {
            ambient: Some(sample_on(
                &primary,
                PreflashLight::Ambient,
                ambient_frame,
                frame_product,
                ambient,
                MeasurementQuality::Measured,
            )),
            torch: Some(sample_on(
                &primary,
                PreflashLight::Torch,
                torch_frame,
                frame_product,
                torch,
                MeasurementQuality::Measured,
            )),
        });
    }
}

pub(super) fn product(seconds_iso: f64) -> ExposureProduct {
    ExposureProduct::from_seconds_iso(seconds_iso).expect("произведение")
}

/// Замер кадра `frame` основной камеры.
pub(super) fn sample(
    light: PreflashLight,
    frame: i64,
    frame_product: f64,
    mid_gray: f64,
    quality: MeasurementQuality,
) -> PreflashSample {
    sample_on(&camera_id("2"), light, frame, frame_product, mid_gray, quality)
}

/// Замер кадра `frame` камеры `camera`.
pub(super) fn sample_on(
    camera: &CameraId,
    light: PreflashLight,
    frame: i64,
    frame_product: f64,
    mid_gray: f64,
    quality: MeasurementQuality,
) -> PreflashSample {
    PreflashSample {
        frame: frame_time(frame),
        measured_on: CameraTag::of(camera),
        light,
        frame_product: product(frame_product),
        mid_gray_product: product(mid_gray),
        quality,
    }
}

/// Итоги снимков со вспышкой из событий.
pub(super) fn flash_reports(events: &[CameraEvent]) -> Vec<FlashStillReport> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::FlashStillFinished { report } => Some(*report),
            _ => None,
        })
        .collect()
}

/// Камера основной роли и её метка нужны тестам, которым чужая камера.
pub(super) fn foreign_tag() -> CameraTag {
    CameraTag::of(&camera_id("0"))
}

/// `count` миллисекунд.
pub(super) fn ms(count: u64) -> std::time::Duration {
    std::time::Duration::from_millis(count)
}
