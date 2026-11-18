//! Вспомогательный поток 3A («пустой поток», решение 62): настройки, решение и отчёт.
//!
//! Факт зонда 08: в сессии из одного RAW-потока Qualcomm не считает статистику 3A — нет
//! состояния AF, AE не сходится, баланс `[1,1,1,1]`. Достаточно объявить в сессии маленький YUV-поток и
//! **не слать в него ни одного запроса**: автоматика HAL оживает, 60 к/с и все RAW-кадры сохраняются.
//! Его пиксели не читаются никогда (решение 8).
//!
//! Вся вендорская особенность собрана здесь и в слое NDK (`android/companion`): остальной код о
//! потоке не знает. Не получилось добавить поток — сессия работает без него, а диагностика
//! честно показывает `unavailable` (AF/AE при этом мертвы — это видно, а не тихо).

use camera_contract::geometry::PixelSize;
use serde::{Deserialize, Serialize};

/// Настройки вспомогательного потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompanionStreamConfig {
    /// Добавлять ли поток в сессию. Выключить можно только ценой мёртвой автоматики HAL.
    pub enabled: bool,
    /// Ширина, пиксели. 640×480 — размер, проверенный зондом 08 на OnePlus 11.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
    /// `maxImages` читателя. Образов он не получает, но HAL выделяет буферы под это число;
    /// 4 — значение зонда 08.
    pub max_images: u32,
}

impl Default for CompanionStreamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 640,
            height: 480,
            max_images: 4,
        }
    }
}

/// Что не так с настройками потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CompanionConfigError {
    #[error("размер вспомогательного потока должен быть от 1 до {max} по каждой стороне, а не {width}×{height}")]
    SizeOutOfRange { width: u32, height: u32, max: u32 },
    #[error("max_images вспомогательного потока должно быть от 1 до {max}, а не {actual}")]
    MaxImagesOutOfRange { actual: u32, max: u32 },
}

impl CompanionStreamConfig {
    /// Верхняя граница стороны: поток «пустой», большой размер только тратит память HAL.
    pub const MAX_SIDE: u32 = 1920;
    /// Верхняя граница `maxImages`.
    pub const MAX_IMAGES_LIMIT: u32 = 8;

    pub fn validate(&self) -> Result<(), CompanionConfigError> {
        let side_ok = |side: u32| (1..=Self::MAX_SIDE).contains(&side);
        if !(side_ok(self.width) && side_ok(self.height)) {
            return Err(CompanionConfigError::SizeOutOfRange {
                width: self.width,
                height: self.height,
                max: Self::MAX_SIDE,
            });
        }
        if !(1..=Self::MAX_IMAGES_LIMIT).contains(&self.max_images) {
            return Err(CompanionConfigError::MaxImagesOutOfRange {
                actual: self.max_images,
                max: Self::MAX_IMAGES_LIMIT,
            });
        }
        Ok(())
    }

    /// Что просить у платформы; `None` — поток выключен настройкой.
    pub fn plan(&self) -> Option<CompanionStreamSpec> {
        self.enabled.then_some(CompanionStreamSpec {
            size: PixelSize::new(self.width, self.height),
            max_images: self.max_images,
        })
    }
}

/// Готовое задание на создание потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompanionStreamSpec {
    pub size: PixelSize,
    pub max_images: u32,
}

/// Почему вспомогательного потока в сессии нет, хотя он был нужен.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum CompanionUnavailableReason {
    /// Не создался читатель `AImageReader` (`media_status_t`).
    ReaderCreationFailed { status: i32 },
    /// Не добавился выход в контейнер сессии (`camera_status_t`).
    OutputRegistrationFailed { status: i32 },
    /// HAL отверг комбинацию потоков; сессия пересоздана без вспомогательного.
    RejectedByHal,
}

/// Состояние вспомогательного потока последней сессии — для диагностики.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum CompanionStreamStatus {
    /// Сессий ещё не было.
    #[default]
    NotConfigured,
    /// Выключен настройкой (AF/AE HAL при этом мертвы).
    DisabledByConfig,
    /// Поток в сессии.
    Active { width: u32, height: u32 },
    /// Нужен был, но в сессии его нет (AF/AE HAL мертвы).
    Unavailable { reason: CompanionUnavailableReason },
}

impl CompanionStreamStatus {
    /// Автоматика HAL при этом состоянии работать не будет.
    pub fn hal_three_a_is_dead(self) -> bool {
        matches!(self, Self::DisabledByConfig | Self::Unavailable { .. })
    }
}

#[cfg(test)]
#[path = "companion_stream_tests.rs"]
mod tests;
