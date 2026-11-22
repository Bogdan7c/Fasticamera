//! Обмен рендера и записи файлов во время проявления снимка.
//!
//! Порядок для одного снимка:
//! 1. приложение отдаёт рендеру [`crate::plan::StillPlan`];
//! 2. рендер выбирает кадр, копирует его сырые данные и сразу возвращает буфер камере →
//!    [`DevelopEvent::Secured`] (затвор свободен, решение 140) или [`DevelopEvent::NotSecured`];
//! 3. запись открывает кодер и отвечает рендеру [`PrimaryTargetOffer`]: куда рисовать картинку —
//!    в поверхность кодера HEIF (решение 142) или в память для JPEG;
//! 4. рендер проявляет снимок по частям между кадрами видоискателя и сообщает
//!    [`DevelopEvent::PrimaryRendered`] / [`DevelopEvent::PrimaryReadback`] или [`DevelopEvent::Failed`].
//!
//! Каждый снимок, принятый рендером, кончается РОВНО одним итоговым событием: `NotSecured`,
//! `PrimaryRendered`, `PrimaryReadback` или `Failed` — чтобы запись не ждала вечно.

use shell_contract::surface::NativeSurface;

use crate::frame_choice::NoFrameReason;
use crate::plan::StillId;
use crate::shot::{RawSamples16, Rgba8Image, StillFrameFacts};

/// Куда рендеру отдать основную картинку.
pub enum PrimaryTarget {
    /// Входная поверхность кодера HEIF: рендер рисует один кадр и показывает его (решение 142).
    EncoderSurface(NativeSurface),
    /// Прочитать картинку в память (JPEG): итог — [`DevelopEvent::PrimaryReadback`].
    Readback,
}

impl std::fmt::Debug for PrimaryTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EncoderSurface(surface) => formatter.debug_tuple("EncoderSurface").field(surface).finish(),
            Self::Readback => formatter.write_str("Readback"),
        }
    }
}

/// Ответ записи на «кадр захвачен».
#[derive(Debug)]
pub enum PrimaryTargetOffer {
    /// Цель готова: рисуй сюда.
    Ready { id: StillId, target: PrimaryTarget },
    /// Цель открыть не удалось (кодер, хранилище): рендеру проявлять незачем, он отпускает снимок
    /// с итогом [`DevelopFailure::TargetUnavailable`].
    Unavailable { id: StillId },
}

impl PrimaryTargetOffer {
    pub fn id(&self) -> StillId {
        match self {
            Self::Ready { id, .. } | Self::Unavailable { id } => *id,
        }
    }
}

/// Почему проявление не удалось.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DevelopFailure {
    /// Параметры ISP для кадра не собрались (нет калибровки, неподдерживаемый поток).
    #[error("параметры ISP кадра недоступны: {0}")]
    ParamsUnavailable(String),
    /// Ошибка GPU: запись команд, сабмит, поверхность кодера.
    #[error("ошибка GPU при проявлении: {0}")]
    Gpu(String),
    /// Запись не смогла дать цель (см. [`PrimaryTargetOffer::Unavailable`]).
    #[error("цели для картинки нет")]
    TargetUnavailable,
    /// Цель не пришла за отведённое время.
    #[error("цель для картинки не пришла вовремя")]
    TargetTimedOut,
    /// Рендер останавливается или пересоздаёт GPU: незаконченный снимок брошен.
    #[error("рендер остановлен до конца проявления")]
    Abandoned,
}

/// События рендера для записи.
#[derive(Debug)]
pub enum DevelopEvent {
    /// Кадр выбран и скопирован, камере он больше не нужен. `raw` есть, если план просил DNG.
    Secured {
        facts: Box<StillFrameFacts>,
        raw: Option<RawSamples16>,
    },
    /// Кадра для снимка нет: итоговое событие.
    NotSecured { id: StillId, reason: NoFrameReason },
    /// Картинка нарисована в поверхность кодера и показана: кодер можно завершать. Итоговое событие.
    PrimaryRendered { id: StillId },
    /// Картинка прочитана в память (JPEG). Итоговое событие.
    PrimaryReadback { id: StillId, image: Rgba8Image },
    /// Проявление сорвалось после захвата. Итоговое событие.
    Failed { id: StillId, failure: DevelopFailure },
}

impl DevelopEvent {
    pub fn id(&self) -> StillId {
        match self {
            Self::Secured { facts, .. } => facts.id,
            Self::NotSecured { id, .. }
            | Self::PrimaryRendered { id }
            | Self::PrimaryReadback { id, .. }
            | Self::Failed { id, .. } => *id,
        }
    }

    /// Итоговое ли это событие снимка (после него рендер о снимке забывает).
    pub fn is_final(&self) -> bool {
        !matches!(self, Self::Secured { .. })
    }
}
