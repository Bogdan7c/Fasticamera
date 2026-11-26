//! Типы приборов и стандарт осей вектороскопа.

use serde::{Deserialize, Serialize};

use crate::level_lines::{LevelLine, WAVEFORM_LEVEL_LINES};
use crate::size::ScopeImageSize;

/// Тип цветового прибора (решение 183).
///
/// Новый прибор (гистограмма, «ложный цвет» пересвета…) — новый вариант здесь плюс своё правило
/// попадания в `scope-model` и свой шейдер накопления в `isp-vulkan`; соседние варианты не трогаются.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// RGB waveform: R', G' и B' по столбцам кадра тремя следами, наложенными друг на друга (серый — белый след,
    /// цветной сдвиг виден цветом): где тени и света, что упёрлось в белое или чёрное, и какого цвета перекос.
    Waveform,
    /// То же для R', G' и B' рядом секциями: сдвиг цвета в тенях или светах (серый — три одинаковые картинки).
    RgbParade,
    /// Цветность по кругу: направление и сила цвета, серый — точка в центре.
    Vectorscope,
}

/// Форма картинки прибора на экране: решает вид прибора, а не раскладка (решение 14m).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopePictureShape {
    /// Картинка занимает всю зону прибора (waveform и parade растут вдоль экрана).
    FillsZone,
    /// Картинка — квадрат, вписанный в зону (сторона = меньшая сторона зоны) и стоящий по её центру:
    /// круг вектороскопа на прямоугольнике превратился бы в овал.
    Square,
}

impl ScopePictureShape {
    /// Размер картинки прибора такой формы в зоне размера `zone`: зона целиком или вписанный в неё квадрат.
    pub fn fit_in(self, zone: ScopeImageSize) -> ScopeImageSize {
        match self {
            ScopePictureShape::FillsZone => zone,
            ScopePictureShape::Square => zone.inscribed_square(),
        }
    }
}

impl ScopeKind {
    /// Все типы приборов. Ресурсы приборов создаются сразу для всех типов, чтобы смена типа свайпом
    /// не создавала ничего посреди жеста.
    pub const ALL: [ScopeKind; 3] = [ScopeKind::Waveform, ScopeKind::RgbParade, ScopeKind::Vectorscope];

    /// Какой формы картинка нужна прибору. `match` без `_`: новый вид не соберётся без своей формы.
    pub fn picture_shape(self) -> ScopePictureShape {
        match self {
            ScopeKind::Waveform | ScopeKind::RgbParade => ScopePictureShape::FillsZone,
            ScopeKind::Vectorscope => ScopePictureShape::Square,
        }
    }

    /// Линии уровней, которые прибор рисует в своей сетке и которые экран подписывает числами слева. У вектороскопа
    /// их нет (пустой список). `match` без `_`: новый прибор не соберётся, пока не скажет, есть ли у него уровни.
    pub fn level_lines(self) -> &'static [LevelLine] {
        match self {
            ScopeKind::Waveform | ScopeKind::RgbParade => &WAVEFORM_LEVEL_LINES,
            ScopeKind::Vectorscope => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shape_fits_into_a_zone_as_the_zone_itself_or_its_inscribed_square() {
        let zone = ScopeImageSize::try_new(540, 743).unwrap();
        assert_eq!(ScopePictureShape::FillsZone.fit_in(zone), zone);
        assert_eq!(
            ScopePictureShape::Square.fit_in(zone),
            ScopeImageSize::try_new(540, 540).unwrap()
        );
    }

    #[test]
    fn only_the_vectorscope_needs_a_square_picture() {
        assert_eq!(ScopeKind::Waveform.picture_shape(), ScopePictureShape::FillsZone);
        assert_eq!(ScopeKind::RgbParade.picture_shape(), ScopePictureShape::FillsZone);
        assert_eq!(ScopeKind::Vectorscope.picture_shape(), ScopePictureShape::Square);
        // Квадратных видов в словаре ровно один: любой другой вид по умолчанию занимает зону.
        let squares = ScopeKind::ALL
            .iter()
            .filter(|kind| kind.picture_shape() == ScopePictureShape::Square)
            .count();
        assert_eq!(squares, 1);
    }
}

/// Стандарт осей вектороскопа; меняется в меню раздела калибровки на ходу (решение 185),
/// поэтому это параметр запроса, а не вариант ресурсов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorscopeAxes {
    /// Цветоразностные оси Cb/Cr по коэффициентам BT.709 — стандарт видеомонтажа (цели полос 75 %,
    /// линия тона кожи).
    YCbCrBt709,
    /// Оси a/b пространства OKLab — те же, в которых работают ручки цвета вида (14a).
    OklabAb,
}
