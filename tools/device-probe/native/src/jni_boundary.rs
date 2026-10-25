//! Граница JNI: гарантирует, что наружу всегда уходит валидный JSON и ни одна паника
//! не пересекает границу FFI (это было бы UB / abort процесса).

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::error::ProbeError;
use crate::report::ErrorReport;

/// Собирает JSON ошибки `{"ok":false,"stage":...,"error":...}`.
pub fn error_json(stage: &str, error: &str) -> String {
    let report = ErrorReport::new(stage, error);
    // Сериализация простой структуры из двух строк не падает; но «молча проглотить»
    // нельзя, поэтому на невозможный сбой отдаём заведомо валидный запасной JSON.
    serde_json::to_string(&report)
        .unwrap_or_else(|_| r#"{"ok":false,"stage":"error_json","error":"serialize_failed"}"#.to_owned())
}

/// Достаёт текст из payload паники (`&str` или `String`).
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panic без текстового сообщения".to_owned()
    }
}

/// Выполняет тело JNI-функции: `Ok` → JSON как есть, `Err` и паника → JSON ошибки.
pub fn guarded_json(stage: &str, body: impl FnOnce() -> Result<String, ProbeError>) -> String {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(json)) => json,
        Ok(Err(error)) => {
            log::error!("{stage}: {error}");
            error_json(stage, &error.to_string())
        }
        Err(payload) => {
            let message = format!("panic: {}", panic_message(payload.as_ref()));
            log::error!("{stage}: {message}");
            error_json(stage, &message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("валидный JSON")
    }

    #[test]
    fn ok_result_passes_through_unchanged() {
        assert_eq!(
            guarded_json("stage", || Ok(r#"{"ok":true}"#.to_owned())),
            r#"{"ok":true}"#
        );
    }

    #[test]
    fn error_becomes_error_report() {
        let json = guarded_json("catalog", || Err(ProbeError::platform("нет камеры")));
        let value = parse(&json);
        assert_eq!(value["ok"], false);
        assert_eq!(value["stage"], "catalog");
        assert_eq!(value["error"], "нет камеры");
    }

    #[test]
    fn panic_is_caught_and_reported() {
        let json = guarded_json("vulkan", || -> Result<String, ProbeError> { panic!("boom {}", 7) });
        let value = parse(&json);
        assert_eq!(value["ok"], false);
        assert_eq!(value["stage"], "vulkan");
        assert_eq!(value["error"], "panic: boom 7");
    }

    #[test]
    fn str_panic_payload_is_reported() {
        let json = guarded_json("s", || -> Result<String, ProbeError> { panic!("static") });
        assert_eq!(parse(&json)["error"], "panic: static");
    }

    #[test]
    fn error_json_escapes_special_characters() {
        let value = parse(&error_json("s", "кавычка \" и \n перевод"));
        assert_eq!(value["error"], "кавычка \" и \n перевод");
    }
}
