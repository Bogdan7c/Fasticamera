//! K2 «Кнопка» — только механика нажатия, без вида (внешний вид рисует `ui-render-vulkan`).

mod press_tracker;

pub use press_tracker::{ButtonError, ButtonState, PressOutcome, PressTracker};
