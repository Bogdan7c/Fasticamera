//! Замеры предвспышки и факты для кадра со вспышкой на стороне AE (сессия 14d, решения 158, 159).
//!
//! # Что здесь решается
//! * Какой свет у кадра, чья статистика ушла в замер, знают только в момент отправки (замер приходит позже и
//!   асинхронно, а в его метке света нет): [`PreflashMeter`] запоминает свет по метке сенсора до ответа.
//! * Для каждого измеренного кадра БЕЗ светодиода (`Ambient`) и С фонариком (`TorchLit`) собирается линейный
//!   [`PreflashSample`]: `mid_gray_product = P_кадра · цель / линейное среднее`
//!   (`scene_estimation::exposure::linear_mid_gray_product`, формула — в его модуле). Остальные кадры
//!   (частичная вспышка, разовая вспышка, неизвестный свет) замера предвспышки не дают.
//! * Замеры кладутся в ящик ЦЕЛИКОМ ([`PreflashSamples`]: отдельные места «без фонарика» / «с фонариком»).
//!   Кадры без светодиода публикуются ВСЕГДА, а не только во время предвспышки: движок запоминает свет сцены
//!   в начале предвспышки, и этот замер должен лежать в ящике ДО её начала. Цена — один небольшой слот и
//!   толчок движку на каждый измеренный кадр (замеры идут по расписанию статистики, не на каждом кадре).
//!
//! Кадры, освещённые светодиодом, в контроллер AE видоискателя не попадают (см. `ExposureRunner::collect_measurements`):
//! иначе экспозиция видоискателя уехала бы к свету фонарика.

use std::collections::VecDeque;
use std::sync::Arc;

use camera_contract::auto_exposure::CameraTag;
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::{FlashFramePlan, FrameLight, PreflashLight, PreflashSample, PreflashSamples};
use camera_contract::frame_passport::StreamPassport;
use scene_estimation::exposure::{CompletedExposureMeasurement, ExposureTarget, linear_mid_gray_product};

use crate::vulkan_backend::camera::flash_facts::AmbientLevel;

/// Сколько кадров с отправленной, но не вернувшейся статистикой помнить. Исполнитель замера держит их
/// единицы; больше — потеря ответов, и лишнее забывается.
const MAX_IN_FLIGHT_FRAMES: usize = 8;

/// Сколько неотданных уровней света сцены держать: исполнитель замера возвращает единицы кадров, а лента
/// читается каждый кадр; больше — потеря чтения, и старое забывается.
const MAX_PENDING_AMBIENT_LEVELS: usize = 32;

/// Свет кадра, статистика которого ушла в замер.
#[derive(Debug, Clone, Copy)]
struct InFlightFrame {
    sensor_timestamp_nanos: i64,
    light: FrameLight,
}

/// Что вышло из готового замера кадра для вспышки.
#[derive(Debug, PartialEq)]
pub(super) struct PreflashOutcome {
    /// Свет кадра (по нему решается, питать ли контроллер AE видоискателя).
    pub(super) light: FrameLight,
    /// Новое состояние ящика замеров, если кадр дал замер предвспышки.
    pub(super) publish: Option<PreflashSamples>,
}

/// Замеры предвспышки и факты для движка снимков (см. модуль).
#[derive(Default)]
pub(super) struct PreflashMeter {
    in_flight: VecDeque<InFlightFrame>,
    samples: PreflashSamples,
    /// Уровни света сцены без вспышки, ещё не отданные движку снимков, по порядку кадров. Отдаются ВСЕ, а не
    /// последний: план вспышки ссылается на уровень конкретного кадра (`FlashFramePlan::ambient_frame`).
    pending_ambient_levels: Vec<AmbientLevel>,
    /// План кадра со вспышкой, который уже передан движку снимков (повтор не передаётся).
    forwarded_plan: Option<FlashFramePlan>,
    pending_plans: Vec<FlashFramePlan>,
    /// Сколько замеров предвспышки опубликовано.
    pub(super) samples_published: u64,
    /// Сколько кадров с готовым замером не дали линейного замера (`linear_mid_gray_product` вернул `None`:
    /// нулевая яркость или не число): без счётчика такой кадр был бы тихо потерян.
    pub(super) not_computable: u64,
    /// Сколько готовых замеров не попало в контроллер AE видоискателя из-за света светодиода.
    pub(super) withheld_from_controller: u64,
}

impl PreflashMeter {
    /// Статистика кадра `sensor_timestamp_nanos` ушла в замер при свете `light`.
    pub(super) fn note_submitted(&mut self, sensor_timestamp_nanos: i64, light: FrameLight) {
        if self.in_flight.len() >= MAX_IN_FLIGHT_FRAMES {
            self.in_flight.pop_front();
        }
        self.in_flight.push_back(InFlightFrame {
            sensor_timestamp_nanos,
            light,
        });
    }

    /// Новый поток: свет кадров и замеры старого потока чужие.
    pub(super) fn reset_for_new_stream(&mut self) {
        self.in_flight.clear();
        self.samples = PreflashSamples::default();
        self.pending_ambient_levels.clear();
    }

    /// Разбирает готовый замер: достаёт свет кадра (неизвестный кадр — обычный свет, как до вспышки) и, если
    /// кадр дал замер предвспышки, обновляет ящик.
    pub(super) fn complete(
        &mut self,
        done: &CompletedExposureMeasurement,
        stream: &Arc<StreamPassport>,
        target: ExposureTarget,
        key_offset_ev: f32,
    ) -> PreflashOutcome {
        let light = self
            .in_flight
            .iter()
            .position(|frame| frame.sensor_timestamp_nanos == done.frame.sensor_timestamp_nanos)
            .and_then(|index| self.in_flight.remove(index))
            .map_or(FrameLight::Ambient, |frame| frame.light);
        let publish = self.record(done, light, stream, target, key_offset_ev);
        PreflashOutcome { light, publish }
    }

    fn record(
        &mut self,
        done: &CompletedExposureMeasurement,
        light: FrameLight,
        stream: &Arc<StreamPassport>,
        target: ExposureTarget,
        key_offset_ev: f32,
    ) -> Option<PreflashSamples> {
        let kind = match light {
            FrameLight::Ambient => PreflashLight::Ambient,
            FrameLight::TorchLit => PreflashLight::Torch,
            FrameLight::FlashFired | FrameLight::PartiallyLit | FrameLight::Unknown => return None,
        };
        let measurement = done.outcome.as_ref().ok()?;
        let Some(mid_gray_product) = linear_mid_gray_product(measurement, done.frame_product, target, key_offset_ev)
        else {
            // Нулевая яркость или не число: линейного замера нет, но это не молчание.
            self.not_computable += 1;
            return None;
        };
        let frame = SensorTimestamp::from_nanos(done.frame.sensor_timestamp_nanos);
        self.samples.record(PreflashSample {
            frame,
            measured_on: CameraTag::of(&stream.camera),
            light: kind,
            frame_product: done.frame_product,
            mid_gray_product,
            quality: measurement.quality,
        });
        self.samples_published += 1;
        if kind == PreflashLight::Ambient {
            // Лишнее забываем с головы: движок снимков читает ленту каждый кадр, копиться могут единицы.
            if self.pending_ambient_levels.len() >= MAX_PENDING_AMBIENT_LEVELS {
                self.pending_ambient_levels.remove(0);
            }
            self.pending_ambient_levels.push(AmbientLevel {
                stream: Arc::clone(stream),
                frame,
                green_per_product: f64::from(measurement.mean_green_level) / done.frame_product.seconds_iso(),
            });
        }
        Some(self.samples)
    }

    /// План кадра со вспышкой из указания движка: передаётся движку снимков один раз.
    pub(super) fn observe_plan(&mut self, planned: Option<FlashFramePlan>) {
        if let Some(plan) = planned
            && self.forwarded_plan != Some(plan)
        {
            self.forwarded_plan = Some(plan);
            self.pending_plans.push(plan);
        }
    }

    /// Новые планы кадра со вспышкой (список становится пустым).
    pub(super) fn drain_plans(&mut self) -> Vec<FlashFramePlan> {
        std::mem::take(&mut self.pending_plans)
    }

    /// Новые уровни света сцены без вспышки с прошлого раза (список становится пустым).
    pub(super) fn drain_ambient_levels(&mut self) -> Vec<AmbientLevel> {
        std::mem::take(&mut self.pending_ambient_levels)
    }
}
