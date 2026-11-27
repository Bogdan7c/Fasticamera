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
    /// Гистограмма (сессия 14s): сколько пикселей кадра на каждом уровне сигнала. Режим (RGB наложением, PARADE
    /// секциями, Y яркость) — параметр запроса [`HistogramMode`](crate::HistogramMode), страница одна.
    Histogram,
    /// RGB waveform: R', G' и B' по столбцам кадра тремя следами, наложенными друг на друга (серый — белый след,
    /// цветной сдвиг виден цветом): где тени и света, что упёрлось в белое или чёрное, и какого цвета перекос.
    Waveform,
    /// Яркостный waveform (Luma): один белый след яркости Y' (BT.709 по кодам сигнала) по столбцам кадра — экспозиция
    /// без цветового шума трёх следов (решение 321: отдельная страница рядом с RGB WAVEFORM).
    LumaWaveform,
    /// То же для R', G' и B' рядом секциями: сдвиг цвета в тенях или светах (серый — три одинаковые картинки).
    RgbParade,
    /// Цветность по кругу: направление и сила цвета, серый — точка в центре.
    Vectorscope,
    /// Диаграмма цветности CIE (сессия 14s, решение 332): где в плоскости цветностей лежат цвета кадра относительно
    /// спектрального локуса и треугольников охвата. Шкалу (xy 1931 или u′v′ 1976) выбирает параметр запроса
    /// [`CieDiagram`], страница одна.
    Cie,
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
    /// Порядок — порядок страниц в разделе калибровки.
    pub const ALL: [ScopeKind; 6] = [
        ScopeKind::Histogram,
        ScopeKind::Waveform,
        ScopeKind::LumaWaveform,
        ScopeKind::RgbParade,
        ScopeKind::Vectorscope,
        ScopeKind::Cie,
    ];

    /// Какой формы картинка нужна прибору. `match` без `_`: новый вид не соберётся без своей формы.
    pub fn picture_shape(self) -> ScopePictureShape {
        match self {
            ScopeKind::Histogram | ScopeKind::Waveform | ScopeKind::LumaWaveform | ScopeKind::RgbParade => {
                ScopePictureShape::FillsZone
            }
            ScopeKind::Vectorscope | ScopeKind::Cie => ScopePictureShape::Square,
        }
    }

    /// Линии уровней, которые прибор рисует в своей сетке и которые экран подписывает числами слева. У вектороскопа
    /// и гистограммы их нет (пустой список): у гистограммы уровень лежит по горизонтали, её вертикальные линии
    /// 0 / 25 / 50 / 75 / 100 % — примитивы сетки внутри картинки. `match` без `_`: новый прибор не соберётся, пока не скажет, есть ли у него уровни.
    pub fn level_lines(self) -> &'static [LevelLine] {
        match self {
            ScopeKind::Waveform | ScopeKind::LumaWaveform | ScopeKind::RgbParade => &WAVEFORM_LEVEL_LINES,
            ScopeKind::Histogram | ScopeKind::Vectorscope | ScopeKind::Cie => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cie_is_the_last_page_and_its_diagram_is_stored_in_snake_case() {
        assert_eq!(ScopeKind::ALL[5], ScopeKind::Cie);
        assert!(ScopeKind::Cie.level_lines().is_empty());
        assert_eq!(CieDiagram::default(), CieDiagram::Xy1931);
        assert_eq!(
            serde_json::to_string(&CieDiagram::UvPrime1976).unwrap(),
            "\"uv_prime1976\""
        );
        assert_eq!(
            serde_json::from_str::<CieDiagram>("\"xy1931\"").unwrap(),
            CieDiagram::Xy1931
        );
    }

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
    fn the_histogram_is_the_first_page_and_has_no_horizontal_level_lines() {
        assert_eq!(ScopeKind::ALL[0], ScopeKind::Histogram);
        assert_eq!(ScopeKind::Histogram.picture_shape(), ScopePictureShape::FillsZone);
        assert!(ScopeKind::Histogram.level_lines().is_empty());
        assert!(ScopeKind::Vectorscope.level_lines().is_empty());
        assert_eq!(ScopeKind::Waveform.level_lines().len(), 5);
    }

    #[test]
    fn the_histogram_mode_is_stored_in_snake_case_and_defaults_to_rgb() {
        assert_eq!(HistogramMode::default(), HistogramMode::Rgb);
        assert_eq!(serde_json::to_string(&HistogramMode::Parade).unwrap(), "\"parade\"");
        assert_eq!(
            serde_json::from_str::<HistogramMode>("\"luma\"").unwrap(),
            HistogramMode::Luma
        );
    }

    #[test]
    fn only_the_vectorscope_and_the_cie_need_a_square_picture() {
        assert_eq!(ScopeKind::Waveform.picture_shape(), ScopePictureShape::FillsZone);
        assert_eq!(ScopeKind::LumaWaveform.picture_shape(), ScopePictureShape::FillsZone);
        assert_eq!(ScopeKind::RgbParade.picture_shape(), ScopePictureShape::FillsZone);
        assert_eq!(ScopeKind::Vectorscope.picture_shape(), ScopePictureShape::Square);
        assert_eq!(ScopeKind::Cie.picture_shape(), ScopePictureShape::Square);
        // Квадратных видов в словаре ровно два (вектороскоп и CIE): любой другой вид занимает зону.
        let squares: Vec<ScopeKind> = ScopeKind::ALL
            .into_iter()
            .filter(|kind| kind.picture_shape() == ScopePictureShape::Square)
            .collect();
        assert_eq!(squares, [ScopeKind::Vectorscope, ScopeKind::Cie]);
    }
}

/// Режим гистограммы (сессия 14s): одна страница HISTOGRAM, три способа показать те же отсчёты. Параметр запроса, а не
/// ресурсов: от режима меняются только число плоскостей счётчиков (три или одна) и способ рисования.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistogramMode {
    /// Три графика R', G', B' друг поверх друга цветами каналов; где совпали — белый (как RGB waveform).
    #[default]
    Rgb,
    /// Те же три графика полосами друг над другом на всю ширину: R сверху, G, B снизу (как YRGB у Resolve, решение 337).
    Parade,
    /// Один белый график яркости Y' (BT.709 по кодам сигнала).
    Luma,
}

/// Шкала диаграммы CIE (решение 332): параметр запроса, а не ресурсов — тот же счёт ячеек, другие числа правила.
/// Хранится при любом типе прибора (свайп не теряет выбор); старые записи без поля читаются как xy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CieDiagram {
    /// CIE 1931 xy: `x = X/(X+Y+Z)`, `y = Y/(X+Y+Z)` (умолчание).
    #[default]
    Xy1931,
    /// CIE 1976 u′v′: `u′ = 4X/(X+15Y+3Z)`, `v′ = 9Y/(X+15Y+3Z)`; расстояния ближе к воспринимаемым.
    UvPrime1976,
}

/// Увеличение вектороскопа (решение 325, кнопка «2×» на полоске вектороскопа; как Show 2X zoom в Resolve): центр
/// вдвое крупнее — видны слабые перекосы белого и кожи. Это параметр запроса, а не ресурсов: меняются только числа
/// правила попадания, проходы и картинка те же.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorscopeZoom {
    /// Обычный масштаб: все цели 75 % и 100 % внутри круга (решение 324).
    #[default]
    Normal,
    /// Вдвое крупнее: цветность на краю вдвое меньше, цели за краем не рисуются.
    Double,
}

impl VectorscopeZoom {
    /// Во сколько раз крупнее обычного масштаба.
    pub const fn magnification(self) -> f32 {
        match self {
            VectorscopeZoom::Normal => 1.0,
            VectorscopeZoom::Double => 2.0,
        }
    }

    /// Другое положение кнопки «2×».
    pub const fn toggled(self) -> Self {
        match self {
            VectorscopeZoom::Normal => VectorscopeZoom::Double,
            VectorscopeZoom::Double => VectorscopeZoom::Normal,
        }
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
