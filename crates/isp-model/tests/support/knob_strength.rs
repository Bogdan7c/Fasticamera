//! Стенд силы ручек вида: серая рампа сцены проходит через ПОЛНУЮ разрешённую цепочку кадра
//! (`IspParams::resolve` с `LookControls` → основа тона → стадия вида → `to_output` → кривая выхода),
//! на выходе — код экрана `0…255` (после OETF Display P3, без подтяжки AE).
//!
//! Стенд принимает `LookSettings` и `LookModelSettings`: те же замеры работают для любой модели вида. Все четыре
//! основы (AgX, ACES, filmic, Legacy) идут одним путём. Цветные ручки меряет соседний модуль [`super::color_chart`]
//! (мишень из RAW-кадра через весь эталонный конвейер).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "помощник тестов: сбой стенда — провал теста"
)]

use std::sync::Arc;

use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::look::LookControls;
use isp_model::look::model::LookModelSettings;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::sensor::SensorDescription;
use isp_model::tone::ToneCurveSettings;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use look_contract::{LookKnob, LookSettings};

use super::real_capture::load_real_capture;

/// Ступени сцены рампы относительно серого сцены: от глубокой тени до света выше белого точки.
const RAMP_FIRST_EV: f32 = -10.0;
const RAMP_LAST_EV: f32 = 6.0;
/// Шаг рампы по умолчанию: 1/16 ступени (пики ручек теней и светов лежат между целыми ступенями).
pub const FINE_RAMP_STEP_EV: f32 = 1.0 / 16.0;
/// Масштаб кода экрана.
const CODE_SCALE: f32 = 255.0;

/// Четыре основы тона с умолчаниями продукта (имя для таблиц, основа).
pub fn all_tone_bases() -> [(&'static str, ToneBase); 4] {
    [
        ("AgX", ToneBase::Agx(AgxSettings::default())),
        ("ACES", ToneBase::Aces(AcesSettings::default())),
        ("filmic", ToneBase::Filmic(FilmicSettings::default())),
        ("Legacy", ToneBase::Legacy),
    ]
}

/// Ручка со значением (в тестах значения заведомо допустимы).
pub fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("значение ручки в −1…+1")
}

/// Наибольшие сдвиги кода экрана относительно нейтрали и где они случились.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeShift {
    /// Наибольший сдвиг вверх, коды из 255 (≥ 0).
    pub up_codes: f32,
    /// На каком ходе рампы (ступени от серого сцены) он случился.
    pub up_at_ev: f32,
    /// Наибольший сдвиг вниз, коды из 255 (≤ 0).
    pub down_codes: f32,
    /// На каком ходе рампы случился.
    pub down_at_ev: f32,
}

/// Стенд: сенсор и кадровые входы настоящей фикстуры, чтобы `IspParams::resolve` шёл как в продукте.
pub struct KnobBench {
    pub(super) sensor: SensorDescription,
    pub(super) frame: FrameInputs,
    pub(super) neutral: CameraNeutral,
    /// Шаг рампы, ступени.
    ramp_step_ev: f32,
}

impl KnobBench {
    /// Стенд с мелкой рампой (1/16 EV): настоящие максимумы сдвига.
    pub fn new() -> Self {
        Self::with_ramp_step_ev(FINE_RAMP_STEP_EV)
    }

    /// Стенд с рампой заданного шага, ступени.
    pub fn with_ramp_step_ev(ramp_step_ev: f32) -> Self {
        let capture = load_real_capture(&super::assets_dir(), "real-main-lamp-gray-tile");
        let frame = FrameInputs {
            levels: capture.sensor.static_levels(),
        };
        Self {
            sensor: capture.sensor,
            frame,
            neutral: capture.neutral,
            ramp_step_ev,
        }
    }

    /// Разрешённые параметры кадра: выход Display P3, без подтяжки AE, вид — `settings` по модели `model`.
    pub fn resolve(
        &self,
        base: &ToneBase,
        settings: &LookSettings,
        model: &LookModelSettings,
    ) -> Result<IspParams, String> {
        let controls =
            ProcessingControls::with_defaults(self.neutral, DemosaicMethod::Quality, DisplayGamut::DisplayP3);
        let controls = ProcessingControls {
            tone_base: *base,
            ..controls
        }
        .with_look(LookControls::new(settings.clone(), Arc::new(model.clone())));
        IspParams::resolve(&self.sensor, &self.frame, &controls).map_err(|error| error.to_string())
    }

    /// Код экрана `0…255` (среднее по каналам) серого сцены с линейным уровнем `scene_level` по параметрам кадра.
    pub fn grey_code(params: &IspParams, scene_level: f32) -> f32 {
        let scene = [scene_level; 3];
        // Основы в `W` — через общий контракт; прежняя — своя кривая в цветах выхода (как в `run_reference`).
        let display_linear = match params.tone_base.working_stage() {
            Some(stage) => {
                let display = stage.map_scene_pixel(scene, 1.0);
                stage.to_output(params.look.apply_to_pixel(display))
            }
            None => params
                .look
                .apply_to_pixel(params.tone.map_pixel(scene))
                .map(|channel| channel.clamp(0.0, 1.0)),
        };
        let transfer = params.output_profile.transfer();
        let encoded = display_linear.map(|channel| transfer.encode(channel));
        CODE_SCALE * (encoded[0] + encoded[1] + encoded[2]) / 3.0
    }

    /// Коды рампы: (ступени от серого сцены, код) для серых с шагом стенда и белого точки (уровень 1,0).
    fn ramp_codes(&self, params: &IspParams) -> Vec<(f32, f32)> {
        let scene_middle_gray = ToneCurveSettings::default().scene_middle_gray;
        let steps = ((RAMP_LAST_EV - RAMP_FIRST_EV) / self.ramp_step_ev).round() as u32;
        let mut ramp: Vec<(f32, f32)> = (0..=steps)
            .map(|step| {
                let ev = RAMP_FIRST_EV + step as f32 * self.ramp_step_ev;
                (ev, Self::grey_code(params, scene_middle_gray * ev.exp2()))
            })
            .collect();
        // Белый точки: уровень 1,0 лежит на ступенях `log2(1/серый)` выше серого.
        ramp.push(((1.0 / scene_middle_gray).log2(), Self::grey_code(params, 1.0)));
        ramp
    }

    /// Сдвиг кода рампы при `settings` относительно нейтрали ТОЙ ЖЕ модели вида и основы. Ошибка — текст причины,
    /// по которой основа или модель не разрешились (тест обязан упасть, а не получить нулевой сдвиг).
    pub fn measure(
        &self,
        base: &ToneBase,
        settings: &LookSettings,
        model: &LookModelSettings,
    ) -> Result<EdgeShift, String> {
        let neutral = self
            .resolve(base, &LookSettings::NEUTRAL, model)
            .map(|params| self.ramp_codes(&params))
            .map_err(|reason| format!("нейтраль: {reason}"))?;
        let touched = self
            .resolve(base, settings, model)
            .map(|params| self.ramp_codes(&params))?;
        let mut shift = EdgeShift {
            up_codes: 0.0,
            up_at_ev: 0.0,
            down_codes: 0.0,
            down_at_ev: 0.0,
        };
        for ((ev, before), (_, after)) in neutral.iter().zip(&touched) {
            let delta = after - before;
            if delta > shift.up_codes {
                (shift.up_codes, shift.up_at_ev) = (delta, *ev);
            }
            if delta < shift.down_codes {
                (shift.down_codes, shift.down_at_ev) = (delta, *ev);
            }
        }
        Ok(shift)
    }
}
