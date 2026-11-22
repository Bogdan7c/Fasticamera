//! Предвспышка: включить фонарик, принять линейные замеры, перемерить пересвеченный, посчитать кадр.
//!
//! Откуда берутся замеры. Рендер мерит кадры и кладёт `PreflashSamples` в ящик нашего AE
//! (`ExposureProposalChannel`); публикация будит поток камеры тем же сигналом, что и предложения
//! AE, и тот зовёт `refresh_three_a`, откуда вызывается [`CameraEngine::poll_flash_samples`].

use std::time::{Duration, Instant};

use camera_contract::auto_exposure::{CameraTag, ExposureProduct, MeasurementQuality};
use camera_contract::capabilities::FrameDuration;
use camera_contract::flash::{FlashShotId, FlashStillFailure, PreflashMetering, PreflashSample, PreflashSamples};

use super::super::{CameraEngine, LOG_TARGET};
use super::plan::{FlashPlanInputs, plan_flash_frame};
use super::{FlashRun, Preflash, PreflashExposure, Stage};
use crate::request_controls::ExposurePlan;
use crate::three_a_exposure_limits::{longest_usable_shutter, manual_sensor_ranges};
use crate::three_a_own::{Anchor, SplitLimits, split};
use camera_contract::control::{ExposureTime, Iso};

/// Насколько произведение кадра может отличаться от запрошенного, чтобы считать кадр кадром
/// этой предвспышки, EV. Причина: сенсор и HAL квантуют выдержку и ISO (ISO целое, выдержка с шагом
/// строки), а мы сравниваем с произведением, которое сами получили ПОСЛЕ округления ISO; 1/8 EV
/// (≈ 9 %) покрывает квантование с запасом, но отличает кадр с прежней экспозицией (разница
/// в EV и больше, а после перемера — ровно шаг конфига, ≥ 0,5 EV).
const PREFLASH_PRODUCT_MATCH_TOLERANCE_EV: f64 = 0.125;

/// Решение по очередному замеру с фонариком.
enum Judgement {
    /// Замер не наш или не годится: ждём дальше.
    Ignore,
    /// Замер пересвечен: уменьшить экспозицию и перемерить. Если уменьшать уже некуда (пределы камеры),
    /// считаем кадр по этому замеру, когда есть замер без фонарика (`ambient`).
    Remeasure {
        torch: PreflashSample,
        ambient: Option<PreflashSample>,
    },
    /// Замер годится: считать кадр.
    Plan {
        torch: PreflashSample,
        ambient: PreflashSample,
    },
}

impl CameraEngine {
    /// Пределы дележа выдержки и ISO для активной камеры и потока (те же, что у нашего AE).
    pub(super) fn flash_split_limits(&self) -> Option<SplitLimits> {
        let capabilities = self.planning_capabilities()?;
        let (iso_range, shutter_range) = manual_sensor_ranges(&capabilities)?;
        let frame_duration = self
            .desired
            .mode
            .map_or_else(|| FrameDuration::from_nanos(0), |mode| mode.frame_rate.frame_duration());
        Some(SplitLimits {
            iso_min: iso_range.min,
            iso_max: iso_range.max,
            shutter_shortest: shutter_range.shortest,
            shutter_cap: longest_usable_shutter(shutter_range, frame_duration),
        })
    }

    /// Экспозиция предвспышки для произведения `wanted`: свободный делёж, без защиты от мерцания.
    fn preflash_exposure_for(limits: SplitLimits, wanted: ExposureProduct) -> Option<PreflashExposure> {
        let divided = split(wanted.seconds_iso(), Anchor::Free, limits, None::<Duration>);
        let exposure_time = ExposureTime::new(divided.shutter).ok()?;
        let iso = Iso::new(divided.iso).ok()?;
        Some(PreflashExposure {
            exposure_time,
            iso,
            product: ExposureProduct::from_sensor(exposure_time, iso),
        })
    }

    /// Начать последовательность по принятой команде: фонарик в повторяющийся запрос.
    pub(super) fn begin_flash_preflash(&mut self, shot: FlashShotId, now: Instant) {
        match self.prepare_flash_run(shot, now) {
            Ok(run) => {
                self.flash.run = Some(run);
                // Сначала рендер узнаёт, что мерить каждый кадр, потом запрос с фонариком уходит в HAL.
                self.three_a
                    .update_flash_directive(|directive| directive.preflash = PreflashMetering::Active { shot });
                self.reconcile(now);
            }
            Err(failure) => {
                self.flash.counters.failed += 1;
                self.emit_flash_report(crate::flash_status::FlashStillReport::Failed { shot, failure });
            }
        }
    }

    /// Собрать начальное состояние предвспышки; `Err` — условия принятой команды уже не выполнены.
    fn prepare_flash_run(&mut self, shot: FlashShotId, now: Instant) -> Result<FlashRun, FlashStillFailure> {
        let camera = self
            .device
            .as_ref()
            .map(|device| device.camera.clone())
            .ok_or(FlashStillFailure::NotStreaming)?;
        // Текущий повторяющийся запрос нашего AE: из него берётся произведение, которое ограничит предвспышку.
        let applied_product = match self.session.as_ref().and_then(|session| session.repeating) {
            Some(request) => match request.controls.exposure {
                ExposurePlan::Sensor { exposure_time, iso } => ExposureProduct::from_sensor(exposure_time, iso),
                ExposurePlan::VendorAuto { .. } => return Err(FlashStillFailure::NeedsOwnExposure),
            },
            None => return Err(FlashStillFailure::NotStreaming),
        };
        let limits = self.flash_split_limits().ok_or(FlashStillFailure::NeedsManualSensor)?;
        let config = self.config.three_a.flash;
        // Фонарик на близком объекте при экспозиции тёмной сцены пересветил бы замер: потолок из конфига.
        let ceiling = ExposureProduct::from_seconds_iso(config.preflash_max_log2_seconds_iso.exp2())
            .map_err(|_| FlashStillFailure::PreflashNotMeasured)?;
        let wanted = if applied_product < ceiling {
            applied_product
        } else {
            ceiling
        };
        let exposure = Self::preflash_exposure_for(limits, wanted).ok_or(FlashStillFailure::NeedsManualSensor)?;
        // Забрать то, что лежит в ящике сейчас: замер без фонарика может уже быть, а замеры с фонариком
        // прошлого снимка отсекает метка кадра.
        let tag = CameraTag::of(&camera);
        let request_floor = self.three_a.latest_result_timestamp();
        let mut ambient = None;
        if let Some((generation, samples)) = self.three_a.preflash_samples_newer_than(self.flash.samples_seen) {
            self.flash.samples_seen = Some(generation);
            ambient = samples
                .ambient
                .filter(|sample| Preflash::is_pre_request_ambient(sample, tag, request_floor));
        }
        tracing::debug!(
            target: LOG_TARGET,
            ?shot,
            preflash_product = exposure.product.seconds_iso(),
            exposure_ms = exposure.exposure_time.duration().as_secs_f64() * 1e3,
            iso = exposure.iso.value(),
            request_floor = ?request_floor,
            has_ambient = ambient.is_some(),
            "предвспышка начата"
        );
        Ok(FlashRun {
            shot,
            camera,
            accepted_at: now,
            stage: Stage::Preflash(Preflash {
                remeasures: 0,
                deadline: now + Duration::from_millis(config.preflash_timeout_ms),
                exposure,
                torch_frame_floor: request_floor,
                request_floor,
                ambient,
            }),
        })
    }

    /// Забрать новые замеры предвспышки и продвинуть последовательность. Зовётся по сигналу пробуждения.
    pub(in crate::engine) fn poll_flash_samples(&mut self, now: Instant) {
        if !matches!(self.flash.run.as_ref().map(|run| run.stage), Some(Stage::Preflash(_))) {
            return;
        }
        let Some((generation, samples)) = self.three_a.preflash_samples_newer_than(self.flash.samples_seen) else {
            return;
        };
        self.flash.samples_seen = Some(generation);
        match self.judge_samples(samples) {
            Judgement::Ignore => {}
            Judgement::Remeasure { torch, ambient } => self.remeasure_preflash(torch, ambient, now),
            Judgement::Plan { torch, ambient } => self.plan_flash_shot(torch, ambient, now),
        }
    }

    /// Принять замеры в состояние предвспышки и решить, что с ними делать. Само решение чистое.
    fn judge_samples(&mut self, samples: PreflashSamples) -> Judgement {
        let max_remeasures = self.config.three_a.flash.max_preflash_remeasures;
        let max_ambient_age = Duration::from_millis(self.config.three_a.flash.ambient_sample_max_age_ms);
        let Some(FlashRun {
            camera,
            stage: Stage::Preflash(preflash),
            ..
        }) = self.flash.run.as_mut()
        else {
            return Judgement::Ignore;
        };
        let tag = CameraTag::of(camera);
        // Замеры без фонарика копим, но только снятые ДО запроса фонарика (см. `Preflash::request_floor`):
        // более поздние кадры могут быть частично освещены и выдали бы вклад фонарика за свет сцены.
        // Замер, снятый до запроса, но дошедший позже (запаздывание замера), заменяет более старый.
        if let Some(ambient) = samples.ambient
            && Preflash::is_pre_request_ambient(&ambient, tag, preflash.request_floor)
            && preflash.ambient.is_none_or(|known| ambient.frame > known.frame)
        {
            preflash.ambient = Some(ambient);
        }
        let Some(torch) = samples.torch else {
            return Judgement::Ignore;
        };
        let is_fresh = preflash.torch_frame_floor.is_none_or(|floor| torch.frame > floor);
        let product_gap_ev = torch.frame_product.ev_above(preflash.exposure.product);
        let is_own_camera = torch.measured_on == tag;
        let is_ours = is_own_camera && product_gap_ev.abs() <= PREFLASH_PRODUCT_MATCH_TOLERANCE_EV;
        if !is_fresh || !is_ours {
            // Причина отказа — в отладочный лог: без неё «предвспышка не измерена» на телефоне не разобрать.
            tracing::debug!(
                target: LOG_TARGET,
                frame = ?torch.frame,
                is_fresh,
                is_own_camera,
                product_gap_ev,
                frame_product = torch.frame_product.seconds_iso(),
                wanted_product = preflash.exposure.product.seconds_iso(),
                "замер с фонариком отвергнут"
            );
            return Judgement::Ignore;
        }
        // Замер без фонарика должен быть из той же сцены: старше срока — другой кадр жизни. Он снят до
        // запроса, а замер с фонариком — после него, поэтому разница неотрицательна; нарушение порядка
        // меток (`None`) считаем негодным замером.
        let ambient = preflash.ambient.filter(|ambient| {
            torch
                .frame
                .duration_since(ambient.frame)
                .is_some_and(|gap| gap <= max_ambient_age)
        });
        if torch.quality == MeasurementQuality::ClippedHighlights && preflash.remeasures < max_remeasures {
            return Judgement::Remeasure { torch, ambient };
        }
        match ambient {
            Some(ambient) => Judgement::Plan { torch, ambient },
            // Ждём следующий кадр с фонариком; срок предвспышки не продлевается.
            None => {
                tracing::debug!(
                    target: LOG_TARGET,
                    torch_frame = ?torch.frame,
                    known_ambient = ?preflash.ambient.map(|sample| sample.frame),
                    request_floor = ?preflash.request_floor,
                    "замер с фонариком годен, но нет замера без фонарика до запроса фонарика"
                );
                Judgement::Ignore
            }
        }
    }

    /// Замер с фонариком пересвечен: уменьшить экспозицию предвспышки и ждать новый замер. Если пределы
    /// камеры не дают уменьшить (делёж выдержки и ISO даёт ту же экспозицию), перемер ничего не изменил
    /// бы: считаем кадр по пересвеченному замеру, если есть замер без фонарика, иначе ждём его.
    fn remeasure_preflash(&mut self, torch: PreflashSample, ambient: Option<PreflashSample>, now: Instant) {
        let step_ev = self.config.three_a.flash.preflash_remeasure_step_ev;
        let extension = Duration::from_millis(self.config.three_a.flash.preflash_timeout_ms);
        let Some(limits) = self.flash_split_limits() else {
            self.fail_flash_keeping_stream(FlashStillFailure::NeedsManualSensor, now);
            return;
        };
        let Some(FlashRun {
            stage: Stage::Preflash(preflash),
            ..
        }) = self.flash.run.as_mut()
        else {
            return;
        };
        let reduced = preflash
            .exposure
            .product
            .scaled_by_ev(-step_ev)
            .and_then(|wanted| Self::preflash_exposure_for(limits, wanted));
        let Some(exposure) = reduced else {
            self.fail_flash_keeping_stream(FlashStillFailure::PreflashNotMeasured, now);
            return;
        };
        if exposure.is_same_request_as(&preflash.exposure) {
            // Нижняя граница камеры: тот же запрос дал бы тот же пересвет, а срок ушёл бы впустую.
            if let Some(ambient) = ambient {
                self.plan_flash_shot(torch, ambient, now);
            }
            return;
        }
        preflash.exposure = exposure;
        preflash.remeasures += 1;
        // Кадры до перестройки запроса несли прежнюю экспозицию; всё, что не новее этого кадра, не берём.
        preflash.torch_frame_floor = Some(torch.frame);
        // Новому запросу нужны свои 7–8 кадров до освещённого кадра и замер: срок продлевается.
        preflash.deadline += extension;
        self.flash.counters.preflash_remeasures += 1;
        tracing::debug!(target: LOG_TARGET, step_ev, "замер с фонариком пересвечен: перемеряем с меньшей экспозицией");
        self.reconcile(now);
    }

    /// Годные замеры есть: посчитать кадр и перейти к паузе без фонарика.
    fn plan_flash_shot(&mut self, torch: PreflashSample, ambient: PreflashSample, now: Instant) {
        let Some(limits) = self.flash_split_limits() else {
            self.fail_flash_keeping_stream(FlashStillFailure::NeedsManualSensor, now);
            return;
        };
        let inputs = FlashPlanInputs {
            shot: self.flash.run.as_ref().map_or(FlashShotId(0), |run| run.shot),
            ambient_frame: ambient.frame,
            ambient: ambient.mid_gray_product,
            torch: torch.mid_gray_product,
            torch_to_flash_ratio: self.config.three_a.flash.torch_to_flash_ratio,
            bias_ev: f64::from(self.desired.three_a.controls.exposure_bias.ev()),
            limits,
        };
        let Some(planned) = plan_flash_frame(&inputs) else {
            self.fail_flash_keeping_stream(FlashStillFailure::PreflashNotMeasured, now);
            return;
        };
        // Известное ограничение: владелец предпочитает снимок отказу, поэтому после исчерпания перемеров
        // стреляем и по пересвеченному замеру (вклад фонарика занижен, кадр может выйти ярче цели).
        if torch.quality == MeasurementQuality::ClippedHighlights {
            self.flash.counters.planned_from_clipped_torch += 1;
        }
        let gap = Duration::from_millis(self.config.three_a.flash.torch_off_to_single_gap_ms);
        let Some(run) = self.flash.run.as_mut() else {
            return;
        };
        run.stage = Stage::Gap {
            planned,
            submit_at: now + gap,
        };
        // Рендер запоминает план по номеру снимка; мерить каждый кадр ему больше не нужно.
        self.three_a.update_flash_directive(|directive| {
            directive.preflash = PreflashMetering::Inactive;
            directive.planned = Some(planned.plan);
        });
        // Повторяющийся запрос без фонарика: пауза нужна именно с выключенным светодиодом (факт 14d).
        self.reconcile(now);
    }

    /// Фонарик в повторяющемся запросе нужен только в предвспышке; единственное место, где он ставится.
    pub(in crate::engine) fn apply_flash_to_plan(&self, planned: &mut crate::engine::three_a::PlannedRequest) {
        let Some((exposure_time, iso, flash)) = self.flash.repeating_override() else {
            return;
        };
        planned.controls.exposure = ExposurePlan::Sensor { exposure_time, iso };
        planned.controls.flash = flash;
        // Поправки и достижимый отрезок относились к плану нашего AE, а не к экспозиции последовательности.
        planned.adjustments.clear();
        planned.exposure_reach = camera_contract::auto_exposure::ProductReach::Unbounded;
    }
}
