//! Намеренная утечка контекстов колбэков, которые NDK может вызвать после разрушения бэкенда.
//!
//! `onClosed` сессии вызывается из деструктора `ACameraCaptureSession`
//! (`camera/ndk/impl/ACameraCaptureSession.cpp`) на потоке последнего владельца сессии, а он
//! может отработать позже и закрытия устройства, и разрушения самого бэкенда. Указатель на
//! контекст у NDK остаётся, поэтому память контекста обязана пережить бэкенд: освободить её —
//! значит получить use-after-free. Цена утечки — около сотни байт на сессию, и она считается в
//! диагностике (`leaked_session_state_contexts`), а не скрыта.
//!
//! Утечка вынесена в нейтральную функцию, чтобы «контекст остаётся жив» проверялось тестом на ПК.

use std::sync::Arc;

use crate::diagnostics::BackendDiagnostics;

/// Оставляет контексты жить до конца процесса и записывает их число в диагностику.
pub fn leak_contexts_for_process_lifetime<Context>(contexts: Vec<Arc<Context>>, diagnostics: &BackendDiagnostics) {
    let leaked = contexts.len() as u64;
    for context in contexts {
        // Намеренно: счётчик ссылок не уменьшается, память не освобождается (см. доку модуля).
        std::mem::forget(context);
    }
    diagnostics.record_leaked_session_state_contexts(leaked);
}

#[cfg(test)]
#[path = "context_leak_tests.rs"]
mod tests;
