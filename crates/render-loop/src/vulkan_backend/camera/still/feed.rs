//! Лента от видоискателя к снимкам: что показано под каждым `present_id` и с какими параметрами
//! обработан каждый кадр.
//!
//! Видоискатель (`CameraSession`) — единственный, кто знает, что именно легло на экран и какую белую точку и
//! подтяжку `k` получил кадр. Снимку это нужно ПОВТОРИТЬ (решение 72), поэтому сессия складывает факты в
//! [`StillFeedRecorder`], а бэкенд раз в кадр забирает их ([`StillFeedRecorder::take`]) и передаёт движку снимков.
//! Так снимок не заглядывает в поля сессии, а сессия не знает, кто и как читает ленту.

use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::FlashFramePlan;
use camera_contract::frame_passport::StreamPassport;
use isp_model::color::chromaticity::Chromaticity;

use crate::vulkan_backend::camera::flash_facts::AmbientLevel;
use isp_model::exposure::ExposureGain;

/// Что видоискатель повторил для кадра: снимок берёт ровно это (решение 72).
#[derive(Debug, Clone)]
pub struct FrameStillParams {
    /// Поток кадра: параметры годны только его кадрам (сравнение по `Arc::ptr_eq`, решение 42).
    pub stream: Arc<StreamPassport>,
    /// Белая точка, с которой обработан кадр (решение нашего AWB, не подсказка HAL).
    pub white: Chromaticity,
    /// Остаточная подтяжка нашего AE (`k`); `UNITY` — вендорский AE или подтяжки не было.
    pub gain: ExposureGain,
}

/// Что лежало на экране под одним показом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShownContent {
    /// Живой кадр камеры с этой меткой сенсора (новый или перекомпоновка уже показанного).
    Live(SensorTimestamp),
    /// Не живой кадр: удержанная картинка смены объектива, чёрный, отладочная мозаика.
    NotLive,
}

/// Запись «показ → что было на экране».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownRecord {
    /// Номер показа цепочки (тот же, что в `PastPresentation::present_id`).
    pub present_id: u32,
    pub content: ShownContent,
}

/// Что сессия записала в принятый сабмит (вход [`StillFeedRecorder::note_present`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShownNow {
    /// Новый кадр прошёл ISP и лёг на экран живым.
    NewLiveFrame(SensorTimestamp),
    /// Перекомпоновка: на экране снова последний живой кадр.
    RecomposedLive,
    /// На экране не живой кадр.
    NotLive,
}

/// Накопленное за кадры с прошлого `take`.
#[derive(Debug, Default)]
pub struct StillFeed {
    /// Счётчик показов начался заново (цепочку пересоздали): прежние `present_id` больше ничьи.
    pub presents_restarted: bool,
    /// Показы по порядку отправки.
    pub presents: Vec<ShownRecord>,
    /// Параметры обработанных кадров по порядку обработки.
    pub params: Vec<(SensorTimestamp, FrameStillParams)>,
    /// Планы кадра со вспышкой из указаний движка камеры (14d), по порядку появления.
    pub flash_plans: Vec<FlashFramePlan>,
    /// Уровни света сцены без вспышки по кадрам (14d), по порядку измерения.
    pub ambient_levels: Vec<AmbientLevel>,
}

impl StillFeed {
    /// Пуста ли лента (тесты).
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        !self.presents_restarted
            && self.presents.is_empty()
            && self.params.is_empty()
            && self.flash_plans.is_empty()
            && self.ambient_levels.is_empty()
    }
}

/// Писатель ленты внутри сессии показа.
#[derive(Debug, Default)]
pub struct StillFeedRecorder {
    pending: StillFeed,
    /// Последний живой кадр на экране: по нему расшифровывается перекомпоновка.
    last_live: Option<SensorTimestamp>,
}

impl StillFeedRecorder {
    /// Кадр с этой меткой обработан видоискателем с такими параметрами.
    pub fn note_frame_params(&mut self, sensor_timestamp: SensorTimestamp, params: FrameStillParams) {
        self.pending.params.push((sensor_timestamp, params));
    }

    /// Движок камеры выбрал план кадра со вспышкой (14d).
    pub fn note_flash_plan(&mut self, plan: FlashFramePlan) {
        self.pending.flash_plans.push(plan);
    }

    /// Свет сцены без вспышки по кадру (14d): все уровни, а не последний — план ссылается на кадр.
    pub fn note_ambient_level(&mut self, level: AmbientLevel) {
        self.pending.ambient_levels.push(level);
    }

    /// Принятый сабмит показал `shown` под номером `present_id`.
    pub fn note_present(&mut self, present_id: u32, shown: ShownNow) {
        let content = match shown {
            ShownNow::NewLiveFrame(sensor_timestamp) => {
                self.last_live = Some(sensor_timestamp);
                ShownContent::Live(sensor_timestamp)
            }
            // Перекомпоновка без известного живого кадра — не живая: выдумывать кадр нельзя.
            ShownNow::RecomposedLive => self.last_live.map_or(ShownContent::NotLive, ShownContent::Live),
            ShownNow::NotLive => {
                self.last_live = None;
                ShownContent::NotLive
            }
        };
        self.pending.presents.push(ShownRecord { present_id, content });
    }

    /// Цепочку показа пересоздали: номера показов начались заново.
    pub fn note_presents_restarted(&mut self) {
        self.pending.presents.clear();
        self.pending.presents_restarted = true;
        self.last_live = None;
    }

    /// Забирает накопленное (лента становится пустой).
    pub fn take(&mut self) -> StillFeed {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
#[path = "feed_tests.rs"]
mod tests;
