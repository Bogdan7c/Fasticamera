//! Подавление работы GPU для A/B-замера тока: «выполнять работу / не выполнять» на настоящем экране камеры.
//!
//! Зачем. Чтобы измерить цену цветовых приборов и рисования UI в мА (сессия 16a), нужен рычаг, который ВРЕМЕННО
//! не выполняет работу GPU, но ничего не меняет в состоянии экрана: экран камеры помнит «объявленный» запрос
//! приборов и шлёт `Show` только при его изменении. Поэтому рычаг живёт в рендере отдельной маской
//! ([`GpuWorkSuppression`]): запрос экрана и установленный источник UI остаются как были, а после снятия маски
//! работа возобновляется сама, без повторной команды экрана.
//!
//! Это НЕ продуктовая настройка: продукт всегда работает с [`GpuWorkSuppression::NONE`]. Маска включается
//! только командой замера ([`ViewfinderCommand::SuppressGpuWork`](super::ViewfinderCommand::SuppressGpuWork)),
//! а конец сценария возвращает `NONE`.

/// Работа продукта, которую замер умеет временно не выполнять.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuppressibleGpuWork {
    /// Расчёт цветовых приборов (`isp-vulkan::scopes`): проход на каждом новом кадре камеры. Запрос экрана о приборах
    /// при этом помнится, объекты GPU приборов освобождаются (закрытые приборы ничего не стоят, решение 190).
    ColorScopes,
    /// GPU-запись прохода UI поверх видоискателя: стекло, пирамида фона, загрузка экземпляров. Источник UI остаётся
    /// установленным и по-прежнему получает `produce_frame` раз на показанный кадр (контракт `UiFrameSource` и разбор
    /// касаний не ломаются); кадр на экране — только компоновщик.
    UiOverlayDrawing,
}

impl SuppressibleGpuWork {
    /// Бит работы в маске. Номера не зависят от порядка объявления: работа, добавленная позже, берёт следующий бит.
    const fn bit(self) -> u8 {
        match self {
            Self::ColorScopes => 1 << 0,
            Self::UiOverlayDrawing => 1 << 1,
        }
    }
}

/// Набор подавленных работ GPU (битовая маска внутри — наружу только intent-методы). Как и `IspStageSet`, это набор
/// для замера, а не для показа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GpuWorkSuppression(u8);

impl GpuWorkSuppression {
    /// Ничего не подавлено — продукт.
    pub const NONE: Self = Self(0);

    /// Набор, в котором подавлена и указанная работа (остальное — как было).
    #[must_use]
    pub const fn suppressing(self, work: SuppressibleGpuWork) -> Self {
        Self(self.0 | work.bit())
    }

    /// Подавлена ли работа.
    pub const fn suppresses(self, work: SuppressibleGpuWork) -> bool {
        self.0 & work.bit() != 0
    }

    /// Ничего не подавлено (продуктовое поведение).
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_WORK: [SuppressibleGpuWork; 2] =
        [SuppressibleGpuWork::ColorScopes, SuppressibleGpuWork::UiOverlayDrawing];

    #[test]
    fn none_is_the_default_and_suppresses_nothing() {
        assert_eq!(GpuWorkSuppression::default(), GpuWorkSuppression::NONE);
        assert!(GpuWorkSuppression::NONE.is_none());
        for work in ALL_WORK {
            assert!(!GpuWorkSuppression::NONE.suppresses(work), "{work:?}");
        }
    }

    #[test]
    fn suppressing_one_work_touches_only_that_work() {
        for suppressed in ALL_WORK {
            let set = GpuWorkSuppression::NONE.suppressing(suppressed);
            assert!(!set.is_none(), "{suppressed:?}");
            for work in ALL_WORK {
                assert_eq!(set.suppresses(work), work == suppressed, "{suppressed:?} / {work:?}");
            }
        }
    }

    #[test]
    fn suppressing_is_idempotent_and_accumulates() {
        let scopes = GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::ColorScopes);
        assert_eq!(
            scopes.suppressing(SuppressibleGpuWork::ColorScopes),
            scopes,
            "повтор ничего не меняет"
        );
        let both = scopes.suppressing(SuppressibleGpuWork::UiOverlayDrawing);
        for work in ALL_WORK {
            assert!(both.suppresses(work), "{work:?}");
        }
        assert_ne!(both, scopes);
        assert_eq!(
            both,
            GpuWorkSuppression::NONE
                .suppressing(SuppressibleGpuWork::UiOverlayDrawing)
                .suppressing(SuppressibleGpuWork::ColorScopes),
            "порядок добавления не важен"
        );
    }
}
