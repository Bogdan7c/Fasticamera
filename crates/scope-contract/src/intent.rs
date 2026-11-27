//! Намерения экрана: экран ничего не считает, он только просит «покажи такой прибор такого размера»
//! или «скрой приборы». Где картинка прибора стоит на экране, решает экран сам (решение 93) —
//! рендеру это знать не нужно.

use serde::{Deserialize, Serialize};

use crate::kind::{CieDiagram, HistogramMode, ScopeKind, VectorscopeAxes, VectorscopeZoom};
use crate::scan::ScopeScan;
use crate::size::ScopeImageSize;

/// Что показать: тип прибора, оси вектороскопа и размер картинки.
///
/// Оси хранятся всегда, даже когда тип — не вектороскоп: смена типа свайпом не должна терять
/// выбранный в меню стандарт осей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeRequest {
    /// Какой прибор.
    pub kind: ScopeKind,
    /// Стандарт осей вектороскопа.
    pub vectorscope_axes: VectorscopeAxes,
    /// Увеличение вектороскопа (решение 325). Хранится, как и оси, при любом типе прибора: свайп не теряет выбор.
    /// Старые записи без поля читаются как обычный масштаб.
    #[serde(default)]
    pub vectorscope_zoom: VectorscopeZoom,
    /// Режим гистограммы (сессия 14s). Хранится при любом типе прибора, как оси и зум: свайп не теряет выбор. Старые
    /// записи без поля читаются как RGB.
    #[serde(default)]
    pub histogram_mode: HistogramMode,
    /// Шкала диаграммы CIE (решение 332). Хранится при любом типе прибора, как зум и режим гистограммы; старые записи
    /// без поля читаются как xy 1931.
    #[serde(default)]
    pub cie_diagram: CieDiagram,
    /// Как глаз читает показанную картинку слева направо (положение телефона): горизонталь waveform и
    /// parade. Вектороскоп от поля не зависит; смена поля — только параметры, ресурсы не пересоздаются.
    pub scan: ScopeScan,
    /// Размер картинки прибора в пикселях экрана.
    pub image_size: ScopeImageSize,
}

/// Намерение экрана о приборах.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeIntent {
    /// Показывать прибор (заменяет прежний запрос целиком: смена типа, осей или размера).
    Show(ScopeRequest),
    /// Скрыть приборы: расчёт прекращается, ресурсы отпускаются (закрытые приборы ничего не стоят).
    Hide,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Запись до сессии 14s: ни режима гистограммы, ни (до 14r) увеличения вектороскопа в ней нет.
    const OLD_RECORD: &str = r#"{"kind":"waveform","vectorscope_axes":"y_cb_cr_bt709","scan":"upright",
        "image_size":{"width":300,"height":200}}"#;

    #[test]
    fn a_record_without_the_histogram_mode_reads_as_rgb_and_the_mode_survives_a_round_trip() {
        let request = serde_json::from_str::<ScopeRequest>(OLD_RECORD)
            .unwrap_or_else(|error| panic!("старая запись не читается: {error}"));
        assert_eq!(request.histogram_mode, HistogramMode::Rgb);
        assert_eq!(request.vectorscope_zoom, VectorscopeZoom::Normal);
        assert_eq!(request.cie_diagram, CieDiagram::Xy1931);
        let chosen = ScopeRequest {
            kind: ScopeKind::Histogram,
            histogram_mode: HistogramMode::Parade,
            ..request
        };
        let text = serde_json::to_string(&chosen).unwrap();
        assert!(text.contains(r#""histogram_mode":"parade""#), "{text}");
        assert_eq!(serde_json::from_str::<ScopeRequest>(&text).unwrap(), chosen);
    }
}
