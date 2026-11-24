//! Дайджест выхода работы в итоге строки: что показывает результат и как это читать.
//!
//! Дайджест — 64-битный отпечаток итогового выхода работы (картинка ISP, кадр показа, выход V4, снимок...),
//! прочитанный блокирующим readback ПОСЛЕ окна замера и `wait_idle` (цену работы он не искажает). Смысл: одна и
//! та же сборка даёт один и тот же дайджест от прогона к прогону, а оптимизация шейдера, не менявшая картинку,
//! даёт тот же дайджест, что «до» неё. Подробности устройства — `gpu_vulkan::saturation::digest`.
//!
//! # Формат
//! * `Computed` — `combined` (общий отпечаток, 16 hex-цифр) и `parts` (по метке на каждый выход: у приборов
//!   `counters` и `picture`, у снимка `image` и `thumbnail`, у остальных одна часть); `frame_note` — факт о
//!   самом кадре, который читали (у видоискателя: был ли он кадром со статистикой AWB — равенство дайджестов
//!   «со статистикой» и «без» на одном компиляторе шейдеров не обязано переноситься на другой);
//! * `Unavailable` — отпечаток не получен, причина текстом; строка замера из-за этого не срывается.
//!
//! В компактной таблице результата — только `combined` (или `-`); части и заметка — в `FASTICAMERA_DETAIL`.

use gpu_vulkan::saturation::WorkloadDigest;
use serde::Serialize;

/// Часть выхода: метка и отпечаток (16 hex-цифр).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DigestPartRow {
    pub label: &'static str,
    pub digest: String,
}

/// Итог дайджеста строки.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DigestOutcome {
    /// Отпечаток получен.
    Computed {
        combined: String,
        parts: Vec<DigestPartRow>,
        /// Факт о прочитанном кадре (см. описание модуля); `None` — нечего сказать.
        frame_note: Option<String>,
    },
    /// Отпечаток не получен: причина текстом.
    Unavailable { reason: String },
}

impl DigestOutcome {
    /// Из результата работы: ошибка чтения не срывает строку, а становится `Unavailable`.
    pub fn from_result(result: Result<WorkloadDigest, String>, frame_note: Option<String>) -> Self {
        match result {
            Ok(digest) => Self::Computed {
                combined: digest.combined().to_string(),
                parts: digest
                    .parts()
                    .iter()
                    .map(|part| DigestPartRow {
                        label: part.label,
                        digest: part.digest.to_string(),
                    })
                    .collect(),
                frame_note,
            },
            Err(reason) => Self::Unavailable { reason },
        }
    }

    /// Дописывает к заметке о кадре ещё одну (отчёт «эталон и вариант»); у `Unavailable` заметки нет — причина остаётся.
    pub fn with_extra_note(self, extra: &str) -> Self {
        match self {
            Self::Computed {
                combined,
                parts,
                frame_note,
            } => Self::Computed {
                combined,
                parts,
                frame_note: Some(match frame_note {
                    Some(existing) => format!("{existing}; {extra}"),
                    None => extra.to_owned(),
                }),
            },
            unavailable @ Self::Unavailable { .. } => unavailable,
        }
    }

    /// Запись для компактной таблицы: общий отпечаток или `-`.
    pub fn compact(&self) -> &str {
        match self {
            Self::Computed { combined, .. } => combined,
            Self::Unavailable { .. } => NO_DIGEST_CELL,
        }
    }
}

/// Ячейка таблицы, когда отпечатка нет.
pub const NO_DIGEST_CELL: &str = "-";

#[cfg(test)]
mod tests {
    use gpu_vulkan::saturation::{DigestPart, OutputDigest};

    use super::*;

    #[test]
    fn computed_digest_keeps_parts_in_order_and_compact_cell_is_the_combined_value() {
        let digest = WorkloadDigest::from_parts(vec![
            DigestPart {
                label: "counters",
                digest: OutputDigest::from_value(0xab),
            },
            DigestPart {
                label: "picture",
                digest: OutputDigest::from_value(0xcd),
            },
        ]);
        let combined = digest.combined().to_string();
        let outcome = DigestOutcome::from_result(Ok(digest), Some("заметка".to_owned()));
        let DigestOutcome::Computed { parts, frame_note, .. } = &outcome else {
            panic!("ожидался Computed: {outcome:?}");
        };
        assert_eq!(
            parts
                .iter()
                .map(|part| (part.label, part.digest.as_str()))
                .collect::<Vec<_>>(),
            [("counters", "00000000000000ab"), ("picture", "00000000000000cd")]
        );
        assert_eq!(frame_note.as_deref(), Some("заметка"));
        assert_eq!(outcome.compact(), combined);
        assert_eq!(combined.len(), 16);
    }

    #[test]
    fn failed_read_becomes_unavailable_with_its_reason_and_a_dash_in_the_table() {
        let outcome = DigestOutcome::from_result(Err("нет доступа к счётчикам".to_owned()), None);
        assert_eq!(
            outcome,
            DigestOutcome::Unavailable {
                reason: "нет доступа к счётчикам".to_owned()
            }
        );
        assert_eq!(outcome.compact(), "-");
        let json = serde_json::to_value(&outcome).expect("сериализуется");
        assert_eq!(json["state"], "unavailable");
    }
}
