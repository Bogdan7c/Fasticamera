//! Генератор знакового поля расстояний (SDF) — свой, clean-room (решение 88).
//!
//! Один и тот же генератор строит три вида картинок:
//! * заливку по замкнутым контурам (буквы, иконки-заливки): знак — по правилу ненулевой обмотки;
//! * штрих по ломаным (иконки-контуры): расстояние до средней линии минус полуширина штриха.
//!
//! # Соглашения
//! * Координаты — в текселях ячейки, ось y вниз; центр текселя `(i, j)` лежит в `(i + 0.5, j + 0.5)`.
//! * Расстояние со знаком: внутри формы **отрицательное**, снаружи положительное (как в `ui-draw::shape`).
//! * Кодирование в байт ([`encode_distance`]): `128` — край, шаг линейный, `±spread` → `128 ± 127`.
//!   Значение `255` — «далеко снаружи», его же получают пустые места атласа.
//!
//! Алгоритм без оптимизаций сверх необходимого: для каждого отрезка обновляем только тексели в его
//! рамке, расширенной на `spread` (дальше расстояние всё равно обрезается), а знак заливки берём
//! построчным подсчётом пересечений луча — это O(отрезки × строки), а не O(тексели × отрезки).

/// Точка в координатах ячейки (текселях).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Размер сетки в текселях.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub width: usize,
    pub height: usize,
}

/// Как заканчивается свободный конец открытой ломаной штриха (атрибут `stroke-linecap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    /// Срез ровно в конечной точке.
    Butt,
    /// Полукруг радиусом в полуширину штриха.
    Round,
    /// Срез, отодвинутый на полуширину штриха.
    Square,
}

/// Ломаная линия штриха: точки по порядку и признак замкнутости.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokePolyline {
    pub points: Vec<Point>,
    /// Замкнутая ломаная: последний отрезок идёт обратно к первой точке, свободных концов нет.
    pub closed: bool,
}

/// Отрезок штриха с видом закрытия на каждом конце.
#[derive(Debug, Clone, Copy)]
struct StrokeSegment {
    start: Point,
    end: Point,
    start_cap: LineCap,
    end_cap: LineCap,
}

/// Допуск на «нулевую длину» отрезка, текселей.
const DEGENERATE_LENGTH: f64 = 1e-9;

/// Расстояние со знаком по замкнутым контурам; внутри — отрицательное, обрезано до `±spread`.
///
/// Контуры считаются замкнутыми неявно (последняя точка соединяется с первой). Правило заливки —
/// ненулевая обмотка: контур с обратным направлением обмотки вырезает дырку (буква «O»).
pub fn fill_distance_field(grid: GridSize, contours: &[Vec<Point>], spread: f64) -> Vec<f32> {
    let mut nearest = vec![spread as f32; grid.width * grid.height];
    for contour in contours {
        for index in 0..contour.len() {
            let start = contour[index];
            let end = contour[(index + 1) % contour.len()];
            raise_nearest_to_segment(&mut nearest, grid, start, end, spread);
        }
    }
    apply_winding_sign(&mut nearest, grid, contours);
    nearest
}

/// Расстояние со знаком для штриха шириной `2 * half_width`; внутри штриха — отрицательное.
///
/// Стыки отрезков внутри ломаной — круглые (`stroke-linejoin: round`), свободные концы — по `cap`.
pub fn stroke_distance_field(
    grid: GridSize,
    polylines: &[StrokePolyline],
    half_width: f64,
    cap: LineCap,
    spread: f64,
) -> Vec<f32> {
    let mut field = vec![spread as f32; grid.width * grid.height];
    for polyline in polylines {
        for segment in stroke_segments(polyline, cap) {
            lower_field_by_stroke_segment(&mut field, grid, segment, half_width, spread);
        }
    }
    field
}

/// Кодирует расстояние со знаком в байт: `128` — край, `±spread` — `128 ± 127`.
pub fn encode_distance(signed_distance: f32, spread: f64) -> u8 {
    let normalized = (f64::from(signed_distance) / spread).clamp(-1.0, 1.0);
    (128.0 + normalized * 127.0).round().clamp(0.0, 255.0) as u8
}

/// Обратное к [`encode_distance`]: расстояние со знаком в текселях по байту.
pub fn decode_distance(byte: u8, spread: f64) -> f64 {
    (f64::from(byte) - 128.0) / 127.0 * spread
}

/// Кодирует всё поле.
pub fn encode_field(field: &[f32], spread: f64) -> Vec<u8> {
    field
        .iter()
        .map(|&distance| encode_distance(distance, spread))
        .collect()
}

/// Разбивает ломаную на отрезки и расставляет виды закрытия: внутренние стыки круглые.
fn stroke_segments(polyline: &StrokePolyline, cap: LineCap) -> Vec<StrokeSegment> {
    let mut points: Vec<Point> = Vec::with_capacity(polyline.points.len());
    for &point in &polyline.points {
        // Подряд идущие совпадающие точки дают отрезки нулевой длины — они не рисуют ничего.
        let repeats_previous = points
            .last()
            .is_some_and(|last| distance_between(*last, point) < DEGENERATE_LENGTH);
        if !repeats_previous {
            points.push(point);
        }
    }
    // Замкнутая ломаная часто повторяет первую точку в конце — это не новый отрезок.
    if polyline.closed
        && points.len() > 1
        && let (Some(first), Some(last)) = (points.first(), points.last())
        && distance_between(*first, *last) < DEGENERATE_LENGTH
    {
        points.pop();
    }
    let segment_count = if polyline.closed {
        points.len()
    } else {
        points.len().saturating_sub(1)
    };
    (0..segment_count)
        .filter(|_| points.len() > 1)
        .map(|index| {
            let is_first = index == 0;
            let is_last = index + 1 == segment_count;
            StrokeSegment {
                start: points[index],
                end: points[(index + 1) % points.len()],
                // У замкнутой ломаной свободных концов нет: все стыки круглые.
                start_cap: if is_first && !polyline.closed {
                    cap
                } else {
                    LineCap::Round
                },
                end_cap: if is_last && !polyline.closed {
                    cap
                } else {
                    LineCap::Round
                },
            }
        })
        .collect()
}

/// Границы тексельной области вокруг отрезка с запасом `margin`, обрезанные по сетке.
fn texel_window(grid: GridSize, start: Point, end: Point, margin: f64) -> (usize, usize, usize, usize) {
    let low_x = (start.x.min(end.x) - margin).floor().max(0.0) as usize;
    let low_y = (start.y.min(end.y) - margin).floor().max(0.0) as usize;
    let high_x = ((start.x.max(end.x) + margin).ceil().max(0.0) as usize).min(grid.width);
    let high_y = ((start.y.max(end.y) + margin).ceil().max(0.0) as usize).min(grid.height);
    (low_x, low_y, high_x, high_y)
}

/// Центр текселя `(column, row)`.
fn texel_center(column: usize, row: usize) -> Point {
    Point::new(column as f64 + 0.5, row as f64 + 0.5)
}

/// Евклидово расстояние между точками.
fn distance_between(first: Point, second: Point) -> f64 {
    (first.x - second.x).hypot(first.y - second.y)
}

/// Расстояние от точки до отрезка (всегда неотрицательное).
fn distance_to_segment(point: Point, start: Point, end: Point) -> f64 {
    let along_x = end.x - start.x;
    let along_y = end.y - start.y;
    let length_squared = along_x * along_x + along_y * along_y;
    if length_squared < DEGENERATE_LENGTH * DEGENERATE_LENGTH {
        return distance_between(point, start);
    }
    let projection = (((point.x - start.x) * along_x + (point.y - start.y) * along_y) / length_squared).clamp(0.0, 1.0);
    distance_between(
        point,
        Point::new(start.x + projection * along_x, start.y + projection * along_y),
    )
}

/// Понижает `nearest` до расстояния до отрезка (беззнаковое расстояние заливки).
fn raise_nearest_to_segment(nearest: &mut [f32], grid: GridSize, start: Point, end: Point, spread: f64) {
    let (low_x, low_y, high_x, high_y) = texel_window(grid, start, end, spread);
    for row in low_y..high_y {
        for column in low_x..high_x {
            let distance = distance_to_segment(texel_center(column, row), start, end) as f32;
            let slot = &mut nearest[row * grid.width + column];
            if distance < *slot {
                *slot = distance;
            }
        }
    }
}

/// Превращает беззнаковое расстояние заливки в знаковое: внутри (обмотка ≠ 0) — минус.
///
/// Для каждой строки текселей собираем пересечения контуров с горизонталью через центры текселей
/// (полуоткрытое правило `y0 <= y < y1`, чтобы вершина на горизонтали не считалась дважды), сортируем
/// по x и идём слева направо: обмотка текселя — сумма направлений пересечений правее его центра.
fn apply_winding_sign(nearest: &mut [f32], grid: GridSize, contours: &[Vec<Point>]) {
    for row in 0..grid.height {
        let scan_y = row as f64 + 0.5;
        let mut crossings: Vec<(f64, i32)> = Vec::new();
        for contour in contours {
            for index in 0..contour.len() {
                let start = contour[index];
                let end = contour[(index + 1) % contour.len()];
                let start_above = start.y <= scan_y;
                let end_above = end.y <= scan_y;
                if start_above == end_above {
                    continue;
                }
                let fraction = (scan_y - start.y) / (end.y - start.y);
                let crossing_x = start.x + fraction * (end.x - start.x);
                let direction = if end.y > start.y { 1 } else { -1 };
                crossings.push((crossing_x, direction));
            }
        }
        if crossings.is_empty() {
            continue;
        }
        crossings.sort_by(|left, right| left.0.total_cmp(&right.0));
        let total_direction: i32 = crossings.iter().map(|crossing| crossing.1).sum();
        let mut passed_direction = 0;
        let mut next_crossing = 0;
        for column in 0..grid.width {
            let center_x = column as f64 + 0.5;
            while next_crossing < crossings.len() && crossings[next_crossing].0 <= center_x {
                passed_direction += crossings[next_crossing].1;
                next_crossing += 1;
            }
            // Обмотка по лучу вправо = сумма направлений пересечений правее центра текселя.
            let winding = total_direction - passed_direction;
            if winding != 0 {
                let slot = &mut nearest[row * grid.width + column];
                *slot = -*slot;
            }
        }
    }
}

/// Знаковое расстояние от точки до штриха одного отрезка.
///
/// Штрих отрезка — прямоугольник `[-e0, L + e1] × [-w/2, w/2]` в системе отрезка (вдоль, поперёк),
/// где `e` — вынос для `Square` и 0 для остальных, плюс круги на круглых концах. Объединение форм —
/// минимум их расстояний: снаружи это точное расстояние, внутри — не глубже настоящего.
fn stroke_segment_distance(point: Point, segment: StrokeSegment, half_width: f64) -> f64 {
    let along_x = segment.end.x - segment.start.x;
    let along_y = segment.end.y - segment.start.y;
    let length = along_x.hypot(along_y);
    let unit = Point::new(along_x / length, along_y / length);
    let offset_x = point.x - segment.start.x;
    let offset_y = point.y - segment.start.y;
    let along = offset_x * unit.x + offset_y * unit.y;
    let across = -offset_x * unit.y + offset_y * unit.x;

    let start_extension = if segment.start_cap == LineCap::Square {
        half_width
    } else {
        0.0
    };
    let end_extension = if segment.end_cap == LineCap::Square {
        half_width
    } else {
        0.0
    };
    let box_half_length = (length + start_extension + end_extension) * 0.5;
    let box_center = (length + end_extension - start_extension) * 0.5;
    let excess_along = (along - box_center).abs() - box_half_length;
    let excess_across = across.abs() - half_width;
    let mut distance = excess_along.max(0.0).hypot(excess_across.max(0.0)) + excess_along.max(excess_across).min(0.0);

    if segment.start_cap == LineCap::Round {
        distance = distance.min(distance_between(point, segment.start) - half_width);
    }
    if segment.end_cap == LineCap::Round {
        distance = distance.min(distance_between(point, segment.end) - half_width);
    }
    distance
}

/// Понижает поле штриха значениями одного отрезка в его окне.
fn lower_field_by_stroke_segment(
    field: &mut [f32],
    grid: GridSize,
    segment: StrokeSegment,
    half_width: f64,
    spread: f64,
) {
    let (low_x, low_y, high_x, high_y) = texel_window(grid, segment.start, segment.end, half_width + spread);
    for row in low_y..high_y {
        for column in low_x..high_x {
            let distance = stroke_segment_distance(texel_center(column, row), segment, half_width) as f32;
            let slot = &mut field[row * grid.width + column];
            if distance < *slot {
                *slot = distance;
            }
        }
    }
}
