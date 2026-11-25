//! Факты экрана, которые можно «заморозить»: так стенд воспроизводит гонку «движок уже изменил белое, а экран на
//! этом кадре ещё видит прежнее». В продукте это случается, потому что экран читает снимок статуса раз за кадр, а
//! движок живёт на своём потоке; стенду нужна управляемая версия, чтобы гонка была каждый раз, а не по везению.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use camera_screen::CameraFacts;
use fasticamera::camera_screen_facts::{CameraFactsSource, EngineFactsSource, FactsDiagnostics};

/// Рубильник заморозки фактов; клоны делят состояние.
#[derive(Clone, Default)]
pub struct FactsFreeze(Arc<AtomicBool>);

impl FactsFreeze {
    /// Экран видит прежние факты, пока заморозка включена.
    pub fn freeze(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn thaw(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    fn is_frozen(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Настоящий источник фактов движка с рубильником: пока заморожен, отдаёт последнее, что отдавал до заморозки.
pub struct FreezableFacts {
    inner: EngineFactsSource,
    freeze: FactsFreeze,
    last: Option<CameraFacts>,
}

impl FreezableFacts {
    pub fn new(inner: EngineFactsSource, freeze: FactsFreeze) -> Self {
        Self {
            inner,
            freeze,
            last: None,
        }
    }
}

impl CameraFactsSource for FreezableFacts {
    fn current_facts(&mut self) -> CameraFacts {
        if self.freeze.is_frozen()
            && let Some(last) = &self.last
        {
            return last.clone();
        }
        let fresh = self.inner.current_facts();
        self.last = Some(fresh.clone());
        fresh
    }

    fn diagnostics(&self) -> FactsDiagnostics {
        self.inner.diagnostics()
    }
}
