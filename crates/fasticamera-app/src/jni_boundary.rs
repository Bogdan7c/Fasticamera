//! Защита границы FFI: ни одна паника не уходит в JVM.
//!
//! Раскрутка стека через `extern "system"` — аварийное завершение процесса, поэтому каждый
//! JNI-экспорт выполняет свою работу через `run_guarded`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::panic_report::{PanicReport, take_last_panic_on_this_thread};

/// Выполняет `body`; паника превращается в `Err(PanicReport)` с местом и сообщением.
///
/// `AssertUnwindSafe` допустим: после паники вызывающий получает только отчёт, а состояние,
/// которое `body` мог оставить наполовину изменённым, через этот вызов наружу не выходит.
pub fn run_guarded<T>(body: impl FnOnce() -> T) -> Result<T, PanicReport> {
    // Старый отчёт от необработанной ранее паники не должен приписаться этому вызову.
    let _stale = take_last_panic_on_this_thread();
    catch_unwind(AssertUnwindSafe(body))
        .map_err(|payload| take_last_panic_on_this_thread().unwrap_or_else(|| PanicReport::from_payload(&*payload)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panic_report::install_panic_hook;

    #[test]
    fn value_passes_through() {
        install_panic_hook();
        assert_eq!(run_guarded(|| 42), Ok(42));
    }

    #[test]
    fn panic_becomes_report_with_message_and_location() {
        install_panic_hook();
        let expected_line = line!() + 1;
        let report = run_guarded(|| -> u32 { panic!("сломалось {}", 7) }).expect_err("паника");
        assert_eq!(report.message, "сломалось 7");
        let location = report.location.clone().expect("место паники");
        assert!(
            location.starts_with(&format!("{}:{expected_line}:", file!())),
            "место {location}"
        );
        assert!(report.log_text().contains(&location));
    }

    #[test]
    fn report_is_consumed_and_not_reused() {
        install_panic_hook();
        let _ = run_guarded(|| -> () { panic!("первая") });
        assert_eq!(take_last_panic_on_this_thread(), None);
        assert_eq!(run_guarded(|| "ok"), Ok("ok"));
    }

    #[test]
    fn payload_fallback_reports_message_without_location() {
        let report = PanicReport::from_payload(&String::from("текст"));
        assert_eq!(report.message, "текст");
        assert_eq!(report.location, None);
        assert!(report.log_text().contains("<место неизвестно>"));
    }
}
