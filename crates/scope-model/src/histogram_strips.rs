//! Вид и раскладка гистограммы (решение 337): настройки рисования и вертикальные полосы PARADE.
//!
//! PARADE рисуется полосами одна над другой (R сверху, B снизу), каждая на всю ширину со своей базовой линией и своей
//! высотой столбиков; у RGB (наложение) и Y полоса одна на всю высоту. Раскладка считается здесь один раз и для
//! эталона ([`crate::histogram_bars`]), и для сетки ([`crate::graticule_layout`]), поэтому разделители и графики не
//! расходятся. Шейдер (`scope_render_body.glsl`, `locate_histogram_strip`) повторяет `locate` с тем же порядком f32.

/// Вид столбиков гистограммы: числа из [`crate::RenderSettings`], которые меняют только рисование, но не счётчики.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistogramLook {
    /// Непрозрачность заливки `0..=1` (на пикселе, полностью закрытом столбиком одного канала).
    pub fill_opacity: f32,
    /// Множитель цвета заливки `0..=1`: затемнение заливки при ярком цвете линии.
    pub fill_tint: f32,
    /// Толщина линии верха, пикселей; `0` — линии нет.
    pub edge_width_px: f32,
    /// Зазор между полосами, долей высоты картинки.
    pub strip_gap: f32,
}

/// Полоса, в которую попал пиксель.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripPosition {
    /// Номер полосы сверху (0 — первая).
    pub index: u32,
    /// Расстояние от центра пикселя до базовой линии полосы вверх, пикселей (всегда `>= 0`).
    pub distance_to_baseline: f32,
    /// Высота полосы, пикселей: во столько раз растёт столбик полной высоты.
    pub height_px: f32,
}

/// Раскладка полос: сколько их и какой между ними зазор.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistogramStrips {
    count: u32,
    gap: f32,
}

impl HistogramStrips {
    /// Раскладка из `count` полос (не меньше одной) с зазором `gap` долей высоты картинки.
    pub fn new(count: u32, gap: f32) -> Self {
        Self {
            count: count.max(1),
            gap,
        }
    }

    /// Число полос.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Верх и низ полосы `index` в нормированных координатах картинки (`0..=1`, y вниз); низ — базовая линия.
    /// Номер сверх числа полос берёт последнюю полосу.
    pub fn normalized_span(&self, index: u32) -> (f32, f32) {
        let index = index.min(self.count - 1);
        let strip = (1.0 - (self.count - 1) as f32 * self.gap) / self.count as f32;
        let top = index as f32 * (strip + self.gap);
        (top, top + strip)
    }

    /// Полоса пикселя с центром по вертикали `centre_y` в картинке высотой `height_px`; `None` — пиксель в зазоре между
    /// полосами (или полосы не помещаются). ПОРЯДОК ОПЕРАЦИЙ f32 тот же, что в шейдере.
    pub fn locate(&self, centre_y: f32, height_px: f32) -> Option<StripPosition> {
        let gap_px = self.gap * height_px;
        let strip_px = (height_px - (self.count - 1) as f32 * gap_px) / self.count as f32;
        if strip_px <= 0.0 {
            return None;
        }
        let pitch = strip_px + gap_px;
        // Номер полосы — по шагу «полоса + зазор»; сверх последней (округление у нижнего края) берётся последняя.
        let index = ((centre_y / pitch) as u32).min(self.count - 1);
        let baseline = index as f32 * pitch + strip_px;
        let distance_to_baseline = baseline - centre_y;
        if distance_to_baseline < 0.0 {
            return None;
        }
        Some(StripPosition {
            index,
            distance_to_baseline,
            height_px: strip_px,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_strip_takes_the_whole_height_and_no_pixel_falls_into_a_gap() {
        let strips = HistogramStrips::new(1, 0.05);
        assert_eq!(strips.normalized_span(0), (0.0, 1.0));
        let top = strips.locate(0.5, 100.0).unwrap();
        assert_eq!((top.index, top.distance_to_baseline, top.height_px), (0, 99.5, 100.0));
        assert_eq!(strips.locate(99.5, 100.0).unwrap().distance_to_baseline, 0.5);
    }

    #[test]
    fn three_strips_stack_top_to_bottom_with_gaps_between_them() {
        // Высота 100, зазор 5 % = 5 px: полоса (100 − 10) / 3 = 30 px, шаг 35.
        let strips = HistogramStrips::new(3, 0.05);
        let at = |y: f32| strips.locate(y, 100.0);
        let first = at(10.5).unwrap();
        assert_eq!((first.index, first.height_px), (0, 30.0));
        assert!((first.distance_to_baseline - 19.5).abs() < 1e-4);
        assert_eq!(at(34.5).map(|strip| strip.index), None, "зазор между первой и второй");
        assert_eq!(at(36.5).map(|strip| strip.index), Some(1));
        assert_eq!(at(71.5).map(|strip| strip.index), Some(2));
        assert_eq!(at(99.5).map(|strip| strip.index), Some(2));
        let (top, bottom) = strips.normalized_span(1);
        assert!(
            (top - 0.35).abs() < 1e-6 && (bottom - 0.65).abs() < 1e-6,
            "{top} {bottom}"
        );
        assert!((strips.normalized_span(2).1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn strips_that_do_not_fit_locate_nothing() {
        // Зазоры съедают всю высоту.
        assert_eq!(HistogramStrips::new(3, 0.6).locate(50.0, 100.0), None);
    }
}
