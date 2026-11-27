//! Подписи прибора (решения 326, 327): где на картинке стоят буквы R, MG, B, CY, G, YL у целей вектороскопа.
//!
//! Положение берётся ТЕМ ЖЕ путём, что и рамки целей сетки ([`ResolvedScope::target_cell`]): цель стоит там, куда
//! правило попадания кладёт её цвет. Поэтому буква не расходится с рамкой ни на одной системе осей, ни при каком
//! охвате экрана (OKLab), масштабе из настроек и зуме ×2. Экран только рисует текст в посчитанной точке.
//!
//! # Где стоит буква
//! От САМОЙ ВНЕШНЕЙ рисуемой цели цвета (100 %, если рисуется; иначе 75 %) — по лучу от центра круга наружу на
//! `target_label_offset_cells`. Цели 100 % лежат у самого края круга, поэтому центр буквы прижимается внутрь картинки с
//! полем `target_label_margin_cells`. Цель за краем сетки (зум ×2) буквы не получает: рамки там тоже нет.
//!
//! # Чего здесь нет
//! Текста («R», «MG»): это язык интерфейса, его выбирает экран. И подписей других приборов: у waveform, яркостного
//! waveform и parade подписей нет (числа уровней экран рисует сам по `WAVEFORM_LEVEL_LINES`).

use scope_contract::{LabelPoint, ScopeLabel, ScopeMark, TargetColor};

use crate::render_settings::RenderSettings;
use crate::resolved::ResolvedScope;

/// Цели вектороскопа по именам цвета и каналам (0/1 на канал R, G, B). Соответствие задано ЯВНО по имени: порядок
/// таблицы рамок сетки (R, G, B, C, M, Y) здесь не используется.
const TARGETS: [(TargetColor, [f32; 3]); 6] = [
    (TargetColor::Red, [1.0, 0.0, 0.0]),
    (TargetColor::Magenta, [1.0, 0.0, 1.0]),
    (TargetColor::Blue, [0.0, 0.0, 1.0]),
    (TargetColor::Cyan, [0.0, 1.0, 1.0]),
    (TargetColor::Green, [0.0, 1.0, 0.0]),
    (TargetColor::Yellow, [1.0, 1.0, 0.0]),
];

/// Амплитуда целей полного размаха (100 %).
const FULL_AMPLITUDE: f32 = 1.0;

impl ResolvedScope {
    /// Подписи прибора для текущего правила и настроек показа. У приборов без подписей — пустой список.
    pub fn labels(&self, settings: &RenderSettings) -> Vec<ScopeLabel> {
        // `match` без `_`: новый прибор не соберётся, пока не скажет, есть ли у него подписи.
        match self {
            Self::Histogram(_) | Self::Waveform(_) | Self::LumaWaveform(_) | Self::RgbParade(_) => Vec::new(),
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => self.vectorscope_target_labels(settings),
            Self::Cie(rule) => crate::cie_graticule::cie_labels(rule, settings),
        }
    }

    /// Буквы целей вектороскопа (см. описание модуля).
    fn vectorscope_target_labels(&self, settings: &RenderSettings) -> Vec<ScopeLabel> {
        let Some(geometry) = self.vectorscope_geometry() else {
            return Vec::new();
        };
        let size = geometry.size() as f32;
        // Центр круга — центр ячейки `centre_cell`, как у креста сетки.
        let centre_cell = geometry.centre_cell() as f32 + 0.5;
        // Поле больше половины картинки означало бы пустой допустимый отрезок: буква встаёт в центр.
        let margin = (settings.target_label_margin_cells / size).min(0.5);
        // Размахи нарисованных целей от внешней к внутренней: буква встаёт у внешней из ВИДИМЫХ рамок. При «2×»
        // цель 100 % может уйти за край, а рамка 75 % остаться — тогда буква у рамки 75 % (решение агента 14s:
        // рамка без буквы читалась бы как неизвестный цвет).
        let drawn_amplitudes = [
            settings.vectorscope_targets.draws_full().then_some(FULL_AMPLITUDE),
            settings
                .vectorscope_targets
                .draws_bars()
                .then_some(settings.target_amplitude),
        ];
        TARGETS
            .into_iter()
            .filter_map(|(color, channels)| {
                // Все нарисованные цели цвета вне сетки: ни рамки, ни буквы.
                let hit = drawn_amplitudes
                    .into_iter()
                    .flatten()
                    .find_map(|amplitude| self.target_cell(channels, amplitude))?;
                let target = [hit.column as f32 + 0.5, hit.row as f32 + 0.5];
                let from_centre = [target[0] - centre_cell, target[1] - centre_cell];
                let distance = from_centre[0].hypot(from_centre[1]);
                // Цель точно в центре направления не имеет (для настоящих цветов не бывает): буквы нет.
                if distance <= 0.0 {
                    return None;
                }
                let scale = (distance + settings.target_label_offset_cells) / distance;
                let outward = [
                    (centre_cell + from_centre[0] * scale) / size,
                    (centre_cell + from_centre[1] * scale) / size,
                ];
                // Прижимаем центр буквы внутрь картинки: у цели 100 % край круга почти у края картинки. После
                // прижатия к [margin, 1 − margin] (margin ≤ 0,5) точка вне 0..1 невозможна; отказ `LabelPoint` значит
                // только NaN из геометрии — такую букву не рисуем (рисовать её негде), а не роняем расчёт.
                let at = LabelPoint::try_new(
                    outward[0].clamp(margin, 1.0 - margin),
                    outward[1].clamp(margin, 1.0 - margin),
                )
                .ok()?;
                Some(ScopeLabel {
                    mark: ScopeMark::VectorscopeTarget(color),
                    at,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use isp_model::image::DisplayGamut;
    use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes, VectorscopeZoom};

    use super::*;
    use crate::graticule::GraticulePrimitive;
    use crate::settings::ScopeModelSettings;

    const CENTRE: f32 = 0.5;

    fn request(kind: ScopeKind, axes: VectorscopeAxes, zoom: VectorscopeZoom) -> ScopeRequest {
        ScopeRequest {
            kind,
            vectorscope_axes: axes,
            vectorscope_zoom: zoom,
            cie_diagram: scope_contract::CieDiagram::default(),
            histogram_mode: scope_contract::HistogramMode::default(),
            scan: ScopeScan::Upright,
            image_size: ScopeImageSize::try_new(512, 512).expect("размер в допуске"),
        }
    }

    fn resolved(
        kind: ScopeKind,
        axes: VectorscopeAxes,
        zoom: VectorscopeZoom,
        gamut: DisplayGamut,
        settings: &ScopeModelSettings,
    ) -> ResolvedScope {
        ResolvedScope::resolve(&request(kind, axes, zoom), settings, gamut).expect("правило решается")
    }

    fn labels_of(
        axes: VectorscopeAxes,
        zoom: VectorscopeZoom,
        gamut: DisplayGamut,
        settings: &ScopeModelSettings,
    ) -> Vec<ScopeLabel> {
        resolved(ScopeKind::Vectorscope, axes, zoom, gamut, settings).labels(&settings.render)
    }

    fn point_of(labels: &[ScopeLabel], color: TargetColor) -> Option<[f32; 2]> {
        labels
            .iter()
            .find(|label| label.mark == ScopeMark::VectorscopeTarget(color))
            .map(|label| [label.at.x(), label.at.y()])
    }

    /// Центры рамок целей сетки с данным полуразмером (доля картинки).
    fn box_centres(rule: &ResolvedScope, settings: &RenderSettings, half_size_cells: f32) -> Vec<[f32; 2]> {
        let half_size = half_size_cells / 512.0;
        rule.graticule(settings)
            .into_iter()
            .filter_map(|primitive| match primitive {
                GraticulePrimitive::BoxOutline {
                    centre,
                    half_size: half,
                    ..
                } if (half[0] - half_size).abs() < 1e-6 => Some(centre),
                _ => None,
            })
            .collect()
    }

    fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }

    /// Угол луча из центра круга в градусах против часовой стрелки от «вправо» (ось «вверх» — строки ВНИЗ, поэтому
    /// знак `y` меняется).
    fn angle_degrees(point: [f32; 2]) -> f32 {
        (-(point[1] - CENTRE)).atan2(point[0] - CENTRE).to_degrees()
    }

    fn angle_gap(a: f32, b: f32) -> f32 {
        let gap = (a - b).rem_euclid(360.0);
        gap.min(360.0 - gap)
    }

    /// Независимый от крейта угол цели на осях Y'CbCr BT.709: Cb вправо, Cr вверх.
    fn ycbcr_angle(rgb: [f32; 3]) -> f32 {
        let luma = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        let cb = (rgb[2] - luma) / 1.8556;
        let cr = (rgb[0] - luma) / 1.5748;
        cr.atan2(cb).to_degrees()
    }

    const EVERY_AXES_AND_GAMUT: [(VectorscopeAxes, DisplayGamut); 4] = [
        (VectorscopeAxes::YCbCrBt709, DisplayGamut::Srgb),
        (VectorscopeAxes::YCbCrBt709, DisplayGamut::DisplayP3),
        (VectorscopeAxes::OklabAb, DisplayGamut::Srgb),
        (VectorscopeAxes::OklabAb, DisplayGamut::DisplayP3),
    ];

    #[test]
    fn every_target_gets_a_letter_inside_the_picture_outward_of_its_outermost_box() {
        let settings = ScopeModelSettings::default();
        for (axes, gamut) in EVERY_AXES_AND_GAMUT {
            let rule = resolved(ScopeKind::Vectorscope, axes, VectorscopeZoom::Normal, gamut, &settings);
            let labels = rule.labels(&settings.render);
            assert_eq!(labels.len(), 6, "{axes:?} {gamut:?}: у каждой цели своя буква");
            let outer_boxes = box_centres(&rule, &settings.render, settings.render.full_target_box_half_cells);
            assert_eq!(outer_boxes.len(), 6, "{axes:?} {gamut:?}: все цели 100 % в круге");
            for label in &labels {
                let at = [label.at.x(), label.at.y()];
                // Буква — центр текста: поля до края картинки не меньше заданных.
                let margin = settings.render.target_label_margin_cells / 512.0;
                assert!(
                    at.iter()
                        .all(|value| *value >= margin - 1e-5 && *value <= 1.0 - margin + 1e-5),
                    "{axes:?} {gamut:?}: {at:?} ближе поля {margin} к краю"
                );
                // Её цель — ближайшая рамка 100 %: буква дальше от центра круга, чем эта рамка.
                let nearest = outer_boxes
                    .iter()
                    .copied()
                    .min_by(|a, b| distance(*a, at).total_cmp(&distance(*b, at)))
                    .expect("рамки есть");
                assert!(
                    distance(at, [CENTRE, CENTRE]) > distance(nearest, [CENTRE, CENTRE]),
                    "{axes:?} {gamut:?}: буква {:?} {at:?} не снаружи цели {nearest:?}",
                    label.mark
                );
                // И рядом с ней: вынос плюс прижатие к краю не уносят букву дальше нескольких размеров рамки.
                assert!(
                    distance(at, nearest) < 0.06,
                    "{axes:?} {gamut:?}: буква {:?} слишком далеко от цели",
                    label.mark
                );
            }
        }
    }

    #[test]
    fn ycbcr_letters_lie_on_the_bt709_angles_of_their_own_colours_so_red_is_not_magenta() {
        let settings = ScopeModelSettings::default();
        let labels = labels_of(
            VectorscopeAxes::YCbCrBt709,
            VectorscopeZoom::Normal,
            DisplayGamut::Srgb,
            &settings,
        );
        let expected = [
            (TargetColor::Red, [1.0, 0.0, 0.0]),
            (TargetColor::Magenta, [1.0, 0.0, 1.0]),
            (TargetColor::Blue, [0.0, 0.0, 1.0]),
            (TargetColor::Cyan, [0.0, 1.0, 1.0]),
            (TargetColor::Green, [0.0, 1.0, 0.0]),
            (TargetColor::Yellow, [1.0, 1.0, 0.0]),
        ];
        for (color, rgb) in expected {
            let at = point_of(&labels, color).expect("буква есть");
            let gap = angle_gap(angle_degrees(at), ycbcr_angle(rgb));
            assert!(
                gap < 8.0,
                "{color:?}: угол буквы {} против {}",
                angle_degrees(at),
                ycbcr_angle(rgb)
            );
        }
    }

    #[test]
    fn oklab_letters_go_around_the_circle_in_hue_order_for_both_display_gamuts() {
        let settings = ScopeModelSettings::default();
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            let labels = labels_of(VectorscopeAxes::OklabAb, VectorscopeZoom::Normal, gamut, &settings);
            // Порядок оттенка OKLab: красный, жёлтый, зелёный, голубой, синий, пурпурный.
            let hue_order = [
                TargetColor::Red,
                TargetColor::Yellow,
                TargetColor::Green,
                TargetColor::Cyan,
                TargetColor::Blue,
                TargetColor::Magenta,
            ];
            let angles: Vec<f32> = hue_order
                .iter()
                .map(|color| angle_degrees(point_of(&labels, *color).expect("буква есть")))
                .collect();
            // Шаги между соседями по оттенку, взятые по кругу, одного знака и в сумме дают один оборот.
            let steps: Vec<f32> = (0..angles.len())
                .map(|index| (angles[(index + 1) % angles.len()] - angles[index]).rem_euclid(360.0))
                .collect();
            let counter_clockwise = steps.iter().all(|step| *step > 0.0 && *step < 180.0);
            let clockwise = steps.iter().all(|step| *step > 180.0);
            assert!(
                counter_clockwise || clockwise,
                "{gamut:?}: порядок букв сбит: {angles:?}"
            );
            let total: f32 = steps.iter().sum();
            assert!(
                (total - 360.0).abs() < 1e-2 || (total - 360.0 * 5.0).abs() < 1e-2,
                "{gamut:?}: за круг должен быть один оборот: {steps:?}"
            );
        }
    }

    #[test]
    fn the_oklab_letters_follow_the_display_gamut_like_the_boxes_do() {
        let settings = ScopeModelSettings::default();
        let srgb = labels_of(
            VectorscopeAxes::OklabAb,
            VectorscopeZoom::Normal,
            DisplayGamut::Srgb,
            &settings,
        );
        let p3 = labels_of(
            VectorscopeAxes::OklabAb,
            VectorscopeZoom::Normal,
            DisplayGamut::DisplayP3,
            &settings,
        );
        // Цвет «чистый красный» на экране P3 — другая точка OKLab, чем в sRGB: положение буквы обязано это показать.
        let red_srgb = point_of(&srgb, TargetColor::Red).expect("буква есть");
        let red_p3 = point_of(&p3, TargetColor::Red).expect("буква есть");
        assert!(distance(red_srgb, red_p3) > 0.005, "{red_srgb:?} против {red_p3:?}");
    }

    #[test]
    fn a_letter_follows_the_scale_settings_and_falls_back_to_the_visible_75_percent_box() {
        // Край круга по цветности 0,96: при зуме ×2 цветность на краю 0,48, и сетка (квадрат!) не вмещает красный,
        // синий, жёлтый и голубой (одна из составляющих 0,5 > 0,48), а зелёный и пурпурный (наибольшая составляющая
        // около 0,454) ещё внутри.
        let wide = ScopeModelSettings {
            ycbcr_edge_chroma: 0.96,
            ..ScopeModelSettings::default()
        };
        let rule = resolved(
            ScopeKind::Vectorscope,
            VectorscopeAxes::YCbCrBt709,
            VectorscopeZoom::Double,
            DisplayGamut::Srgb,
            &wide,
        );
        let labels = rule.labels(&wide.render);
        let full_boxes = box_centres(&rule, &wide.render, wide.render.full_target_box_half_cells);
        assert_eq!(
            full_boxes.len(),
            2,
            "в сетке остались только рамки 100 % зелёного и пурпурного"
        );
        // Рамки 75 % всех шести цветов ещё в сетке (наибольшая составляющая 0,375 < 0,48), поэтому букв шесть: у
        // зелёного и пурпурного — у рамки 100 %, у остальных — у рамки 75 % (внешней из видимых).
        assert_eq!(labels.len(), 6, "{labels:?}");
        let bar_boxes = box_centres(&rule, &wide.render, wide.render.target_box_half_cells);
        assert_eq!(bar_boxes.len(), 6);
        for fallen_back in [
            TargetColor::Red,
            TargetColor::Blue,
            TargetColor::Yellow,
            TargetColor::Cyan,
        ] {
            let at = point_of(&labels, fallen_back).expect("буква у рамки 75 %");
            let nearest_bar = bar_boxes
                .iter()
                .copied()
                .min_by(|a, b| distance(*a, at).total_cmp(&distance(*b, at)))
                .expect("рамки есть");
            let offset_cells = distance(at, nearest_bar) * 512.0;
            assert!(
                offset_cells <= wide.render.target_label_offset_cells + 1.0,
                "{fallen_back:?}: буква в {offset_cells} клетках от рамки 75 %, а не у ушедшей за край 100 %"
            );
        }
        for kept_full in [TargetColor::Green, TargetColor::Magenta] {
            let at = point_of(&labels, kept_full).expect("буква у рамки 100 %");
            let nearest_full = full_boxes
                .iter()
                .copied()
                .min_by(|a, b| distance(*a, at).total_cmp(&distance(*b, at)))
                .expect("рамки есть");
            assert!(distance(at, nearest_full) * 512.0 <= wide.render.target_label_offset_cells + 1.0);
        }
        // Обычный масштаб при тех же настройках: все шесть, и каждая буква ближе к центру, чем при зуме ×2.
        let normal = labels_of(
            VectorscopeAxes::YCbCrBt709,
            VectorscopeZoom::Normal,
            DisplayGamut::Srgb,
            &wide,
        );
        assert_eq!(normal.len(), 6);
        for label in &labels {
            let (ScopeMark::VectorscopeTarget(color), zoomed) = (label.mark, [label.at.x(), label.at.y()]) else {
                panic!("у вектороскопа только буквы целей");
            };
            let plain = point_of(&normal, color).expect("при обычном масштабе буква есть");
            assert!(
                distance(zoomed, [CENTRE, CENTRE]) > distance(plain, [CENTRE, CENTRE]),
                "{color:?}: зум ×2 уводит цель от центра"
            );
        }
    }

    #[test]
    fn default_scale_at_double_zoom_leaves_every_target_outside_so_no_letters() {
        let settings = ScopeModelSettings::default();
        let labels = labels_of(
            VectorscopeAxes::YCbCrBt709,
            VectorscopeZoom::Double,
            DisplayGamut::Srgb,
            &settings,
        );
        assert!(labels.is_empty(), "{labels:?}");
    }

    #[test]
    fn only_75_percent_targets_put_the_letter_outward_of_the_75_percent_box() {
        let settings = ScopeModelSettings {
            render: RenderSettings {
                vectorscope_targets: crate::render_settings::VectorscopeTargets::Bars75,
                ..RenderSettings::default()
            },
            ..ScopeModelSettings::default()
        };
        let rule = resolved(
            ScopeKind::Vectorscope,
            VectorscopeAxes::YCbCrBt709,
            VectorscopeZoom::Normal,
            DisplayGamut::Srgb,
            &settings,
        );
        let labels = rule.labels(&settings.render);
        let bar_boxes = box_centres(&rule, &settings.render, settings.render.target_box_half_cells);
        assert_eq!(bar_boxes.len(), 6);
        for label in &labels {
            let at = [label.at.x(), label.at.y()];
            let nearest = bar_boxes
                .iter()
                .copied()
                .min_by(|a, b| distance(*a, at).total_cmp(&distance(*b, at)))
                .expect("рамки есть");
            let offset = distance(at, nearest) * 512.0;
            // 75 % цели далеко от края картинки: прижатия нет, вынос ровно заданный.
            assert!(
                (offset - settings.render.target_label_offset_cells).abs() < 1.0,
                "{:?}: вынос {offset} клеток",
                label.mark
            );
        }
    }

    #[test]
    fn instruments_without_target_marks_have_no_labels() {
        let settings = ScopeModelSettings::default();
        for kind in [ScopeKind::Waveform, ScopeKind::LumaWaveform, ScopeKind::RgbParade] {
            let rule = resolved(
                kind,
                VectorscopeAxes::YCbCrBt709,
                VectorscopeZoom::Normal,
                DisplayGamut::Srgb,
                &settings,
            );
            assert!(rule.labels(&settings.render).is_empty(), "{kind:?}");
        }
    }
}
