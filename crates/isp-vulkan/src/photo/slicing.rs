//! Геометрия проявления «по полосам» — чистая арифметика без GPU.
//!
//! Снимок проявляется не целым кадром, а областью `crop` плюс ореол (halo), нарезанной на полосы по строкам.
//! Каждая полоса — самостоятельная мини-картинка (область полосы), которую конвейер проявляет как целый
//! кадр; строки ореола нужны, чтобы результат внутри полосы был БИТ-В-БИТ равен проявлению целого кадра.
//!
//! # Почему ореол равен 12 пикселям
//! Выходной пиксель зависит от мозаики в окрестности, радиус которой складывается по цепочке проходов Menon
//! 2007 (стенд V4, `shaders/quality_menon_*.comp`); считаем от выхода назад, «радиус» — на сколько пикселей
//! (по любой оси) пиксель области проявления нужен выходному пикселю:
//!
//! | проход (что вычисляет)                 | читает соседей на | радиус его результата для выхода |
//! |----------------------------------------|-------------------|----------------------------------|
//! | выход, уровни (поточечные)             | 0                 | 0                                |
//! | 7: R/B в чужих узлах (уточнение)       | ±1 (R−B)          | 0                                |
//! | 6: R/B в зелёных узлах (уточнение)     | ±1                | 1                                |
//! | 5: зелёный в R/B-узлах (уточнение)     | ±1                | 2                                |
//! | 4: R/B в чужих узлах (черновой)        | ±1                | 3                                |
//! | 3: R/B в зелёных узлах (черновой)      | ±1 (зелёный, R/B) | 4                                |
//! | 2б: решение H/V и зелёный              | ±2 (классификатор)| 5                                |
//! | 2а: модули цветовых разностей          | +2                | 7                                |
//! | 1: оценки зелёного по строке/столбцу   | ±2                | 9                                |
//! | мозаика (читается проходом 1)          | —                 | 11                               |
//!
//! Итого 11 — СТРОГАЯ верхняя оценка (сумма худших шагов каждого прохода). Ореол берётся ЧЁТНЫМ (12):
//! начало области проявления обязано быть на границе ячейки CFA 2×2 (иначе раскладка цветов сдвинется), а
//! чётное смещение сохраняет её. Измерение на эталоне CPU (`tests/photo_halo.rs`: возмущаем отсчёт мозаики,
//! смотрим, где меняется результат) даёт 8 — меньше оценки, потому что шаги проходов идут вдоль осей и
//! чередуют чётность узлов, и крайние пути не совмещаются. Ореол выбран по строгой оценке, а не по
//! измерению: измерение — нижняя граница (зависит от данных), оценка — гарантия. Паритет «полосы = целый
//! кадр» проверяет `tests/photo_slices.rs` (ореол 4 его ломает).
//!
//! # Края кадра
//! Граница полезной области — настоящий край: проходы зеркалят отсчёты через него, и полоса, область которой
//! упирается в этот край, зеркалит ровно так же, как целый кадр. Граница области, прошедшая ВНУТРИ
//! кадра, зеркалит «неправильно», но порча распространяется не дальше радиуса 11 внутрь, а выход отстоит от
//! неё не меньше чем на 12: ореол «съедает» порчу. Поэтому ореол добавляется только там, где позади него ещё
//! есть кадр (`saturating_sub` и `min` по размеру).

use camera_contract::geometry::{PixelRect, PixelSize};

use super::error::PhotoError;

/// Ореол, пиксели: на столько область проявления шире выходной области с каждой стороны (где позади есть кадр).
/// Вывод числа — в документации модуля. Чётное: сохраняет раскладку CFA.
pub const DEMOSAIC_HALO: u32 = 12;

// Ореол чётный (сохраняет раскладку CFA) и не меньше строгой оценки радиуса зависимости (11, см. таблицу выше).
const _: () = assert!(DEMOSAIC_HALO.is_multiple_of(2) && DEMOSAIC_HALO >= 11);

/// Индексы и размеры передаются шейдерам как f32 (push-константы `PlaneGeometry`): точны до 2^24.
const MAX_EXACT_INDEX: u32 = 1 << 24;

/// Одна полоса проявления.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlicePlan {
    /// Первая строка области полосы в полезной области кадра (чётная).
    pub region_top: u32,
    /// Число строк области полосы (чётное).
    pub region_rows: u32,
    /// Номер строки области, с которой начинаются выходные строки (там кончается верхний ореол).
    pub first_output_row: u32,
    /// Число выходных строк полосы.
    pub output_rows: u32,
    /// Строка картинки снимка, куда ложится первая выходная строка.
    pub destination_row: u32,
}

/// План проявления: общая область по горизонтали и полосы по вертикали.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevelopmentPlan {
    /// Левый край области проявления в полезной области кадра (чётный).
    pub region_left: u32,
    /// Ширина области проявления (чётная).
    pub region_width: u32,
    /// Отступ выходного окна от левого края области (левый ореол).
    pub window_left: u32,
    /// Размер картинки снимка.
    pub crop_size: PixelSize,
    /// Полосы сверху вниз; вместе их выходные строки покрывают `crop` ровно один раз.
    pub slices: Vec<SlicePlan>,
}

/// Наибольшее число строк области полосы высотой `slice_rows`: полоса + ореол с двух сторон + по одной строке
/// выравнивания на чётную границу с каждой стороны. Столько строк должны вмещать плоскости.
pub fn max_region_rows(slice_rows: u32) -> u32 {
    slice_rows + 2 * DEMOSAIC_HALO + 2
}

/// Строит план. `valid` — размер полезной области (чётный), `crop` — область снимка В КООРДИНАТАХ ПОЛЕЗНОЙ
/// ОБЛАСТИ (не буфера кадра), `slice_rows` — высота полосы (чётная, ≥ 2).
pub fn plan_development(valid: PixelSize, crop: PixelRect, slice_rows: u32) -> Result<DevelopmentPlan, PhotoError> {
    let invalid = |what: String| Err(PhotoError::InvalidInput { what });
    if valid.width < 2 || valid.height < 2 || !valid.width.is_multiple_of(2) || !valid.height.is_multiple_of(2) {
        return invalid(format!(
            "полезная область {}x{}: нужен чётный размер не меньше 2x2 (ячейка CFA)",
            valid.width, valid.height
        ));
    }
    if valid.width >= MAX_EXACT_INDEX || valid.height >= MAX_EXACT_INDEX {
        return invalid(format!(
            "полезная область {}x{} больше предела {MAX_EXACT_INDEX} точек",
            valid.width, valid.height
        ));
    }
    if slice_rows < 2 || !slice_rows.is_multiple_of(2) {
        return invalid(format!("высота полосы {slice_rows}: нужно чётное число не меньше 2"));
    }
    let crop_right = u64::from(crop.left) + u64::from(crop.size.width);
    let crop_bottom = u64::from(crop.top) + u64::from(crop.size.height);
    if crop.size.width == 0
        || crop.size.height == 0
        || crop_right > u64::from(valid.width)
        || crop_bottom > u64::from(valid.height)
    {
        return invalid(format!(
            "область снимка {crop:?} пуста или выходит за полезную область {}x{}",
            valid.width, valid.height
        ));
    }

    // По горизонтали область одна на все полосы: crop ± ореол, края — на границах ячеек CFA.
    let region_left = floor_even(crop.left.saturating_sub(DEMOSAIC_HALO));
    let region_right = ceil_even(crop.left + crop.size.width + DEMOSAIC_HALO).min(valid.width);

    let mut slices = Vec::new();
    let mut destination_row = 0;
    while destination_row < crop.size.height {
        let output_rows = slice_rows.min(crop.size.height - destination_row);
        let output_top = crop.top + destination_row;
        let region_top = floor_even(output_top.saturating_sub(DEMOSAIC_HALO));
        let region_bottom = ceil_even(output_top + output_rows + DEMOSAIC_HALO).min(valid.height);
        slices.push(SlicePlan {
            region_top,
            region_rows: region_bottom - region_top,
            first_output_row: output_top - region_top,
            output_rows,
            destination_row,
        });
        destination_row += output_rows;
    }
    Ok(DevelopmentPlan {
        region_left,
        region_width: region_right - region_left,
        window_left: crop.left - region_left,
        crop_size: crop.size,
        slices,
    })
}

/// Ближайшее чётное не больше `value`.
fn floor_even(value: u32) -> u32 {
    value & !1
}

/// Ближайшее чётное не меньше `value`.
fn ceil_even(value: u32) -> u32 {
    (value + 1) & !1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: u32, top: u32, width: u32, height: u32) -> PixelRect {
        PixelRect {
            left,
            top,
            size: PixelSize::new(width, height),
        }
    }

    const VALID: PixelSize = PixelSize::new(200, 120);

    /// Проверяет инварианты любого плана: покрытие crop ровно один раз, чётные границы, запас ореола там,
    /// где позади области есть кадр.
    fn assert_plan_invariants(valid: PixelSize, crop: PixelRect, slice_rows: u32, plan: &DevelopmentPlan) {
        assert_eq!(plan.crop_size, crop.size);
        assert_eq!(plan.region_left % 2, 0);
        assert_eq!(plan.region_width % 2, 0);
        assert!(plan.region_left + plan.region_width <= valid.width);
        // Горизонтальные запасы.
        assert_eq!(plan.region_left + plan.window_left, crop.left);
        if plan.region_left > 0 {
            assert!(plan.window_left >= DEMOSAIC_HALO, "левый ореол {}", plan.window_left);
        }
        let right_margin = plan.region_left + plan.region_width - (crop.left + crop.size.width);
        if plan.region_left + plan.region_width < valid.width {
            assert!(right_margin >= DEMOSAIC_HALO, "правый ореол {right_margin}");
        }
        // Вертикально: полосы подряд без дыр и перекрытий.
        let mut next_destination = 0;
        for slice in &plan.slices {
            assert_eq!(slice.destination_row, next_destination);
            assert!(slice.output_rows >= 1 && slice.output_rows <= slice_rows);
            next_destination += slice.output_rows;
            assert_eq!(slice.region_top % 2, 0);
            assert_eq!(slice.region_rows % 2, 0);
            assert!(slice.region_rows <= max_region_rows(slice_rows));
            let region_bottom = slice.region_top + slice.region_rows;
            assert!(region_bottom <= valid.height);
            let output_top = crop.top + slice.destination_row;
            assert_eq!(slice.region_top + slice.first_output_row, output_top);
            if slice.region_top > 0 {
                assert!(slice.first_output_row >= DEMOSAIC_HALO, "верхний ореол {slice:?}");
            }
            let bottom_margin = region_bottom - (output_top + slice.output_rows);
            if region_bottom < valid.height {
                assert!(bottom_margin >= DEMOSAIC_HALO, "нижний ореол {slice:?}");
            }
        }
        assert_eq!(next_destination, crop.size.height, "полосы покрывают crop целиком");
    }

    #[test]
    fn every_crop_position_and_slice_height_keeps_the_invariants() {
        for slice_rows in [2, 4, 8, 12, 14, 24, 64, 4096] {
            for (left, top, width, height) in [
                (0, 0, 200, 120),
                (0, 0, 1, 1),
                (1, 1, 5, 7),
                (3, 5, 100, 61),
                (50, 40, 100, 40),
                (199, 119, 1, 1),
                (100, 0, 100, 120),
                (0, 60, 200, 60),
                (13, 17, 150, 99),
            ] {
                let crop = rect(left, top, width, height);
                let plan = plan_development(VALID, crop, slice_rows).unwrap();
                assert_plan_invariants(VALID, crop, slice_rows, &plan);
            }
        }
    }

    #[test]
    fn full_frame_with_one_tall_slice_is_a_single_region_equal_to_the_frame() {
        let plan = plan_development(VALID, rect(0, 0, 200, 120), 4096).unwrap();
        assert_eq!(plan.slices.len(), 1);
        assert_eq!((plan.region_left, plan.region_width, plan.window_left), (0, 200, 0));
        let slice = plan.slices[0];
        assert_eq!(
            (slice.region_top, slice.region_rows, slice.first_output_row),
            (0, 120, 0)
        );
    }

    #[test]
    fn slice_count_is_ceiling_of_crop_height_over_slice_rows_and_last_slice_is_short() {
        let plan = plan_development(VALID, rect(0, 0, 200, 120), 50).unwrap();
        let rows: Vec<u32> = plan.slices.iter().map(|slice| slice.output_rows).collect();
        assert_eq!(rows, vec![50, 50, 20]);
    }

    #[test]
    fn interior_crop_gets_halo_on_all_four_sides() {
        let crop = rect(60, 50, 40, 20);
        let plan = plan_development(VALID, crop, 4096).unwrap();
        assert_eq!((plan.region_left, plan.region_width), (48, 64));
        let slice = plan.slices[0];
        assert_eq!(
            (slice.region_top, slice.region_rows, slice.first_output_row),
            (38, 44, 12)
        );
    }

    #[test]
    fn odd_crop_origin_widens_region_to_even_cell_boundary() {
        let plan = plan_development(VALID, rect(61, 51, 40, 20), 4096).unwrap();
        // 61 − 12 = 49 → 48; 61 + 40 + 12 = 113 → 114.
        assert_eq!((plan.region_left, plan.region_width, plan.window_left), (48, 66, 13));
        let slice = plan.slices[0];
        assert_eq!((slice.region_top, slice.first_output_row), (38, 13));
    }

    #[test]
    fn invalid_inputs_are_refused_with_input_errors() {
        let ok = rect(0, 0, 10, 10);
        for (valid, crop, slice_rows) in [
            (PixelSize::new(201, 120), ok, 8),
            (PixelSize::new(200, 1), ok, 8),
            (VALID, rect(0, 0, 0, 10), 8),
            (VALID, rect(0, 0, 10, 0), 8),
            (VALID, rect(195, 0, 10, 10), 8),
            (VALID, rect(0, 115, 10, 10), 8),
            (VALID, rect(u32::MAX, 0, 10, 10), 8),
            (VALID, ok, 0),
            (VALID, ok, 3),
        ] {
            assert!(
                matches!(
                    plan_development(valid, crop, slice_rows),
                    Err(PhotoError::InvalidInput { .. })
                ),
                "{valid:?} {crop:?} {slice_rows}"
            );
        }
    }

    #[test]
    fn phone_frame_in_default_slices_bounds_plane_memory() {
        // 4096x3072, полоса 256: плоскости вмещают полосу + ореол, а не кадр.
        let plan = plan_development(PixelSize::new(4096, 3072), rect(0, 0, 4096, 3072), 256).unwrap();
        assert_eq!(plan.slices.len(), 12);
        assert!(
            plan.slices
                .iter()
                .all(|slice| slice.region_rows <= max_region_rows(256))
        );
        assert_eq!(max_region_rows(256), 282);
    }
}
