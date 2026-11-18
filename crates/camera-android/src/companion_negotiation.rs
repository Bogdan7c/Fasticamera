//! Переговоры о вспомогательном потоке 3A при создании сессии (решение 62).
//!
//! Правило: вспомогательный поток **нужен, но не обязателен для запуска камеры**. Если его не
//! удалось создать, добавить в контейнер выходов или HAL отверг комбинацию потоков, сессия
//! создаётся заново без него, а отчёт честно говорит `unavailable` (автоматика HAL при этом
//! мертва — это видно в диагностике). Любой другой отказ (устройство отключилось, сбой платформы)
//! к потоку отношения не имеет и возвращается как есть — без повторной попытки.
//!
//! Логика вынесена из слоя NDK, чтобы порядок попыток, число вызовов и итоговый отчёт проверялись
//! на ПК. Слой NDK подставляет настоящие создание потока и сессии; `C` — владелец потока (его
//! `Drop` освобождает ресурс), `S` — созданная сессия.

use camera_core::backend::ConfigureFailure;

use crate::companion_stream::{CompanionStreamSpec, CompanionStreamStatus, CompanionUnavailableReason};

/// Почему одна попытка создать сессию не удалась.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptError {
    /// Контейнер отверг выход вспомогательного потока (`camera_status_t`). Есть смысл повторить
    /// без потока.
    CompanionOutputRejected { status: i32 },
    /// Любой другой отказ создания сессии.
    Failed(ConfigureFailure),
}

/// Итог переговоров: сессия, владелец потока (если он остался) и отчёт.
#[derive(Debug)]
pub struct Negotiated<C, S> {
    pub session: S,
    pub companion: Option<C>,
    pub status: CompanionStreamStatus,
}

/// Создаёт сессию с вспомогательным потоком, а при его отказе — без него.
///
/// * `create_companion` — создать поток; `Err(status)` — код `media_status_t`.
/// * `build_session` — одна попытка создать сессию; `None` — без вспомогательного потока.
pub fn negotiate_session<C, S>(
    spec: Option<CompanionStreamSpec>,
    create_companion: impl FnOnce(&CompanionStreamSpec) -> Result<C, i32>,
    mut build_session: impl FnMut(Option<&C>) -> Result<S, AttemptError>,
) -> Result<Negotiated<C, S>, ConfigureFailure> {
    let Some(spec) = spec else {
        let session = build_session(None).map_err(plain_failure)?;
        return Ok(Negotiated {
            session,
            companion: None,
            status: CompanionStreamStatus::DisabledByConfig,
        });
    };
    let companion = match create_companion(&spec) {
        Ok(companion) => companion,
        Err(status) => {
            return without_companion(
                &mut build_session,
                CompanionUnavailableReason::ReaderCreationFailed { status },
            );
        }
    };
    match build_session(Some(&companion)) {
        Ok(session) => Ok(Negotiated {
            session,
            companion: Some(companion),
            status: CompanionStreamStatus::Active {
                width: spec.size.width,
                height: spec.size.height,
            },
        }),
        Err(AttemptError::CompanionOutputRejected { status }) => {
            drop(companion);
            without_companion(
                &mut build_session,
                CompanionUnavailableReason::OutputRegistrationFailed { status },
            )
        }
        Err(AttemptError::Failed(ConfigureFailure::RejectedByHal { .. })) => {
            drop(companion);
            without_companion(&mut build_session, CompanionUnavailableReason::RejectedByHal)
        }
        Err(AttemptError::Failed(failure)) => Err(failure),
    }
}

/// Вторая попытка: без вспомогательного потока.
fn without_companion<C, S>(
    build_session: &mut impl FnMut(Option<&C>) -> Result<S, AttemptError>,
    reason: CompanionUnavailableReason,
) -> Result<Negotiated<C, S>, ConfigureFailure> {
    let session = build_session(None).map_err(plain_failure)?;
    Ok(Negotiated {
        session,
        companion: None,
        status: CompanionStreamStatus::Unavailable { reason },
    })
}

/// Попытка без потока не может сослаться на поток: «выход отвергнут» здесь означает баг вызывающего,
/// и он становится обычным отказом платформы, а не теряется.
fn plain_failure(error: AttemptError) -> ConfigureFailure {
    match error {
        AttemptError::Failed(failure) => failure,
        AttemptError::CompanionOutputRejected { status } => {
            ConfigureFailure::Backend(camera_core::backend::BackendFailure {
                operation: "ACaptureSessionOutputContainer_add",
                code: status,
                detail: "выход вспомогательного потока отвергнут в попытке без вспомогательного потока".to_owned(),
            })
        }
    }
}

#[cfg(test)]
#[path = "companion_negotiation_tests.rs"]
mod tests;
