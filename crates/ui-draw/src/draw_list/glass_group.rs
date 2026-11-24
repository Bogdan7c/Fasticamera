//! Группа стекла: несколько форм, сливающихся в одно стекло.

use crate::color::Opacity;
use crate::draw_list::bounds::{linear_stretch, transformed_bounds};
use crate::geometry::{PxPoint, PxRect, PxVector};
use crate::glass::GlassParamsPx;
use crate::glass::reference::PressInput;
use crate::shape::{ShapeGeometry, corrected_distance, smooth_union_with_weights};
use crate::transform::Affine2;

/// Предел числа форм в одной группе — размер массива в шейдере.
///
/// Инженерная константа: массив членов лежит в uniform-буфере (около 100 байт на член: размер,
/// радиус, показатель, обратная матрица, нажатие), 16 членов = 1,6 КиБ, с большим запасом в пределах
/// гарантированного размера uniform-буфера Vulkan (16 КиБ). Хватает на самую большую связную группу
/// экрана камеры (нижний ряд кнопок плюс переключатель режимов). Больше — ошибка валидации,
/// а не молчаливая обрезка. Подгонка, не Apple.
pub const MAX_GLASS_GROUP_MEMBERS: usize = 16;

/// Нажатие одного члена группы.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressState {
    /// 0 — не нажат, 1 — нажат полностью.
    pub amount: f32,
    /// Точка касания в локальных координатах члена (центр = 0).
    pub touch_point_local: PxPoint,
}

impl PressState {
    /// Не нажат.
    pub const RELEASED: PressState = PressState {
        amount: 0.0,
        touch_point_local: PxPoint::ORIGIN,
    };
}

/// Одна форма группы.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassMember {
    pub geometry: ShapeGeometry,
    /// Локальные координаты (центр = 0) → экран. Уже включает масштаб нажатия
    /// ([`crate::glass::PressParamsPx::scale_for`]): рендер его не добавляет.
    pub transform: Affine2,
    pub press: PressState,
}

impl GlassMember {
    /// Точка касания члена в экранных пикселях.
    pub fn touch_point_px(&self) -> PxPoint {
        self.transform.apply_point(self.press.touch_point_local)
    }

    /// Экранный охватывающий прямоугольник области, где расстояние поля формы члена меньше `reach_px` (экранных
    /// px) от границы. Подробности оценки — в [`GlassGroupItem::draw_rect_px`]. `None` — преобразование не
    /// конечно или вырождено.
    pub(crate) fn reach_bounds_px(&self, reach_px: f32) -> Option<PxRect> {
        let stretch_max = linear_stretch(&self.transform);
        let stretch_min = self.transform.determinant().abs() / stretch_max;
        if !(stretch_min.is_finite() && stretch_min > 0.0) {
            return None;
        }
        let local = PxRect::from_center(PxPoint::ORIGIN, self.geometry.extent()).expanded(reach_px / stretch_min);
        transformed_bounds(&self.transform, &local)
    }

    /// Границы формы на экране (без запасов).
    pub fn shape_bounds_px(&self) -> Option<PxRect> {
        let local = PxRect::from_center(PxPoint::ORIGIN, self.geometry.extent());
        transformed_bounds(&self.transform, &local)
    }
}

/// Запас квада рисования на сглаживание края, px: покрытие слоя ненулевое, пока расстояние меньше 0,5 px
/// (`coverage_ref`, ширина сглаживания 1 px); берём 1 px, чтобы центр пикселя на границе не выпал из квада.
pub const DRAW_RECT_EDGE_MARGIN_PX: f32 = 1.0;

/// Группа стекла: одним проходом над одним фоном. Слияние членов — `smooth_union` радиусом
/// `merge_radius_px` (у Apple — `spacing` контейнера, `glass.md` раздел 2).
#[derive(Debug, Clone, PartialEq)]
pub struct GlassGroupItem {
    pub params: GlassParamsPx,
    pub members: Vec<GlassMember>,
    /// Радиус слияния, px; 0 — формы не сливаются.
    pub merge_radius_px: f32,
    /// Материализация 0..1 (Apple 219, 2:55): сила линзирования, НЕ прозрачность.
    pub effect: f32,
    pub opacity: Opacity,
}

/// Поле слитой формы в точке экрана.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupField {
    /// Исправленное расстояние до слитой формы, px (внутри < 0).
    pub distance_px: f32,
    /// Единичная нормаль наружу (нулевая, если направления нет).
    pub normal_out: PxVector,
}

impl GlassGroupItem {
    /// Прямоугольник адаптации яркости: от него шейдер и эталон берут 5 точек «средней яркости фона под
    /// группой» (центр и четверти сторон, `mean_luma_under`). Это объединённые границы форм членов БЕЗ запасов
    /// (решение 220, сессия 16a): яркость меряется под самой панелью.
    ///
    /// Прежде здесь лежали границы, раздутые на запас под слияние, тень, вынос линзы и свечение нажатия
    /// ([`GlassGroupItem::influence_rect_px`]). У небольшой панели запас был больше самой панели, и 4 из 5 точек
    /// замера оказывались ВНЕ неё: измерялся фон вокруг стекла, а не под ним. Вид стекла от этой правки
    /// осознанно меняется (средняя яркость сдвигает затемнение).
    /// Он НЕ определяет, где группа рисуется ([`GlassGroupItem::draw_rect_px`]) и какие границы у элемента
    /// ([`GlassGroupItem::bounds_px`]).
    /// `None` — группа пуста или преобразование не конечно.
    pub fn adaptation_rect_px(&self) -> Option<PxRect> {
        self.members_bounds_px()
    }

    /// Консервативные границы «всё, что может зависеть от группы»: формы членов, раздутые на запас под
    /// слияние, тень, вынос линзы и свечение нажатия. Это прежнее определение границ группы; от него зависят
    /// вершины квада ([`GlassGroupItem::bounds_px`]) и потребители границ элемента. `None` — как у
    /// [`GlassGroupItem::adaptation_rect_px`].
    pub fn influence_rect_px(&self) -> Option<PxRect> {
        let bounds = self.members_bounds_px()?;
        let params = &self.params;
        let margin = self
            .merge_radius_px
            .max(params.shadow.radius_px + params.shadow.offset_px)
            .max(params.refraction_strength_px * (1.0 + params.press.refraction_boost))
            .max(params.press.glow_radius_px);
        Some(bounds.expanded(margin))
    }

    /// Общее имя границ элемента ([`crate::draw_list::DrawItem::bounds_px`]): для стекла это консервативные
    /// границы [`GlassGroupItem::influence_rect_px`] (по ним строится и квад вершин). Прямоугольник адаптации
    /// яркости с решения 220 от них отделён.
    pub fn bounds_px(&self) -> Option<PxRect> {
        self.influence_rect_px()
    }

    /// Квад рисования: область, вне которой группа точно даёт нулевой вклад в кадр (premultiplied 0).
    ///
    /// Состав: слой стекла (покрытие ненулевое, пока расстояние поля меньше 0,5 px) и тень (форма, сдвинутая
    /// вниз на `offset`, пока расстояние меньше радиуса; только если у тени есть альфа), обе — с запасом
    /// `merge_radius / 4` на выпуклость плавного слияния и запасом на сглаживание [`DRAW_RECT_EDGE_MARGIN_PX`].
    /// НЕ входят: свечение нажатия (оно смешивается с белым внутри слоя стекла, снаружи слоя ничего не
    /// рисует) и вынос линзы (он меняет, ОТКУДА читается фон, а не где рисуется).
    ///
    /// Расстояние поля измеряется в экранных px, но считается в локальных координатах члена и делится на
    /// длину экранного градиента нормали, а она при растяжении преобразования доходит до `1 / σmin`
    /// (σmin — наименьшее сингулярное число линейной части). Поэтому досягаемость `reach` px экрана в
    /// локальных координатах — `reach / σmin` (строгая верхняя оценка; при равномерном масштабе — точная).
    /// `None` — группа пуста, преобразование не конечно или вырождено.
    pub fn draw_rect_px(&self) -> Option<PxRect> {
        let merge_bulge = self.merge_radius_px * 0.25;
        let layer_reach = merge_bulge + DRAW_RECT_EDGE_MARGIN_PX;
        let shadow = &self.params.shadow;
        let mut area: Option<PxRect> = None;
        let mut add = |rect: PxRect| {
            area = Some(match area {
                Some(acc) => acc.union(&rect),
                None => rect,
            });
        };
        for member in &self.members {
            add(member.reach_bounds_px(layer_reach)?);
            if shadow.alpha > 0.0 {
                let shadow_reach = shadow.radius_px + merge_bulge + DRAW_RECT_EDGE_MARGIN_PX;
                let mut shifted = member.reach_bounds_px(shadow_reach)?;
                // Тень сдвинута вниз по ЭКРАНУ (шейдер вычитает смещение из экранной y), а не по осям члена.
                shifted.origin.y += shadow.offset_px;
                add(shifted);
            }
        }
        area
    }

    /// Охватывающий прямоугольник форм всех членов (без запасов).
    fn members_bounds_px(&self) -> Option<PxRect> {
        let mut bounds: Option<PxRect> = None;
        for member in &self.members {
            let member_bounds = member.shape_bounds_px()?;
            bounds = Some(match bounds {
                Some(acc) => acc.union(&member_bounds),
                None => member_bounds,
            });
        }
        bounds
    }

    /// Нажатие группы для пикселя: у `shade_glass_pixel` одно нажатие на пиксель, а у членов оно
    /// своё. Берётся член с НАИБОЛЬШЕЙ силой нажатия (при равенстве — первый в списке): его сила и
    /// точка касания (экранные px) управляют усилением преломления и свечением всей группы.
    /// Свечение растекается на соседние формы, потому что точка касания и пиксель живут в одной
    /// экранной системе (Apple 219, 12:05). Нет членов — [`PressInput::NONE`]. Этим правилом
    /// пользуется и рендер при упаковке группы.
    pub fn dominant_press(&self) -> PressInput {
        let mut dominant: Option<&GlassMember> = None;
        for member in &self.members {
            // Строго «больше»: при равенстве остаётся первый.
            if dominant.is_none_or(|current| member.press.amount > current.press.amount) {
                dominant = Some(member);
            }
        }
        dominant.map_or(PressInput::NONE, |member| PressInput {
            amount: member.press.amount,
            touch_point_px: member.touch_point_px(),
        })
    }

    /// Эталонное поле слитой формы в экранной точке: расстояния членов (исправленные, в px)
    /// сливаются `smooth_union`, нормали — теми же весами. `Ok(None)` — членов нет;
    /// ошибка — вырожденное преобразование члена.
    pub fn field_at(&self, screen_point: PxPoint) -> Result<Option<GroupField>, crate::transform::AffineError> {
        let mut folded: Option<(f32, PxVector)> = None;
        for member in &self.members {
            let (distance, normal) = member_field(member, screen_point)?;
            folded = Some(match folded {
                None => (distance, normal),
                Some((acc_distance, acc_normal)) => {
                    let merged = smooth_union_with_weights(acc_distance, distance, self.merge_radius_px);
                    // Градиент слитого поля — взвешенная сумма градиентов (аналитическая производная smooth-min).
                    let blended = acc_normal * merged.weight_first + normal * merged.weight_second;
                    (merged.distance, blended)
                }
            });
        }
        Ok(folded.map(|(distance, blended)| GroupField {
            distance_px: distance,
            normal_out: blended.normalized().unwrap_or(PxVector::ZERO),
        }))
    }
}

/// Исправленное расстояние (px) и единичная нормаль члена в экранной точке.
fn member_field(member: &GlassMember, screen_point: PxPoint) -> Result<(f32, PxVector), crate::transform::AffineError> {
    let to_local = member.transform.inverse()?;
    let local_point = to_local.apply_point(screen_point);
    let sample = corrected_distance(&member.geometry, local_point);
    // Градиент в экранных координатах: (L⁻¹)ᵀ·∇_локальный; его длина — 1/масштаб. Деление на неё
    // переводит расстояние из локальных единиц в пиксели экрана (первый порядок).
    let local_gradient = sample.normal_out;
    let screen_gradient = to_local.apply_gradient(local_gradient);
    match screen_gradient.normalized() {
        Some(normal) => Ok((sample.distance / screen_gradient.length(), normal)),
        None => Ok((sample.distance, PxVector::ZERO)),
    }
}
