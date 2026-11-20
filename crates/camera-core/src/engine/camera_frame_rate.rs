//! Потолок частоты кадров по камерам (решение 23 + сессия 13).
//!
//! **Желаемый** режим остаётся таким, каким его запросил продукт (4:3@60). **Эффективный** режим
//! для конкретной камеры — запрошенный или сниженный до потолка, который эта камера уже показала:
//! фронтальная отдаёт только 30 к/с, основная — 60 через вендорские ключи CamX.
//!
//! Что именно камера умеет, знает только бэкенд (`frame_rate_plan` в `camera-android` решает
//! по вендорским ключам, а `ae_fps_ranges` в характеристиках для CamX-камеры занижены), поэтому
//! потолок узнаётся реактивно: запрошенная частота не прошла (`FrameRateNotAchievable`) → откат
//! на запасную (решение 23) → потолок камеры запоминается здесь. Возврат на другую камеру снова
//! просит запрошенные 60: потолок хранится **по камерам**, а не в желаемом режиме (иначе 30
//! фронтальной «прилипали» бы к основной — известное ограничение из `results/05.md`).
//!
//! Потолок сбрасывается новой командой `Configure` и `Close`: новый запрос стоит проверить заново.

use std::collections::HashMap;

use camera_contract::ids::CameraId;
use camera_contract::stream::{StreamMode, TargetFrameRate};

/// Потолки частоты по камерам.
///
/// Известное ограничение: потолок, записанный после `RejectedByHal`, живёт до `Configure` с
/// другим режимом или до `Close`. Если камера позже стала бы тянуть запрошенную частоту
/// (например, после сброса вендорского состояния), мы это не узнаем без новой команды.
#[derive(Debug, Default)]
pub(super) struct CameraFrameRateLimits {
    ceilings: HashMap<CameraId, TargetFrameRate>,
}

impl CameraFrameRateLimits {
    /// Эффективный режим камеры: запрошенный, но не выше известного потолка этой камеры.
    pub(super) fn effective_mode(&self, camera: &CameraId, requested: StreamMode) -> StreamMode {
        match self.ceilings.get(camera) {
            Some(ceiling) if requested.frame_rate > *ceiling => StreamMode {
                frame_rate: *ceiling,
                ..requested
            },
            _ => requested,
        }
    }

    /// Камера не потянула более высокую частоту: запомнить её потолок.
    pub(super) fn remember_ceiling(&mut self, camera: CameraId, ceiling: TargetFrameRate) {
        self.ceilings.insert(camera, ceiling);
    }

    /// Забыть все потолки (новый запрос режима или закрытие камеры).
    pub(super) fn forget_all(&mut self) {
        self.ceilings.clear();
    }

    /// Потолок камеры (для проверок и отчётов).
    #[cfg(test)]
    pub(super) fn ceiling_of(&self, camera: &CameraId) -> Option<TargetFrameRate> {
        self.ceilings.get(camera).copied()
    }
}
