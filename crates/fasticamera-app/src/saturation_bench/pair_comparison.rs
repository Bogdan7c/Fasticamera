//! Исполнение сравнения «эталон и вариант» ([`PairComparison`]): запоминает картинки эталонов и пишет в итог варианта
//! заметку с отчётом об отличии (см. [`super::compare_pairs`]).
//!
//! Модуль не знает GPU: ему отдают уже прочитанные [`OutputPixels`]. Заметка идёт в `frame_note` дайджеста строки
//! (она есть и в компактной таблице детальной сводки), поэтому форма результата строки не меняется.

use std::collections::HashMap;

use gpu_vulkan::saturation::OutputPixels;

use super::compare_pairs::{is_reference, reference_of};

/// Память сравнений одного прогона.
#[derive(Default)]
pub struct PairComparison {
    references: HashMap<&'static str, OutputPixels>,
}

impl PairComparison {
    /// Принимает картинку только что прогнанной работы `name` (или причину, почему её нельзя прочитать) и возвращает
    /// заметку для её итога: у варианта — отчёт об отличии от эталона, у чистого эталона и у работы вне пар — `None`.
    pub fn note_for(&mut self, name: &'static str, pixels: Result<OutputPixels, String>) -> Option<String> {
        let note = reference_of(name).map(|reference| self.compare(name, reference, &pixels));
        if is_reference(name) {
            // Эталон, не прочитанный из-за ошибки, не сохраняется: варианты честно скажут «эталона нет».
            if let Ok(pixels) = pixels {
                self.references.insert(name, pixels);
            }
        }
        note
    }

    fn compare(&self, name: &str, reference: &'static str, pixels: &Result<OutputPixels, String>) -> String {
        let variant = match pixels {
            Ok(variant) => variant,
            Err(reason) => return format!("diff_vs={reference}: картинку {name} прочитать не удалось: {reason}"),
        };
        let Some(base) = self.references.get(reference) else {
            return format!(
                "diff_vs={reference}: эталон не прогонялся раньше в этом списке (поставьте его перед вариантом)"
            );
        };
        match variant.diff_against(base) {
            Ok(report) => format!("diff_vs={reference}: {report}"),
            Err(error) => format!("diff_vs={reference}: сравнить нельзя: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpu_vulkan::saturation::ImageFormat;

    use super::*;

    fn picture(codes: &[u32]) -> OutputPixels {
        let bytes = codes
            .iter()
            .flat_map(|code| (code | (code << 10) | (code << 20) | (3 << 30)).to_le_bytes())
            .collect();
        OutputPixels::from_dense_bytes(ImageFormat::Rgb10A2, codes.len() as u32, 1, bytes).unwrap()
    }

    #[test]
    fn variant_gets_a_report_against_the_reference_run_before_it() {
        let mut comparison = PairComparison::default();
        assert_eq!(
            comparison.note_for("viewfinder_awb_never", Ok(picture(&[10, 20, 30]))),
            None
        );
        let note = comparison
            .note_for("viewfinder_awb_never_twin", Ok(picture(&[10, 22, 30])))
            .expect("у варианта есть заметка");
        assert!(note.starts_with("diff_vs=viewfinder_awb_never:"), "{note}");
        assert!(
            note.contains("max|Δ| R/G/B = 0/2/0") || note.contains("2/2/2"),
            "{note}"
        );
        assert!(note.contains("Δ≥1: 1") && note.contains("Δ≥2: 1"), "{note}");
    }

    #[test]
    fn missing_reference_is_said_aloud_instead_of_silently_skipped() {
        let mut comparison = PairComparison::default();
        let note = comparison
            .note_for("viewfinder_awb_never_twin", Ok(picture(&[1])))
            .unwrap();
        assert!(note.contains("эталон не прогонялся"), "{note}");
    }

    #[test]
    fn unreadable_variant_and_shape_mismatch_are_reported() {
        let mut comparison = PairComparison::default();
        comparison.note_for("viewfinder_awb_never", Ok(picture(&[1, 2])));
        let unreadable = comparison
            .note_for("viewfinder_awb_never_twin", Err("нет доступа".to_owned()))
            .unwrap();
        assert!(unreadable.contains("нет доступа"), "{unreadable}");
        let wrong_shape = comparison
            .note_for("viewfinder_awb_never_twin", Ok(picture(&[1, 2, 3])))
            .unwrap();
        assert!(wrong_shape.contains("сравнить нельзя"), "{wrong_shape}");
    }

    #[test]
    fn work_outside_pairs_gets_no_note() {
        let mut comparison = PairComparison::default();
        assert_eq!(comparison.note_for("viewfinder", Ok(picture(&[1]))), None);
    }
}
