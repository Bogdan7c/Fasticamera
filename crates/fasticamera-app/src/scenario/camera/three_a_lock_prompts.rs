//! Тексты подсказок владельцу в шаге блокировки `3a`: «приготовься», «закрой объектив», «убери руку».
//!
//! Только слова и вид (цвет) подсказки. Когда показывать, решает `LockStep`, как показывать — платформа.

use shell_contract::operator_prompt::{OperatorPrompt, OperatorPromptKind};

/// Отсчёт до начала удержания: «ПРИГОТОВЬСЯ: через 3 с» / «закрой объектив рукой».
pub fn prepare_prompt(seconds_left: u64) -> OperatorPrompt {
    OperatorPrompt::new(
        OperatorPromptKind::Prepare,
        format!("ПРИГОТОВЬСЯ: через {seconds_left} с"),
        "закрой объектив рукой",
    )
}

/// Окно удержания началось (висит всё удержание): закрывать объектив нужно сейчас.
pub fn hold_start_prompt() -> OperatorPrompt {
    OperatorPrompt::new(
        OperatorPromptKind::Act,
        "ЗАКРОЙ ОБЪЕКТИВ РУКОЙ — держи",
        "держи, пока не появится «УБЕРИ РУКУ»",
    )
}

/// Окно удержания кончилось: объектив можно открывать.
pub fn hold_end_prompt() -> OperatorPrompt {
    OperatorPrompt::new(OperatorPromptKind::Done, "УБЕРИ РУКУ", "окно удержания закончено")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_carry_the_expected_kind_and_words() {
        let prepare = prepare_prompt(3);
        assert_eq!(prepare.kind(), OperatorPromptKind::Prepare);
        assert_eq!(prepare.headline(), "ПРИГОТОВЬСЯ: через 3 с");
        assert_eq!(prepare.detail(), "закрой объектив рукой");
        assert_eq!(hold_start_prompt().kind(), OperatorPromptKind::Act);
        assert_eq!(hold_start_prompt().headline(), "ЗАКРОЙ ОБЪЕКТИВ РУКОЙ — держи");
        assert_eq!(hold_end_prompt().kind(), OperatorPromptKind::Done);
        assert_eq!(hold_end_prompt().headline(), "УБЕРИ РУКУ");
    }
}
