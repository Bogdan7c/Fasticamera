//! Подписи прибора (решения 326, 327): канал «рендер → приложение → экран», по которому эталон `scope-model`
//! сообщает экрану, ГДЕ на картинке прибора стоит подписанная отметка.
//!
//! # Зачем канал
//! Место цели вектороскопа зависит от того, чего экран не знает: охват экрана (для OKLab), масштаб из настроек
//! (`ycbcr_edge_chroma`, `oklab_range`), зум ×2. Знает только рендер, поэтому он считает положения по ТОМУ ЖЕ правилу,
//! что рисует саму сетку, а экран только пишет текст в готовую точку. Тот же канал понесёт подписи треугольников
//! охвата CIE и точки белой.
//!
//! # Что здесь, а что на экране
//! Здесь — только значения: КАКАЯ отметка ([`ScopeMark`]) и в какой точке картинки ([`LabelPoint`]). Сам текст
//! («R», «MG», «P3») — язык интерфейса, его выбирает экран.
//!
//! # Подписи привязаны к запросу
//! [`ScopeLabels`] хранит запрос, для которого посчитаны. После свайпа на другую страницу, смены осей или зума
//! экран ещё какое-то время видит старые подписи: по запросу он их отличает и не рисует чужие
//! ([`ScopeLabelsState::labels_for`]).

use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::intent::ScopeRequest;

/// Цвет цели вектороскопа. Порядок вариантов НЕ связан с порядком таблицы целей в `scope-model`: соответствие
/// задано там явно по имени.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetColor {
    /// Красный (R).
    Red,
    /// Пурпурный (MG).
    Magenta,
    /// Синий (B).
    Blue,
    /// Голубой (CY).
    Cyan,
    /// Зелёный (G).
    Green,
    /// Жёлтый (YL).
    Yellow,
}

/// Охват, треугольник которого рисует диаграмма цветности CIE (подписи «sRGB», «P3», «Rec.2020»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChromaticityGamut {
    /// sRGB / Rec.709.
    Srgb,
    /// Display P3.
    DisplayP3,
    /// Rec.2020.
    Rec2020,
}

/// Что подписано: отметка прибора, не текст.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeMark {
    /// Цель цвета вектороскопа.
    VectorscopeTarget(TargetColor),
    /// Треугольник охвата на диаграмме цветности CIE.
    Gamut(ChromaticityGamut),
    /// Точка белой на диаграмме цветности CIE.
    WhitePoint,
}

/// Ошибка точки подписи.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum LabelPointError {
    /// Координата вне `0..=1` или не число.
    #[error("точка подписи ({x}, {y}) вне картинки 0..=1 или не число")]
    OutOfPicture {
        /// Отвергнутая координата по горизонтали.
        x: f32,
        /// Отвергнутая координата по вертикали.
        y: f32,
    },
}

/// Точка на картинке прибора в долях её размера: `x` вправо, `y` ВНИЗ, обе в `0..=1` — те же координаты, что у
/// примитивов сетки (`GraticulePrimitive`). Непроверенной точки не существует.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelPoint {
    x: f32,
    y: f32,
}

impl LabelPoint {
    /// Проверяет и строит точку. NaN и бесконечности отвергаются (в диапазон они не попадают).
    pub fn try_new(x: f32, y: f32) -> Result<Self, LabelPointError> {
        if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) {
            Ok(Self { x, y })
        } else {
            Err(LabelPointError::OutOfPicture { x, y })
        }
    }

    /// Доля ширины картинки от левого края.
    pub fn x(self) -> f32 {
        self.x
    }

    /// Доля высоты картинки от верхнего края.
    pub fn y(self) -> f32 {
        self.y
    }
}

/// Одна подпись: какая отметка и где должен стоять ЦЕНТР её текста.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScopeLabel {
    /// Что подписано.
    pub mark: ScopeMark,
    /// Куда ставить центр текста.
    pub at: LabelPoint,
}

/// Подписи прибора, посчитанные для ровно этого запроса.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeLabels {
    /// Запрос, для которого посчитаны положения (вид, оси, зум, размер картинки).
    pub request: ScopeRequest,
    /// Подписи; пусто, если у прибора их нет (waveform) или все цели вне сетки.
    pub labels: Vec<ScopeLabel>,
}

/// Что рендер сообщил о подписях. Три состояния различимы, чтобы экран не путал «ещё не знаем» и «приборы закрыты».
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ScopeLabelsState {
    /// Рендер ещё ничего не публиковал (потока нет, приборы не открывались).
    #[default]
    NotPublished,
    /// Приборы закрыты: подписывать нечего.
    Withdrawn,
    /// Подписи посчитаны для запроса.
    Published(ScopeLabels),
}

impl ScopeLabelsState {
    /// Подписи, которые можно рисовать при желании экрана `desired`: только посчитанные ИМЕННО для него. Чужой
    /// запрос (другая страница, оси, зум, размер), «закрыто» и «не публиковалось» дают пустой список.
    pub fn labels_for(&self, desired: &ScopeRequest) -> &[ScopeLabel] {
        match self {
            Self::Published(published) if published.request == *desired => &published.labels,
            Self::Published(_) | Self::Withdrawn | Self::NotPublished => &[],
        }
    }
}

/// Доска подписей: последнее значение, клоны делят одно состояние (как `LookWhiteStopsBoard`). Пишет рендер при
/// смене запроса или охвата, читает приложение для экрана; очереди нет, промежуточные значения не нужны.
///
/// Отравленный замок читается как обычный: значение заменяется целиком, частично записанного не бывает.
#[derive(Debug, Clone, Default)]
pub struct ScopeLabelsBoard {
    latest: Arc<Mutex<ScopeLabelsState>>,
}

impl ScopeLabelsBoard {
    /// Новая доска: [`ScopeLabelsState::NotPublished`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Заменяет значение (пишет поток рендера).
    pub fn publish(&self, state: ScopeLabelsState) {
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }

    /// Копия последнего значения; до первой публикации — [`ScopeLabelsState::NotPublished`].
    pub fn latest(&self) -> ScopeLabelsState {
        self.latest.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::{CieDiagram, ScopeKind, VectorscopeAxes, VectorscopeZoom};
    use crate::scan::ScopeScan;
    use crate::size::ScopeImageSize;

    fn request(kind: ScopeKind, zoom: VectorscopeZoom) -> ScopeRequest {
        ScopeRequest {
            kind,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            vectorscope_zoom: zoom,
            cie_diagram: CieDiagram::default(),
            histogram_mode: crate::HistogramMode::default(),
            scan: ScopeScan::Upright,
            image_size: ScopeImageSize::try_new(512, 512).expect("размер в допуске"),
        }
    }

    fn red_label() -> ScopeLabel {
        ScopeLabel {
            mark: ScopeMark::VectorscopeTarget(TargetColor::Red),
            at: LabelPoint::try_new(0.9, 0.4).expect("точка в картинке"),
        }
    }

    #[test]
    fn a_point_accepts_the_closed_unit_square_and_rejects_everything_else() {
        let corner = LabelPoint::try_new(1.0, 0.0).expect("угол картинки допустим");
        assert_eq!((corner.x(), corner.y()), (1.0, 0.0));
        for (x, y) in [
            (-0.001, 0.5),
            (0.5, 1.001),
            (f32::NAN, 0.5),
            (0.5, f32::NAN),
            (f32::INFINITY, 0.5),
            (0.5, f32::NEG_INFINITY),
        ] {
            assert!(
                matches!(LabelPoint::try_new(x, y), Err(LabelPointError::OutOfPicture { .. })),
                "({x}, {y}) должно быть отвергнуто"
            );
        }
    }

    #[test]
    fn the_board_is_not_published_until_the_first_publish_and_clones_share_the_latest_value() {
        let board = ScopeLabelsBoard::new();
        let reader = board.clone();
        assert_eq!(reader.latest(), ScopeLabelsState::NotPublished);

        let shown = ScopeLabelsState::Published(ScopeLabels {
            request: request(ScopeKind::Vectorscope, VectorscopeZoom::default()),
            labels: vec![red_label()],
        });
        board.publish(shown.clone());
        assert_eq!(reader.latest(), shown);

        board.publish(ScopeLabelsState::Withdrawn);
        assert_eq!(reader.latest(), ScopeLabelsState::Withdrawn);
    }

    #[test]
    fn labels_are_given_out_only_for_the_exact_request_they_were_computed_for() {
        let computed_for = request(ScopeKind::Vectorscope, VectorscopeZoom::default());
        let state = ScopeLabelsState::Published(ScopeLabels {
            request: computed_for,
            labels: vec![red_label()],
        });
        assert_eq!(state.labels_for(&computed_for), &[red_label()]);

        let zoomed = request(ScopeKind::Vectorscope, VectorscopeZoom::Double);
        let other_page = request(ScopeKind::Waveform, VectorscopeZoom::default());
        assert!(
            state.labels_for(&zoomed).is_empty(),
            "после зума ×2 чужие подписи не рисуются"
        );
        assert!(
            state.labels_for(&other_page).is_empty(),
            "после свайпа чужие подписи не рисуются"
        );
        assert!(ScopeLabelsState::Withdrawn.labels_for(&computed_for).is_empty());
        assert!(ScopeLabelsState::NotPublished.labels_for(&computed_for).is_empty());
    }

    #[test]
    fn gamut_names_are_written_in_snake_case() {
        let json = serde_json::to_string(&ChromaticityGamut::DisplayP3).expect("перечисление пишется");
        assert_eq!(json, "\"display_p3\"");
        let parsed: ChromaticityGamut = serde_json::from_str("\"rec2020\"").expect("rec2020 читается");
        assert_eq!(parsed, ChromaticityGamut::Rec2020);
    }
}
