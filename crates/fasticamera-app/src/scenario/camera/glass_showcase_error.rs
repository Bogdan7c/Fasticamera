//! Ошибки демо-сцены стекла: настройка (конфиг) и работа (кадр) разделены, ничего не глотается.

use thiserror::Error;
use ui_core::scene::SceneError;
use ui_core::spring::SpringError;
use ui_draw::glass::GlassStyleError;
use ui_draw::light::LightError;
use ui_draw::text::TextError;
use ui_kit::tokens::TokenError;

/// Конфиг сцены задан неверно (проверяется один раз в `GlassShowcaseConfig::validate`).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ShowcaseConfigError {
    /// Число должно быть конечным и строго положительным.
    #[error("поле «{field}» должно быть конечным и > 0, получено {value}")]
    NotPositive { field: &'static str, value: f64 },
    /// Число должно быть конечным и неотрицательным.
    #[error("поле «{field}» должно быть конечным и >= 0, получено {value}")]
    Negative { field: &'static str, value: f64 },
    /// Число вне допустимого диапазона.
    #[error("поле «{field}» вне диапазона {min}…{max}: {value}")]
    OutOfRange {
        field: &'static str,
        value: f64,
        min: f64,
        max: f64,
    },
    /// Цвет вне 0..1 или не число.
    #[error("цвет «{field}» некорректен: компоненты должны быть числами от 0 до 1")]
    InvalidColor { field: &'static str },
    /// Подпись пустая: нечего рисовать.
    #[error("подпись «{field}» пуста")]
    EmptyLabel { field: &'static str },
    /// Поля по отдельности верны, но друг другу противоречат.
    #[error("противоречивый конфиг: {reason}")]
    Inconsistent { reason: String },
    /// Настройки света от наклона.
    #[error("свет: {0}")]
    Light(#[from] LightError),
}

/// Ошибка создания сцены или построения кадра.
#[derive(Debug, Error)]
pub enum ShowcaseError {
    #[error("конфиг: {0}")]
    Config(#[from] ShowcaseConfigError),
    #[error("токены ui-kit: {0}")]
    Tokens(#[from] TokenError),
    #[error("пресет стекла: {0}")]
    Style(#[from] GlassStyleError),
    #[error("пружина: {0}")]
    Spring(#[from] SpringError),
    #[error("сцена ui-core: {0}")]
    Scene(#[from] SceneError),
    #[error("свёрстка подписи: {0}")]
    Text(#[from] TextError),
    /// Размер экрана от рендера не годится для сцены (ноль, не число, не помещается в u32).
    #[error("логический размер экрана {width_px}×{height_px} px непригоден для сцены")]
    UnusableSurface { width_px: f32, height_px: f32 },
    /// Масштаб pt от рендера расходится с тем, что вывела сцена из ширины экрана: списки были бы
    /// нарисованы в другом масштабе, чем вёрстка и жесты, — это ошибка сборки, а не повод «подогнать».
    #[error("px_per_pt рендера {requested} расходится с масштабом сцены {scene}")]
    ScaleMismatch { requested: f32, scene: f64 },
    /// Элемент сцены не попал в порядок рисования (скрыт или удалён) — нарушен инвариант сцены.
    #[error("элемент «{label}» отсутствует в порядке рисования сцены")]
    MissingVisual { label: &'static str },
    /// Прозрачность элемента вне 0..1 (не должно случаться: `node_placement_px` её зажимает).
    #[error("прозрачность {value} элемента «{label}» некорректна")]
    BadOpacity { label: &'static str, value: f32 },
}
