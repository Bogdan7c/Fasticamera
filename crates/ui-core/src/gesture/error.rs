//! Ошибки арены жестов.

use thiserror::Error;

use super::relations::RelationError;
use super::spec::SpecError;
use super::state::RecognizerId;
use crate::velocity::VelocityConfigError;

/// Ошибка настройки арены. Ошибки входа (касания) ошибками не считаются: их описывает
/// [`super::PointerDisposition`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum GestureError {
    #[error("неверные пороги распознавателя: {0}")]
    InvalidSpec(#[from] SpecError),
    #[error("неверные параметры оценки скорости: {0}")]
    InvalidVelocityConfig(#[from] VelocityConfigError),
    #[error("неверное отношение между распознавателями: {0}")]
    Relation(#[from] RelationError),
    #[error("распознаватель {0:?} неизвестен арене (не добавлен или уже удалён)")]
    UnknownRecognizer(RecognizerId),
    #[error("исчерпаны идентификаторы распознавателей арены (u32): новые добавить нельзя")]
    RecognizerIdsExhausted,
}
