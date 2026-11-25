//! Основа тона ACES: RRT и выходное преобразование SDR 100 нит из Academy aces-dev (решения 176, 177, 182).
//!
//! # Источник и лицензия
//! Взято из репозитория Academy Software Foundation / A.M.P.A.S. `ampas/aces-dev`, версия 1.3:
//! <https://github.com/ampas/aces-dev> (файлы `transforms/ctl/rrt/RRT.ctl`, `transforms/ctl/lib/ACESlib.RRT_Common.ctl`,
//! `ACESlib.Tonescales.ctl`, `ACESlib.Utilities_Color.ctl`, `ACESlib.ODT_Common.ctl`, `ACESlib.Transform_Common.ctl`,
//! `transforms/ctl/odt/rec2020/ODT.Academy.Rec2020_100nits_dim.ctl`). Описание системы: <https://docs.acescentral.com>.
//!
//! **Что взято:** структура преобразования (свечение, красный модификатор, глобальная десатурация, тональная кривая
//! по каналам в AP1, нормировка в `linCV`, поправка тусклого окружения, десатурация выхода); числовые константы
//! (`RRT_GLOW_*`, `RRT_RED_*`, `RRT_SAT_FACTOR`, `ODT_SAT_FACTOR`, `DIM_SURROUND_GAMMA`, `CINEMA_WHITE/BLACK`);
//! координаты цветности AP0 и AP1; **коэффициенты сегментированных сплайнов** C5 (`RRT_PARAMS`) и C9 (`ODT_48nits`).
//! Код переписан на Rust заново, без LUT; чужой код не копировался.
//!
//! **Лицензия** — файл `LICENSE.md` репозитория (проверено по тексту, 2026-11-23). Ключевая фраза: «A worldwide,
//! royalty-free, non-exclusive right to copy, modify, create derivatives, and use, in source and binary forms, is
//! hereby granted, subject to acceptance of this license». Использование разрешено при условиях ниже; текст
//! сохранён дословно, как требует условие «Copies of source code, in whole or in part, must retain the above
//! copyright notice, this list of conditions and the Disclaimer of Warranty».
//!
//! > License Terms for Academy Color Encoding System Components
//! >
//! > Academy Color Encoding System (ACES) software and tools are provided by the Academy under the following terms
//! > and conditions: A worldwide, royalty-free, non-exclusive right to copy, modify, create derivatives, and use, in
//! > source and binary forms, is hereby granted, subject to acceptance of this license.
//! >
//! > Copyright © 2015 Academy of Motion Picture Arts and Sciences (A.M.P.A.S.). Portions contributed by others as
//! > indicated. All rights reserved.
//! >
//! > Performance of any of the aforementioned acts indicates acceptance to be bound by the following terms and
//! > conditions:
//! >
//! > * Copies of source code, in whole or in part, must retain the above copyright notice, this list of conditions
//! >   and the Disclaimer of Warranty.
//! > * Use in binary form must retain the above copyright notice, this list of conditions and the Disclaimer of
//! >   Warranty in the documentation and/or other materials provided with the distribution.
//! > * Nothing in this license shall be deemed to grant any rights to trademarks, copyrights, patents, trade secrets
//! >   or any other intellectual property of A.M.P.A.S. or any contributors, except as expressly stated herein.
//! > * Neither the name "A.M.P.A.S." nor the name of any other contributors to this software may be used to endorse or
//! >   promote products derivative of or based on this software without express prior written permission of
//! >   A.M.P.A.S. or the contributors, as appropriate.
//! >
//! > This license shall be construed pursuant to the laws of the State of California, and any disputes related
//! > thereto shall be subject to the jurisdiction of the courts therein.
//! >
//! > Disclaimer of Warranty: THIS SOFTWARE IS PROVIDED BY A.M.P.A.S. AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR
//! > IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
//! > PARTICULAR PURPOSE, AND NON-INFRINGEMENT ARE DISCLAIMED. IN NO EVENT SHALL A.M.P.A.S., OR ANY CONTRIBUTORS OR
//! > DISTRIBUTORS, BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, RESITUTIONARY, OR
//! > CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
//! > DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
//! > STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
//! > SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//! >
//! > WITHOUT LIMITING THE GENERALITY OF THE FOREGOING, THE ACADEMY SPECIFICALLY DISCLAIMS ANY REPRESENTATIONS OR
//! > WARRANTIES WHATSOEVER RELATED TO PATENT OR OTHER INTELLECTUAL PROPERTY RIGHTS IN THE ACADEMY COLOR ENCODING
//! > SYSTEM, OR APPLICATIONS THEREOF, HELD BY PARTIES OTHER THAN A.M.P.A.S., WHETHER DISCLOSED OR UNDISCLOSED.
//!
//! Условия, которые остаются на владельце (не в этом crate): второе условие (бинарный вид) требует приложить
//! уведомление об авторстве и отказ от гарантий к документации или материалам поставки приложения; четвёртое запрещает
//! использовать имя «A.M.P.A.S.» для продвижения продукта. Слово «ACES» здесь — название модели в настройках.
//!
//! # Какой вариант ACES и почему
//! Выбран ACES 1.x: RRT (v1.0.3, `ACES 1.0 - RRT`) плюс выходное преобразование `Rec.2020 100 нит, тусклое окружение`.
//! Он целиком аналитический: два сегментированных сплайна (квадратичные B-сплайны в `log10`), свечение,
//! красный модификатор, три матрицы, одна степень. Ни таблиц, ни итераций на пиксель. Отвергнуто:
//! * **ACES 2.0 Output Transform** — модель цветового восприятия (CAM16-подобная), таблицы кромки охвата
//!   и итеративный поиск границы охвата: ближе всего к LUT и дороже всего на шейдере;
//! * **«Single Stage Tone Scale»** (ACES 1.2, HDR/SDR-ветка): та же идея сплайна, но без свечения и красного
//!   модификатора, определяющих «вид ACES» на лицах и закатах; взят полный RRT + ODT как эталон узнаваемой картинки.
//!
//! # Цепочка пикселя (рабочее пространство W = линейный Rec. 2020, D65; решение 176)
//! ```text
//! w    = вход(c)                              — обрезка нулём или сжатие охвата W (общий шаг [`WorkingSpaceIo`])
//! a    = clamp(w · 0,18/grey_eff, 0, 65504)   — серый сцены → серая карта ACES 0,18 (grey_eff = scene_middle_gray / k)
//! ap0  = M_{W→AP0} · a                        — Брэдфорд D65 → белая ACES (D60) и основные цвета AP0 (ACES2065-1)
//! ap0' = свечение и красный модификатор (ap0) — RRT: [`sweeteners`]; зажим 0…65504
//! ap1  = M_{AP0→AP1} · ap0', зажим 0…65504    — рабочее пространство рендеринга ACEScg
//! ap1  = s_RRT · ap1 + (1 − s_RRT)·Y(ap1)     — глобальная десатурация RRT (Y — яркость AP1)
//! cv_c = linear_cv(ap1_c; E_L)                — по каналам: тональная кривая, см. [`tonescale`]
//! cv   = cv · Y(cv)^(γ_s − 1)                 — поправка окружения (`darkSurround_to_dimSurround`, xyY: Y → Y^γ_s)
//! out  = M_{AP1→W} · cv                       — десатурация выхода, AP1 → XYZ, Брэдфорд D60 → D65, XYZ → Rec. 2020
//! ```
//! затем общий шаг [`AcesBase::to_output`] (`W → цвета выхода`, мягкое сжатие охвата, обрезка 0…1 — общий для всех моделей [`WorkingSpaceIo`]).
//!
//! # Где и почему это отступает от «чистого» ACES
//! * **Пара матриц AP1 → AP0 (конец RRT) и AP0 → AP1 (начало ODT)** опущена: она тождественна, а между ними нет
//!   нелинейных шагов.
//! * **Серый**: чистый ACES переводит 0,18 в ≈ 0,10 на экране; мы задаём серому экрана `display_middle_gray`
//!   (по умолчанию 0,20, как у AgX), поэтому к кривой применён сдвиг экспозиции `δ` (≈ +0,67 ступени), подобранный
//!   по обратным сплайнам. Свечение и красный модификатор сдвига не видят: они работают в единицах ACES, где серая
//!   карта = 0,18, ровно как задумано в RRT; сдвиг — часть тональной кривой.
//! * **Свой белый точки (решение 177)**: у ACES белый — `0,18·2^6,5 ≈ 16,3` (в нашей шкале это ≈ 5,8 ступеней над
//!   серым), у нас сенсор насыщается на `L ≥ 1` (≈ 3,7 ступени над серым при `L = 1`). Выше колена (по умолчанию
//!   +1,5 ступени) кривая ACES заменена кубикой (C1), приходящей ровно в 1 в `L` с нулевым наклоном; ниже колена кривая —
//!   ACES бит в бит, серый от `L` не зависит. Это нормировка выхода, не часть ACES.
//! * **Окружение**: сохранена поправка тусклого окружения ACES `Y^0,9811` и десатурация выхода 0,93: это штатные
//!   части «100 nits dim». Для яркого экрана на улице они приблизительны; числа вынесены в настройки (`surround_gamma`,
//!   `odt_saturation`), 1 выключает.
//! * **Выход без обрезки Rec. 2020**: ODT обрезает цвета в 0…1 основных цветов экрана; здесь результат остаётся в W без
//!   обрезки (сжатие охвата и обрезка — общий шаг выхода, где экран — sRGB или Display P3). `atan2` красного
//!   модификатора заменён многочленом ради паритета GPU (см. [`sweeteners`]).
//! * **Тени и света, контраст** — ручки слоя вида (решение 182): ACES не имеет таких параметров, они добавлены нами как
//!   сдвиги оси экспозиции вокруг серого (см. [`tonescale`]); с нулями кривая — чистый ACES.
//!
//! # Как подключить (для следующего этапа)
//! `AcesBase::resolve(&settings.with_exposure_gain(k), gamut)` → [`AcesBase`]; на пиксель — `xyz_d50_to_working`,
//! `map_scene_pixel(scene_w, white)`, `to_output`. Для шейдера — [`AcesBase::parameters`]: порядок формул — как в
//! цепочке выше, все числа уже в f32.

use std::sync::Arc;

#[path = "aces_settings.rs"]
pub mod settings;
#[path = "aces_spline.rs"]
pub mod spline;
#[path = "aces_sweeteners.rs"]
pub mod sweeteners;
#[path = "aces_tonescale.rs"]
pub mod tonescale;

use super::working_io::WorkingSpaceIo;
use crate::color::chromaticity::{Chromaticity, Primaries, bradford_adaptation};
use crate::color::matrix::{Matrix3, PixelMatrix};
use crate::color::working_space::WORKING_PRIMARIES;
use crate::gamut::GamutCompressionParameters;
use crate::image::DisplayGamut;
pub use settings::{AcesSettings, InvalidAces};
use spline::SplineSpec;
use sweeteners::{SweetenerParameters, apply_sweeteners};
use tonescale::{REFERENCE_GREY, ToneScaleParameters};

/// Белая точка ACES (около D60), `ACESlib.Utilities_Color.ctl`.
const ACES_WHITE: Chromaticity = Chromaticity { x: 0.32168, y: 0.33767 };

/// ACES2065-1: основные цвета AP0 (SMPTE ST 2065-1).
pub const AP0: Primaries = Primaries {
    red: Chromaticity { x: 0.7347, y: 0.2653 },
    green: Chromaticity { x: 0.0, y: 1.0 },
    blue: Chromaticity { x: 0.0001, y: -0.077 },
    white: ACES_WHITE,
};

/// Пространство рендеринга ACES 1.0: основные цвета AP1.
pub const AP1: Primaries = Primaries {
    red: Chromaticity { x: 0.713, y: 0.293 },
    green: Chromaticity { x: 0.165, y: 0.83 },
    blue: Chromaticity { x: 0.128, y: 0.044 },
    white: ACES_WHITE,
};

/// Наибольшее конечное значение half-float: верхний зажим цвета в RRT (`HALF_MAX`).
const HALF_MAX: f32 = 65504.0;

/// Яркость AP1 не выше этой — чёрный: поправка окружения `Y^(γ_s − 1)` не применяется (степень от нуля с
/// отрицательным показателем бесконечна, а цвет и так нулевой). Много меньше любой видимой яркости.
const SURROUND_BLACK_LUMINANCE: f32 = 6.0e-11;

/// Матрицы цепочки, посчитанные в f64 и один раз сужаемые до f32.
struct AcesMatrices {
    working_to_ap0: Matrix3,
    ap0_to_ap1: Matrix3,
    ap1_to_working: Matrix3,
    ap1_luminance_row: [f64; 3],
}

impl AcesMatrices {
    fn resolve(settings: &AcesSettings) -> Result<Self, crate::color::matrix::SingularMatrix> {
        let working_to_xyz = WORKING_PRIMARIES.rgb_to_xyz()?;
        let ap0_to_xyz = AP0.rgb_to_xyz()?;
        let ap1_to_xyz = AP1.rgb_to_xyz()?;
        let to_aces_white = bradford_adaptation(WORKING_PRIMARIES.white, ACES_WHITE)?;
        let from_aces_white = bradford_adaptation(ACES_WHITE, WORKING_PRIMARIES.white)?;
        let ap1_luminance_row = ap1_to_xyz.rows[1];
        // Десатурация выхода: `s·v + (1 − s)·Y(v)` — матрица `s·I + (1 − s)·1·Yᵀ` (`ODT_SAT_MAT`).
        let saturation = f64::from(settings.odt_saturation);
        let mut desaturation = [[0.0; 3]; 3];
        for (row, output) in desaturation.iter_mut().enumerate() {
            for (column, element) in output.iter_mut().enumerate() {
                let identity = if row == column { saturation } else { 0.0 };
                *element = identity + (1.0 - saturation) * ap1_luminance_row[column];
            }
        }
        Ok(Self {
            working_to_ap0: ap0_to_xyz.inverse()?.mul(&to_aces_white).mul(&working_to_xyz),
            ap0_to_ap1: ap1_to_xyz.inverse()?.mul(&ap0_to_xyz),
            ap1_to_working: working_to_xyz
                .inverse()?
                .mul(&from_aces_white)
                .mul(&ap1_to_xyz)
                .mul(&Matrix3::from_rows(desaturation)),
            ap1_luminance_row,
        })
    }
}

/// Числа основы ACES для шейдера: ровно те f32, что подставляет [`AcesBase::map_scene_pixel`].
/// Только чтение: собрать [`AcesBase`] из них нельзя (единственный вход — [`AcesBase::resolve`]).
///
/// Матрицы — по строкам, порядок применения — в доке модуля. `ap1_to_working` уже включает десатурацию выхода.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct AcesParameters {
    pub xyz_d50_to_working: [[f32; 3]; 3],
    pub working_to_output: [[f32; 3]; 3],
    pub working_to_ap0: [[f32; 3]; 3],
    pub ap0_to_ap1: [[f32; 3]; 3],
    pub ap1_to_working: [[f32; 3]; 3],
    /// Строка яркости AP1 (`Y = ap1 · row`).
    pub ap1_luminance: [f32; 3],
    /// `0,18 / grey_eff`: множитель входа.
    pub exposure_scale: f32,
    pub sweeteners: SweetenerParameters,
    /// Глобальная десатурация RRT.
    pub rrt_saturation: f32,
    /// `γ_s − 1`: показатель поправки окружения `Y^(γ_s − 1)`.
    pub surround_exponent: f32,
    pub tone: ToneScaleParameters,
    /// Сжатие охвата на входе: `None` — обрезка нулём.
    pub input_compression: Option<GamutCompressionParameters>,
    pub output_compression: GamutCompressionParameters,
}

/// Разрешённая основа ACES на кадр.
#[derive(Debug, Clone, PartialEq)]
pub struct AcesBase {
    /// Общий шаг рабочего пространства: вход в W, `XYZ D50 → W`, `W → выход`, сжатие охвата выхода.
    io: WorkingSpaceIo,
    working_to_ap0: PixelMatrix,
    ap0_to_ap1: PixelMatrix,
    ap1_to_working: PixelMatrix,
    ap1_luminance: [f32; 3],
    exposure_scale: f32,
    sweeteners: SweetenerParameters,
    rrt_saturation: f32,
    surround_exponent: f32,
    tone: ToneScaleParameters,
}

impl AcesBase {
    /// Разрешает настройки (уже с подтяжкой AE, см. [`AcesSettings::with_exposure_gain`]) для гаммы выхода.
    pub fn resolve(settings: &AcesSettings, output_gamut: DisplayGamut) -> Result<Arc<Self>, InvalidAces> {
        let tone = ToneScaleParameters::resolve(settings)?;
        let matrices = AcesMatrices::resolve(settings)?;
        let io = WorkingSpaceIo::resolve(&settings.working_io(), output_gamut)?;
        Ok(Arc::new(Self {
            io,
            working_to_ap0: matrices.working_to_ap0.to_f32(),
            ap0_to_ap1: matrices.ap0_to_ap1.to_f32(),
            ap1_to_working: matrices.ap1_to_working.to_f32(),
            ap1_luminance: matrices.ap1_luminance_row.map(|weight| weight as f32),
            exposure_scale: REFERENCE_GREY / settings.scene_middle_gray,
            sweeteners: SweetenerParameters {
                glow_gain: settings.glow_gain,
                red_modifier_scale: settings.red_modifier_scale,
            },
            rrt_saturation: settings.rrt_saturation,
            surround_exponent: settings.surround_gamma - 1.0,
            tone,
        }))
    }

    /// Гамма выхода, под которую посчитана матрица `W → выход`.
    pub fn output_gamut(&self) -> DisplayGamut {
        self.io.output_gamut()
    }

    /// Числа формулы для GPU (см. [`AcesParameters`]).
    pub fn parameters(&self) -> AcesParameters {
        let io = self.io.parameters();
        AcesParameters {
            xyz_d50_to_working: io.xyz_d50_to_working,
            working_to_output: io.working_to_output,
            working_to_ap0: self.working_to_ap0.rows,
            ap0_to_ap1: self.ap0_to_ap1.rows,
            ap1_to_working: self.ap1_to_working.rows,
            ap1_luminance: self.ap1_luminance,
            exposure_scale: self.exposure_scale,
            sweeteners: self.sweeteners,
            rrt_saturation: self.rrt_saturation,
            surround_exponent: self.surround_exponent,
            tone: self.tone,
            input_compression: io.input_compression,
            output_compression: io.output_compression,
        }
    }

    /// Разрешённая тональная кривая (для тестов и измерений).
    pub fn tone_scale(&self) -> &ToneScaleParameters {
        &self.tone
    }

    /// `XYZ D50 → W` одного пикселя.
    pub fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
        self.io.xyz_d50_to_working(xyz_d50)
    }

    /// Вход в W: обрезка нулём или мягкое сжатие охвата (общий шаг).
    pub fn apply_input(&self, scene: [f32; 3]) -> [f32; 3] {
        self.io.apply_input(scene)
    }

    /// Последний шаг: `W → цвета выхода`, мягкое сжатие охвата выхода и обрезка 0…1 (общий шаг).
    pub fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
        self.io.to_output(display_working)
    }

    /// Основа ACES для пикселя сцены в W со своим белым `white` → линейный свет экрана в W (формула модуля).
    /// NaN в любом канале — чёрный пиксель.
    pub fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
        if scene.iter().any(|channel| channel.is_nan()) {
            return [0.0; 3];
        }
        let entering = self.apply_input(scene);
        let aces = entering.map(|channel| (channel * self.exposure_scale).clamp(0.0, HALF_MAX));
        let ap0 = self.working_to_ap0.apply(aces);
        let sweetened = apply_sweeteners(&self.sweeteners, ap0);
        // RRT: зажим, чтобы насыщенные отрицательные не стали положительными после матрицы.
        let ap1 = self
            .ap0_to_ap1
            .apply(sweetened.map(|channel| channel.clamp(0.0, HALF_MAX)))
            .map(|channel| channel.clamp(0.0, HALF_MAX));
        let desaturated = self.desaturate_rrt(ap1);
        let white_ev = self.tone.white_ev(white);
        let linear_cv = desaturated.map(|channel| self.tone.linear_cv(channel, white_ev));
        self.ap1_to_working.apply(self.apply_surround(linear_cv))
    }

    /// Глобальная десатурация RRT: `s·v + (1 − s)·Y(v)` по каналам.
    fn desaturate_rrt(&self, ap1: [f32; 3]) -> [f32; 3] {
        let luminance = self.luminance_of(ap1);
        ap1.map(|channel| self.rrt_saturation * channel + (1.0 - self.rrt_saturation) * luminance)
    }

    /// Поправка тусклого окружения: Y → Y^γ_s при неизменной цветности, то есть `cv · Y^(γ_s − 1)`.
    /// Чёрный (`Y ≤ 0`) не трогаем: степень от нуля с отрицательным показателем бесконечна.
    fn apply_surround(&self, linear_cv: [f32; 3]) -> [f32; 3] {
        let luminance = self.luminance_of(linear_cv);
        if luminance <= SURROUND_BLACK_LUMINANCE {
            return linear_cv;
        }
        let factor = luminance.powf(self.surround_exponent);
        linear_cv.map(|channel| channel * factor)
    }

    fn luminance_of(&self, ap1: [f32; 3]) -> f32 {
        self.ap1_luminance[0] * ap1[0] + self.ap1_luminance[1] * ap1[1] + self.ap1_luminance[2] * ap1[2]
    }
}

/// Сплайны ACES в f64 для измерений и тестов (`rrt`, `odt`).
pub fn aces_splines() -> (SplineSpec, SplineSpec) {
    let rrt = SplineSpec::rrt();
    let odt = SplineSpec::odt_48_nits(&rrt);
    (rrt, odt)
}

#[cfg(test)]
#[path = "aces_reference.rs"]
mod reference;

#[cfg(test)]
#[path = "aces_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "aces_color_tests.rs"]
mod color_tests;

#[cfg(test)]
#[path = "aces_settings_tests.rs"]
mod settings_tests;
