//! Память белой сцены между сессиями показа (решение 65: смена камеры стартует с последней белой).
//!
//! Сессия показа пересоздаётся с каждой surface, а белая сцены — это знание о свете, а не о surface.
//! Владелец — бэкенд; сессия получает клон ручки. Запись редкая (только при заметной смене белой), замок
//! держится на копирование одного значения.

use std::sync::{Arc, Mutex, PoisonError};

use scene_estimation::smoother::WhiteState;

/// Ручка к последней белой сцены (клонируется дёшево).
#[derive(Debug, Clone, Default)]
pub struct SceneWhiteMemory {
    shared: Arc<Mutex<Option<WhiteState>>>,
}

impl SceneWhiteMemory {
    /// Последняя сохранённая белая; `None` — сцену ещё не видели.
    pub fn load(&self) -> Option<WhiteState> {
        *self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Запоминает белую.
    pub fn store(&self, white: WhiteState) {
        *self.shared.lock().unwrap_or_else(PoisonError::into_inner) = Some(white);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_one_value() {
        let memory = SceneWhiteMemory::default();
        let other = memory.clone();
        assert_eq!(memory.load(), None);
        other.store(WhiteState {
            mired: 200.0,
            duv: 0.001,
        });
        assert_eq!(
            memory.load(),
            Some(WhiteState {
                mired: 200.0,
                duv: 0.001
            })
        );
    }
}
