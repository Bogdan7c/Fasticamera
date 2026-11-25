//! Исполнитель детектора «сцена под точкой тапа сменилась» на потоке рендера (решение 241).
//!
//! # Кто чем владеет
//! * `scene_estimation::exposure::SceneChangeDetector` — чистая арифметика: эталон, сравнение, подтверждение;
//! * [`SceneWatch`] — решает, КОГДА детектор кормить (какие кадры и при каком указании), и превращает подтверждение
//!   в наблюдение [`PointObsolete`] для движка (рендер → движок). Точку снимает и блокировку учитывает сам движок.
//!
//! # Инварианты
//! 1. Кормим только пока указание несёт точку тапа и блокировки нет: без точки следить не за чем, а под блокировкой
//!    точка живёт по воле владельца. Иначе детектор забывает эталон (после разблокировки он снимается заново).
//! 2. Кадры со светом светодиода и кадры без спаренной выдержки/ISO детектор не видят (их свет не сопоставим),
//!    но и слежение не сбрасывают.
//! 3. Наблюдение публикуется один раз на номер тапа; если движок точку не снял (вспышка, серия снимков), повтор —
//!    не чаще раза в `confirm_ms`, пока условие держится: потерянное наблюдение иначе залипло бы навсегда.
//! 4. Наблюдение «регион вне кадра» (зум увёл место тапа за край) не зависит от блокировки: снимать ли точку под
//!    блокировкой, решает движок, а рендер лишь сообщает факт.
//! 5. Расписание статистики не меняется: детектор кормится теми кадрами, которые и так посчитаны для замера.

use camera_contract::auto_exposure::{CameraTag, ExposureDirective};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::FrameLight;
use camera_contract::scene_change::{PointObsolete, PointObsoleteCause};
use camera_contract::three_a::{AeAfLock, PointOfInterest, PointRequestId};
use isp_model::statistics::AwbStatistics;
use scene_estimation::error::ConfigError;
use scene_estimation::exposure::{SceneChangeConfig, SceneChangeDetector, SceneFrame, SceneWatchStep};

use super::{ExposureFrameFacts, ExposureRunner, ExposureStream, LOG_TARGET, WeightsKey, WeightsUnavailable};
use crate::viewfinder::AwbFrameIdentity;
use camera_contract::auto_exposure::MeteredArea;

/// Кадр, которым кормят слежение.
pub(super) struct WatchedFrame<'a> {
    /// Действующее указание движка.
    pub(super) directive: &'a ExposureDirective,
    /// Факты кадра (произведение, кадрирование, свет).
    pub(super) facts: &'a ExposureFrameFacts,
    /// Статистика кадра.
    pub(super) statistics: &'a AwbStatistics,
    /// Метка сенсора кадра.
    pub(super) sensor_timestamp: SensorTimestamp,
    /// Камера кадра: движок не принимает наблюдение с другой камеры.
    pub(super) camera: CameraTag,
}

/// Счётчики слежения (в диагностику AE).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SceneWatchCounters {
    /// Эталонов снято.
    pub(super) references_taken: u64,
    /// Кадров с подтверждённой сменой сцены.
    pub(super) changes_confirmed: u64,
    /// Наблюдений опубликовано (обоих видов).
    pub(super) published: u64,
}

/// Когда и для какого тапа наблюдение публиковалось в последний раз.
#[derive(Debug, Clone, Copy)]
struct LastPublication {
    request: PointRequestId,
    at: SensorTimestamp,
}

/// Слежение за сценой под точкой тапа (см. модуль).
pub(super) struct SceneWatch {
    detector: SceneChangeDetector,
    /// Не чаще одного повтора наблюдения за столько наносекунд (по меткам сенсора).
    repeat_after_nanos: i64,
    last_publication: Option<LastPublication>,
    counters: SceneWatchCounters,
}

impl SceneWatch {
    pub(super) fn new(config: SceneChangeConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            detector: SceneChangeDetector::new(config)?,
            repeat_after_nanos: i64::from(config.confirm_ms) * 1_000_000,
            last_publication: None,
            counters: SceneWatchCounters::default(),
        })
    }

    pub(super) fn counters(&self) -> SceneWatchCounters {
        self.counters
    }

    /// Новый поток камеры: эталон и память о публикациях относятся к прежней сцене.
    pub(super) fn reset_for_new_stream(&mut self) {
        self.detector.reset();
        self.last_publication = None;
    }

    /// Кадр статистики: наблюдение «сцена сменилась» для публикации, если оно подтвердилось и пора.
    pub(super) fn observe_frame(&mut self, frame: &WatchedFrame<'_>) -> Option<PointObsolete> {
        let PointOfInterest::Point { request, .. } = frame.directive.metering else {
            self.detector.reset();
            return None;
        };
        if frame.directive.lock != AeAfLock::Unlocked {
            self.detector.reset();
            return None;
        }
        let (FrameLight::Ambient, Some(product)) = (frame.facts.light, frame.facts.product) else {
            return None;
        };
        let step = self.detector.observe(&SceneFrame {
            request,
            statistics: frame.statistics,
            product,
            sensor_timestamp: frame.sensor_timestamp,
            framing: frame.facts.crop,
        });
        match step {
            SceneWatchStep::ReferenceTaken(reason) => {
                self.counters.references_taken += 1;
                tracing::debug!(target: LOG_TARGET, request = request.0, ?reason, "эталон сцены под точкой снят");
                None
            }
            SceneWatchStep::Confirmed(evidence) => {
                self.counters.changes_confirmed += 1;
                let observation = self.publish_if_due(request, frame, PointObsoleteCause::SceneChanged)?;
                tracing::info!(
                    target: LOG_TARGET,
                    request = request.0,
                    changed_cells = evidence.changed_cells,
                    voting_cells = evidence.voting_cells,
                    "сцена под точкой тапа сменилась: точка устарела"
                );
                Some(observation)
            }
            SceneWatchStep::Ignored
            | SceneWatchStep::AwaitingReference
            | SceneWatchStep::Similar
            | SceneWatchStep::Inconclusive
            | SceneWatchStep::Diverging => None,
        }
    }

    /// Регион точки лежит вне сетки статистики (зум увёл место тапа за край кадра): замеру нечего мерить.
    pub(super) fn observe_region_outside_frame(&mut self, frame: &WatchedFrame<'_>) -> Option<PointObsolete> {
        let PointOfInterest::Point { request, .. } = frame.directive.metering else {
            return None;
        };
        let observation = self.publish_if_due(request, frame, PointObsoleteCause::RegionOutsideFrame)?;
        tracing::info!(
            target: LOG_TARGET,
            request = request.0,
            "регион точки тапа вне кадра: точка устарела"
        );
        Some(observation)
    }

    /// Наблюдение, если для этого тапа ещё не публиковали или с прошлой публикации прошло достаточно времени.
    fn publish_if_due(
        &mut self,
        request: PointRequestId,
        frame: &WatchedFrame<'_>,
        cause: PointObsoleteCause,
    ) -> Option<PointObsolete> {
        let repeated_too_soon = self.last_publication.is_some_and(|last| {
            last.request == request
                && frame.sensor_timestamp.as_nanos().saturating_sub(last.at.as_nanos()) < self.repeat_after_nanos
        });
        if repeated_too_soon {
            return None;
        }
        self.last_publication = Some(LastPublication {
            request,
            at: frame.sensor_timestamp,
        });
        self.counters.published += 1;
        Some(PointObsolete {
            request,
            measured_on: frame.camera,
            frame: frame.sensor_timestamp,
            cause,
        })
    }
}

impl ExposureRunner {
    /// Установлено ли для области `key` (поток, кадрирование, тап), что регион лежит вне сетки статистики.
    fn region_is_outside_grid_for(&self, key: WeightsKey) -> bool {
        self.weights.as_ref().is_some_and(|cached| {
            cached.key == key && matches!(cached.weights, Err(WeightsUnavailable::RegionOutsideGrid))
        })
    }

    /// Кормит слежение кадром статистики. Вызывается из `ingest_statistics` ДО ранних выходов замера (нет веса
    /// области, ручная экспозиция, нет пары): слежение от контроллера и его `Holding` не зависит.
    pub(super) fn feed_scene_watch(
        &mut self,
        frame: AwbFrameIdentity,
        facts: &ExposureFrameFacts,
        statistics: &AwbStatistics,
    ) {
        let Some(bound) = &self.bound else { return };
        let watched = WatchedFrame {
            directive: &self.directive,
            facts,
            statistics,
            sensor_timestamp: SensorTimestamp::from_nanos(frame.sensor_timestamp_nanos),
            camera: CameraTag::of(&bound.passport.camera),
        };
        if let Some(observation) = self.scene_watch.observe_frame(&watched) {
            self.proposals.publish_point_obsolete(observation);
        }
    }

    /// Сообщает движку, что регион точки вышел за сетку статистики. Вызывается ПОСЛЕ постановки замера кадра:
    /// только она строит веса области. Публикуем лишь если «вне сетки» установлено для ТЕКУЩЕЙ области (веса с этим
    /// номером тапа, этим кадрированием и этим потоком), а не по флагу, оставшемуся от прошлой точки.
    pub(super) fn report_region_outside_frame(
        &mut self,
        stream: ExposureStream,
        frame: AwbFrameIdentity,
        facts: &ExposureFrameFacts,
        statistics: &AwbStatistics,
    ) {
        let current = WeightsKey {
            stream,
            crop: facts.crop,
            area: MeteredArea::for_point(self.directive.metering),
        };
        if !self.region_is_outside_grid_for(current) {
            return;
        }
        let Some(bound) = &self.bound else { return };
        let watched = WatchedFrame {
            directive: &self.directive,
            facts,
            statistics,
            sensor_timestamp: SensorTimestamp::from_nanos(frame.sensor_timestamp_nanos),
            camera: CameraTag::of(&bound.passport.camera),
        };
        if let Some(observation) = self.scene_watch.observe_region_outside_frame(&watched) {
            self.proposals.publish_point_obsolete(observation);
        }
    }
}

#[cfg(test)]
#[path = "scene_watch_tests.rs"]
mod tests;
