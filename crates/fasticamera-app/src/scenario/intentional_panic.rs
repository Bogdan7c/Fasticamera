//! Сценарий `panic` (только debug): намеренная паника для проверки panic hook на телефоне.
//! Ожидание smoke-проверки: в logcat строка `panic at .../intentional_panic.rs:N:M`, а в
//! строке результата `status: panicked` с тем же местом.

use super::ScenarioError;

#[expect(clippy::panic, reason = "сценарий существует ровно для того, чтобы запаниковать")]
pub fn run() -> Result<serde_json::Value, ScenarioError> {
    panic!("намеренная паника сценария panic (проверка panic hook)")
}
