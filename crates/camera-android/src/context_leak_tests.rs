//! Тесты утечки контекстов: память переживает владельца, число видно в диагностике.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};

use super::*;

/// Контекст, который считает своё уничтожение.
struct Tracked {
    dropped: Arc<AtomicU32>,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn leaked_contexts_stay_alive_and_are_counted() {
    let diagnostics = BackendDiagnostics::new(4);
    let dropped = Arc::new(AtomicU32::new(0));
    let contexts: Vec<Arc<Tracked>> = (0..3)
        .map(|_| {
            Arc::new(Tracked {
                dropped: Arc::clone(&dropped),
            })
        })
        .collect();
    // Слабые ссылки играют роль указателей, оставшихся у NDK.
    let ndk_side: Vec<Weak<Tracked>> = contexts.iter().map(Arc::downgrade).collect();

    leak_contexts_for_process_lifetime(contexts, &diagnostics);

    assert_eq!(dropped.load(Ordering::SeqCst), 0, "ни один контекст не уничтожен");
    assert!(
        ndk_side.iter().all(|pointer| pointer.upgrade().is_some()),
        "память жива"
    );
    assert_eq!(diagnostics.snapshot().leaked_session_state_contexts, 3);
}

#[test]
fn nothing_to_leak_adds_nothing_to_the_counter() {
    let diagnostics = BackendDiagnostics::new(4);
    leak_contexts_for_process_lifetime::<Tracked>(Vec::new(), &diagnostics);
    assert_eq!(diagnostics.snapshot().leaked_session_state_contexts, 0);
}

#[test]
fn counter_accumulates_across_backends() {
    let diagnostics = BackendDiagnostics::new(4);
    leak_contexts_for_process_lifetime(vec![Arc::new(5_u8)], &diagnostics);
    leak_contexts_for_process_lifetime(vec![Arc::new(6_u8), Arc::new(7_u8)], &diagnostics);
    assert_eq!(diagnostics.snapshot().leaked_session_state_contexts, 3);
}
