//! Учёт surface у оболочки: номер поколения, своя ссылка на текущее окно и пожелание частоты.
//!
//! Частота кадров — свойство конкретной surface, а surface пересоздаётся при каждом
//! сворачивании. Поэтому оболочка помнит пожелание и сама применяет его к каждой новой
//! surface — потребителю не нужно следить за жизненным циклом.
//!
//! Реестр — только учёт под замком. Сам вызов платформы (binder к SurfaceFlinger) вызывающий
//! делает уже **вне** замка с окном, которое вернул реестр: так главный поток Android не ждёт
//! чужой binder-вызов, а поток маршрутизатора — главный поток.
//!
//! Тип окна — параметр `W`: на телефоне это ссылка на `ANativeWindow`, в тестах — fake.

use shell_contract::commands::{FrameRate, FrameRateOutcome};
use shell_contract::surface::SurfaceGeneration;

/// Код успеха платформенного вызова установки частоты.
const PLATFORM_OK: i32 = 0;

#[derive(Debug)]
pub struct SurfaceRegistry<W> {
    last_generation: u64,
    current: Option<(SurfaceGeneration, W)>,
    preferred_frame_rate: Option<FrameRate>,
}

impl<W> Default for SurfaceRegistry<W> {
    fn default() -> Self {
        Self {
            last_generation: 0,
            current: None,
            preferred_frame_rate: None,
        }
    }
}

impl<W> SurfaceRegistry<W> {
    /// Новая surface: выдаёт следующий номер поколения. Возвращает и запомненное пожелание
    /// частоты — вызывающий применяет его к своей ссылке на это окно (`None` — пожелания нет).
    pub fn surface_created(&mut self, window: W) -> (SurfaceGeneration, Option<FrameRate>) {
        self.last_generation += 1;
        let generation = SurfaceGeneration::new(self.last_generation);
        self.current = Some((generation, window));
        (generation, self.preferred_frame_rate)
    }

    /// Номер текущей surface, если она есть.
    pub fn current_generation(&self) -> Option<SurfaceGeneration> {
        self.current.as_ref().map(|(generation, _)| *generation)
    }

    /// Surface уходит: отдаёт своё окно (его нужно отпустить) и номер.
    pub fn surface_destroyed(&mut self) -> Option<(SurfaceGeneration, W)> {
        self.current.take()
    }

    /// Запоминает пожелание. Возвращает копию ссылки на текущее окно, к которому его нужно
    /// применить сейчас (`None` — surface нет, применится к следующей).
    pub fn set_preferred_frame_rate(&mut self, rate: FrameRate) -> Option<W>
    where
        W: Clone,
    {
        self.preferred_frame_rate = Some(rate);
        self.current.as_ref().map(|(_, window)| window.clone())
    }
}

/// Итог вызова платформы установки частоты по его коду (0 — принято).
pub fn frame_rate_outcome(status: i32) -> FrameRateOutcome {
    if status == PLATFORM_OK {
        FrameRateOutcome::Applied
    } else {
        FrameRateOutcome::Rejected { status }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake-окно: имя для проверки, к какому окну относится вызов.
    type FakeWindow = &'static str;

    fn rate(hertz: f32) -> FrameRate {
        FrameRate::new(hertz).expect("частота")
    }

    #[test]
    fn generations_grow_with_each_surface() {
        let mut registry = SurfaceRegistry::<FakeWindow>::default();
        let (first, _) = registry.surface_created("a");
        assert_eq!(registry.surface_destroyed(), Some((first, "a")));
        let (second, _) = registry.surface_created("b");
        assert!(second > first);
        assert_eq!(registry.current_generation(), Some(second));
    }

    #[test]
    fn preference_without_surface_is_deferred_and_returned_for_next_surface() {
        let mut registry = SurfaceRegistry::<FakeWindow>::default();
        assert_eq!(
            registry.set_preferred_frame_rate(rate(120.0)),
            None,
            "окна нет — применять некуда"
        );
        let (_, pending) = registry.surface_created("main");
        assert_eq!(pending.map(FrameRate::hertz), Some(120.0));
    }

    #[test]
    fn preference_with_surface_returns_that_window() {
        let mut registry = SurfaceRegistry::<FakeWindow>::default();
        registry.surface_created("main");
        assert_eq!(registry.set_preferred_frame_rate(rate(90.0)), Some("main"));
    }

    #[test]
    fn preference_is_returned_again_after_surface_recreation() {
        let mut registry = SurfaceRegistry::<FakeWindow>::default();
        registry.surface_created("first");
        registry.set_preferred_frame_rate(rate(120.0));
        registry.surface_destroyed();
        let (_, pending) = registry.surface_created("second");
        assert_eq!(pending.map(FrameRate::hertz), Some(120.0));
        assert_eq!(registry.surface_destroyed().map(|(_, window)| window), Some("second"));
    }

    #[test]
    fn no_preference_means_nothing_to_apply() {
        let mut registry = SurfaceRegistry::<FakeWindow>::default();
        let (_, pending) = registry.surface_created("main");
        assert_eq!(pending, None);
        assert_eq!(registry.surface_destroyed().map(|(_, window)| window), Some("main"));
        assert_eq!(registry.surface_destroyed(), None, "повторное уничтожение — пусто");
    }

    #[test]
    fn platform_status_maps_to_outcome() {
        const EINVAL: i32 = -22;
        assert_eq!(frame_rate_outcome(0), FrameRateOutcome::Applied);
        assert_eq!(
            frame_rate_outcome(EINVAL),
            FrameRateOutcome::Rejected { status: EINVAL }
        );
    }
}
