//! Узкая граница 3A для вспышки (14d): то, что движку вспышки нужно знать и менять у нашего AE и
//! ящика указаний, не зная, как они устроены внутри `ThreeAEngine`.

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::{FlashDirective, PreflashSamples};
use camera_contract::three_a::DirectiveGeneration;

use super::ThreeAEngine;
use crate::three_a_control::{PlanRefresh, SceneForFlash};

impl ThreeAEngine {
    /// Идёт ли серия: вспышка и серия делят повторяющийся запрос и не идут одновременно.
    pub(in crate::engine) fn bracket_in_progress(&self) -> bool {
        self.bracket.is_some()
    }

    /// Свет сцены без вспышки у исполнителя экспозиции.
    pub(in crate::engine) fn scene_for_flash(&self) -> SceneForFlash {
        self.exposure.scene_for_flash()
    }

    /// Замеры предвспышки новее `seen`.
    pub(in crate::engine) fn preflash_samples_newer_than(
        &self,
        seen: Option<DirectiveGeneration>,
    ) -> Option<(DirectiveGeneration, PreflashSamples)> {
        self.exposure.preflash_samples_newer_than(seen)
    }

    /// Метка последнего увиденного результата кадра: все кадры до неё заведомо старше запроса, который
    /// выдан сейчас.
    pub(in crate::engine) fn latest_result_timestamp(&self) -> Option<SensorTimestamp> {
        self.last_result.and_then(|last| last.sensor_timestamp)
    }

    /// Дать нашему AE забрать решения, накопившиеся, пока вспышка держала экспозицию.
    pub(in crate::engine) fn catch_up_exposure(&mut self) -> PlanRefresh {
        self.exposure.refresh()
    }

    /// Изменить часть указания про вспышку (публикуется только при изменении).
    pub(in crate::engine) fn update_flash_directive(&mut self, update: impl FnOnce(&mut FlashDirective)) {
        self.exposure_directive.update_flash(update);
    }
}
