//! Сцена-«пробник» стенда насыщения: кадр, который проходит ВЕСЬ диапазон входа ISP.
//!
//! # Зачем (простыми словами)
//! Обычная синтетическая сцена (`SceneContent::Natural`) — плавный градиент средней яркости с лёгким шумом: тёмные
//! тени, пересветы и насыщенные цвета в ней почти не встречаются. Для неточной оптимизации (замена функции на более
//! дешёвую) этого мало: «наибольшая разница ≤ 1 младшего разряда» ничего не доказывает, если ветки шейдера, где
//! разница вырастает (около чёрного, около пересвета, у границы охвата), ни разу не исполнялись. Пробник
//! делит кадр на горизонтальные полосы, каждая из которых нагружает свою часть диапазона:
//!
//! | Полоса | Что в ней | Какие ветви шейдера нагружает |
//! |---|---|---|
//! | 0 | серая рампа от чёрного до пересвета (до 1,35 белого сенсора) | кривая тона на всей яркости, обрезка баланса |
//! | 1 | рампа около чёрного (0…2 % света) | носок кривой, линейный участок OETF, квантование |
//! | 2 | круг оттенков, полная насыщенность, средняя яркость | сжатие охвата, цвет вида |
//! | 3 | круг оттенков, полная насыщенность, около чёрного | цветность на малой яркости, возврат оттенка |
//! | 4 | круг оттенков, яркий, один канал пересвечен | обрезка баланса, плечо кривой |
//! | 5 | шахматка чёрный / белый по ячейкам 2×2 | края, резкие переходы между соседними ячейками |
//!
//! Свет задан для каждой ячейки CFA 2×2 одной тройкой «камера RGB» (линейной, 0…1 от размаха сенсора): внутри
//! ячейки все четыре отсчёта получают свой цвет по мозаике RGGB, так что демозаик видит именно этот цвет.

use camera_contract::geometry::PixelSize;

/// Сколько горизонтальных полос в кадре (таблица в описании модуля).
pub(super) const PROBE_BAND_COUNT: u32 = 6;
/// Во сколько раз самая яркая точка серой рампы выше белого сенсора (часть кадра уходит в пересвет).
const RAMP_OVEREXPOSURE: f64 = 1.35;
/// Верхняя граница света в полосе «около чёрного».
const NEAR_BLACK_LIGHT: f64 = 0.02;
/// Яркость колец оттенков: средняя, тёмная, яркая (доля размаха сенсора у самого яркого канала).
const MID_VALUE: f64 = 0.35;
const DARK_VALUE: f64 = 0.03;
const BRIGHT_VALUE: f64 = 0.9;
/// Во сколько раз самый яркий канал яркого кольца превышает белый (кольцо уходит в пересвет по одному каналу).
const BRIGHT_RING_OVEREXPOSURE: f64 = 1.25;

/// Линейный «камерный» свет R, G, B ячейки CFA с индексами `(cell_x, cell_y)` в кадре `size` (отсчёты 0…1 от размаха
/// сенсора; значения выше 1 станут пересветом после перевода в коды). Функция чистая: один вход — один выход.
pub(super) fn probe_cell_light(cell_x: u32, cell_y: u32, size: PixelSize) -> [f64; 3] {
    let cells_wide = f64::from((size.width / 2).max(2) - 1);
    let cells_high = (size.height / 2).max(1);
    let across = f64::from(cell_x) / cells_wide;
    // Полоса и положение внутри неё (0…1 сверху вниз) — для полос, где вторая ось тоже что-то меняет.
    let band = (cell_y * PROBE_BAND_COUNT / cells_high).min(PROBE_BAND_COUNT - 1);
    let band_top = (band * cells_high).div_ceil(PROBE_BAND_COUNT);
    let band_height = ((band + 1) * cells_high).div_ceil(PROBE_BAND_COUNT) - band_top;
    let down = f64::from(cell_y - band_top) / f64::from(band_height.max(1));
    match band {
        0 => {
            // Рампа с квадратичной шкалой: плотнее в тенях, где разница заметнее; хвост уходит за белый.
            let light = RAMP_OVEREXPOSURE * across * across;
            [light; 3]
        }
        1 => [NEAR_BLACK_LIGHT * across * (0.5 + 0.5 * down); 3],
        2 => hue_color(across, MID_VALUE * (0.4 + 0.6 * down)),
        3 => hue_color(across, DARK_VALUE * (0.3 + 0.7 * down)),
        4 => hue_color(across, BRIGHT_VALUE + (BRIGHT_RING_OVEREXPOSURE - BRIGHT_VALUE) * down),
        _ => {
            if (cell_x / 2 + cell_y / 2).is_multiple_of(2) {
                [0.0; 3]
            } else {
                [1.0; 3]
            }
        }
    }
}

/// Цвет круга оттенков: `hue` 0…1 по кругу, насыщенность полная, самый яркий канал равен `value`.
fn hue_color(hue: f64, value: f64) -> [f64; 3] {
    let sector = (hue * 6.0).floor().min(5.0);
    let fraction = hue * 6.0 - sector;
    let (rising, falling) = (fraction, 1.0 - fraction);
    let (red, green, blue) = match sector as u32 {
        0 => (1.0, rising, 0.0),
        1 => (falling, 1.0, 0.0),
        2 => (0.0, 1.0, rising),
        3 => (0.0, falling, 1.0),
        4 => (rising, 0.0, 1.0),
        _ => (1.0, 0.0, falling),
    };
    [red * value, green * value, blue * value]
}

/// Какой из каналов R, G, B несёт отсчёт позиции `(x, y)` мозаики RGGB.
pub(super) fn probe_channel_of_sample(x: u32, y: u32) -> usize {
    match (x & 1, y & 1) {
        (0, 0) => 0,
        (1, 1) => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: PixelSize = PixelSize::new(512, 384);

    /// Все ячейки полосы `band`.
    fn band_cells(band: u32) -> Vec<[f64; 3]> {
        let (cells_wide, cells_high) = (SIZE.width / 2, SIZE.height / 2);
        (0..cells_high)
            .filter(|&cell_y| (cell_y * PROBE_BAND_COUNT / cells_high).min(PROBE_BAND_COUNT - 1) == band)
            .flat_map(|cell_y| (0..cells_wide).map(move |cell_x| probe_cell_light(cell_x, cell_y, SIZE)))
            .collect()
    }

    #[test]
    fn gray_ramp_goes_from_black_past_white() {
        let cells = band_cells(0);
        assert_eq!(cells.first().copied(), Some([0.0; 3]), "рампа начинается с чёрного");
        let brightest = cells.iter().map(|rgb| rgb[0]).fold(0.0, f64::max);
        assert!(brightest > 1.3, "хвост рампы за белым сенсора: {brightest}");
        assert!(
            cells.iter().all(|rgb| rgb[0] == rgb[1] && rgb[1] == rgb[2]),
            "рампа серая"
        );
    }

    #[test]
    fn near_black_band_stays_near_black_and_is_not_empty() {
        let cells = band_cells(1);
        let brightest = cells.iter().map(|rgb| rgb[0]).fold(0.0, f64::max);
        assert!(brightest > 0.005 && brightest <= NEAR_BLACK_LIGHT, "{brightest}");
    }

    #[test]
    fn hue_rings_cover_every_hue_sector_with_saturated_colors() {
        for band in [2, 3, 4] {
            let cells = band_cells(band);
            // Насыщенность полная: хотя бы один канал нулевой у каждой ячейки кольца.
            assert!(cells.iter().all(|rgb| rgb.contains(&0.0)), "полоса {band}");
            // Все шесть секторов оттенков представлены: доминирует каждый из каналов и каждая пара.
            let has = |predicate: &dyn Fn(&[f64; 3]) -> bool| cells.iter().any(predicate);
            assert!(has(&|c| c[0] > c[1] && c[1] > c[2]), "красно-жёлтый, полоса {band}");
            assert!(has(&|c| c[1] > c[0] && c[0] > c[2]), "жёлто-зелёный, полоса {band}");
            assert!(has(&|c| c[1] > c[2] && c[2] > c[0]), "зелёно-голубой, полоса {band}");
            assert!(has(&|c| c[2] > c[1] && c[1] > c[0]), "голубо-синий, полоса {band}");
            assert!(has(&|c| c[2] > c[0] && c[0] > c[1]), "сине-пурпурный, полоса {band}");
            assert!(has(&|c| c[0] > c[2] && c[2] > c[1]), "пурпурно-красный, полоса {band}");
        }
    }

    #[test]
    fn bright_ring_overexposes_one_channel_and_dark_ring_stays_dark() {
        let bright_max = band_cells(4).iter().flatten().copied().fold(0.0, f64::max);
        assert!(bright_max > 1.0, "самый яркий канал за белым: {bright_max}");
        let dark_max = band_cells(3).iter().flatten().copied().fold(0.0, f64::max);
        assert!(dark_max <= DARK_VALUE, "{dark_max}");
    }

    #[test]
    fn checker_band_alternates_black_and_white_and_all_bands_are_distinct_in_frame() {
        let cells = band_cells(5);
        assert!(cells.contains(&[0.0; 3]) && cells.contains(&[1.0; 3]));
        // В кадре действительно есть все шесть полос: самая нижняя ячейка — шахматка, самая верхняя — рампа.
        assert_eq!(probe_cell_light(0, 0, SIZE), [0.0; 3]);
        let last_row = SIZE.height / 2 - 1;
        let bottom: Vec<[f64; 3]> = (0..8).map(|x| probe_cell_light(x, last_row, SIZE)).collect();
        assert!(bottom.iter().all(|rgb| rgb == &[0.0; 3] || rgb == &[1.0; 3]));
    }

    #[test]
    fn mosaic_is_rggb() {
        let channels: Vec<usize> = [(0, 0), (1, 0), (0, 1), (1, 1), (2, 0), (3, 3)]
            .iter()
            .map(|&(x, y)| probe_channel_of_sample(x, y))
            .collect();
        assert_eq!(channels, vec![0, 1, 1, 2, 0, 2]);
    }
}
