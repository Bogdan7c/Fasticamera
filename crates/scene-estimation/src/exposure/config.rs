//! Настройки нашего AE: замер ([`MeteringConfig`]) и контроллер цели ([`ExposureControllerConfig`]).
//!
//! Все числа — **оценка до серой карты и телефона**: сверка в 08c/09 (решение 68). Источник
//! каждого значения по умолчанию написан рядом с ним.

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// Поле должно быть конечным и строго положительным.
fn require_positive(field: &'static str, value: f32) -> Result<(), ConfigError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(invalid(field, "нужно конечное число > 0"))
    }
}

/// Поле должно быть конечным и неотрицательным.
fn require_non_negative(field: &'static str, value: f32) -> Result<(), ConfigError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(invalid(field, "нужно конечное число >= 0"))
    }
}

/// Поле должно лежать в (0, 1].
fn require_unit(field: &'static str, value: f32) -> Result<(), ConfigError> {
    if value.is_finite() && value > 0.0 && value <= 1.0 {
        Ok(())
    } else {
        Err(invalid(field, "нужно число в (0, 1]"))
    }
}

fn invalid(field: &'static str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        field,
        reason: reason.to_owned(),
    }
}

/// Замер по статистике: веса областей, яркость пересвета, пороги качества и ограниченные шаги.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MeteringConfig {
    /// Ширина гаусса матричного замера: доля кадра (по каждой нормированной оси от центра).
    pub matrix_center_sigma: f32,
    /// Вес плитки вне региона точечного замера (до нормировки; плитка внутри региона весит 1).
    pub spot_background_weight: f32,
    /// Яркость пересвеченной ячейки (шкала яркости тона): честная нижняя оценка — «не меньше».
    pub clipped_cell_luminance: f32,
    /// Пол яркости под `log2` (темнее — считается равным полу: шум, ноль в логарифме запрещён).
    pub dark_floor: f32,
    /// Плитка темнее этой яркости считается «тёмной» (для доли тёмных плиток).
    pub dark_tile_luminance: f32,
    /// Доля (по весу) пересвеченных ячеек, с которой замер считается `ClippedHighlights`.
    pub clipped_fraction_limit: f32,
    /// Доля (по весу) пересвеченных ячеек, с которой пересвет «сильный»: точной оценки нет вовсе и
    /// идёт ограниченный шаг вниз. Между `clipped_fraction_limit` и этим порогом замер по-прежнему
    /// считается (с защитой светов), но экспозицию можно только снижать (против предельного цикла
    /// на контровой сцене).
    pub severe_clipped_fraction: f32,
    /// При частичном пересвете: если пересвет + тёмные плитки покрывают не меньше этой доли, в кадре
    /// остались только белое и чёрное — средняя яркость упирается в пол и перестаёт отвечать на
    /// выдержку, поэтому экспозиция держится (дальше вниз не ползём).
    pub blown_or_black_hold_fraction: f32,
    /// Доля (по весу) тёмных плиток, с которой замер считается `TooDark`.
    pub too_dark_fraction_limit: f32,
    /// Минимальный вес плиток с ячейками; меньше — замер не получился (область пуста).
    pub min_used_weight: f32,
    /// С какой доли пересвета матричный замер начинает защиту светов.
    pub highlight_protect_start: f32,
    /// Максимальная защита светов, EV (на пороге `clipped_fraction_limit`).
    pub highlight_protection_max_ev: f32,
    /// Смещение ключа сцены, EV: цель яркости `scene_middle_gray · 2^offset` (0 — ровно средний серый тона).
    pub key_offset_ev: f32,
    /// Шаг вниз при `ClippedHighlights`, EV: точной оценки нет, идём ограниченно.
    pub max_step_down_ev: f32,
    /// Шаг вверх при `TooDark`, EV.
    pub max_step_up_ev: f32,
    /// То же, что `max_step_down_ev`, но на холодном старте потока — пока не было ни одной надёжной (`Measured`)
    /// оценки: крупнее, чтобы не тратить кадры скрытия AWB на плавный спуск с пересвета. Оценка, сверить в 08c.
    pub cold_start_max_step_down_ev: f32,
    /// То же, что `max_step_up_ev`, но на холодном старте потока. Оценка, сверить в 08c.
    pub cold_start_max_step_up_ev: f32,
}

impl Default for MeteringConfig {
    /// Оценка до 08c/09:
    /// * σ 0,35 — центр заметно тяжелее краёв, но углы не нулевые (как у «центро-взвешенного» замера камер);
    /// * фон точечного замера 0,001 — при регионе в десяток плиток фон даёт единицы процентов веса;
    /// * пересвет 1,0 — ячейка не меньше белого сенсора, ничего честнее нижней границы нет;
    /// * пол 0,001 (≈ −10 EV) и «тёмная» плитка 0,003 — шум чтения на уровне долей процента белого;
    /// * пересвет 35 % и тьма 90 % — «значимая часть» области;
    /// * защита светов с 2 % до 35 % пересвета, до −1,5 EV: небо в кадре не должно выжигаться ради лица;
    /// * шаги 2 EV вниз / 3 EV вверх: вниз осторожнее — после пересвета информации о светах нет вовсе;
    /// * шаги холодного старта 5 / 5 EV (решение владельца 76): камера стартует с 4 с·ISO, и AWB не оценивает
    ///   пересвеченный или тёмный кадр, поэтому до первой надёжной оценки экспозицию надо вести быстро.
    fn default() -> Self {
        Self {
            matrix_center_sigma: 0.35,
            spot_background_weight: 0.001,
            clipped_cell_luminance: 1.0,
            dark_floor: 0.001,
            dark_tile_luminance: 0.003,
            clipped_fraction_limit: 0.35,
            severe_clipped_fraction: 0.8,
            blown_or_black_hold_fraction: 0.99,
            too_dark_fraction_limit: 0.9,
            min_used_weight: 0.05,
            highlight_protect_start: 0.02,
            highlight_protection_max_ev: 1.5,
            key_offset_ev: 0.0,
            max_step_down_ev: 2.0,
            max_step_up_ev: 3.0,
            cold_start_max_step_down_ev: 5.0,
            cold_start_max_step_up_ev: 5.0,
        }
    }
}

impl MeteringConfig {
    /// Проверка диапазонов и порядка порогов.
    pub fn validate(&self) -> Result<(), ConfigError> {
        require_positive("metering.matrix_center_sigma", self.matrix_center_sigma)?;
        require_non_negative("metering.spot_background_weight", self.spot_background_weight)?;
        require_positive("metering.clipped_cell_luminance", self.clipped_cell_luminance)?;
        require_positive("metering.dark_floor", self.dark_floor)?;
        require_positive("metering.dark_tile_luminance", self.dark_tile_luminance)?;
        if self.dark_tile_luminance < self.dark_floor {
            return Err(invalid(
                "metering.dark_tile_luminance",
                "тёмная плитка не может быть темнее пола яркости",
            ));
        }
        require_unit("metering.clipped_fraction_limit", self.clipped_fraction_limit)?;
        require_unit("metering.severe_clipped_fraction", self.severe_clipped_fraction)?;
        if self.severe_clipped_fraction <= self.clipped_fraction_limit {
            return Err(invalid(
                "metering.severe_clipped_fraction",
                "сильный пересвет должен быть выше обычного порога пересвета",
            ));
        }
        require_unit(
            "metering.blown_or_black_hold_fraction",
            self.blown_or_black_hold_fraction,
        )?;
        require_unit("metering.too_dark_fraction_limit", self.too_dark_fraction_limit)?;
        require_unit("metering.min_used_weight", self.min_used_weight)?;
        require_non_negative("metering.highlight_protect_start", self.highlight_protect_start)?;
        if self.highlight_protect_start >= self.clipped_fraction_limit {
            return Err(invalid(
                "metering.highlight_protect_start",
                "защита должна начинаться раньше порога пересвета",
            ));
        }
        require_non_negative("metering.highlight_protection_max_ev", self.highlight_protection_max_ev)?;
        if !self.key_offset_ev.is_finite() {
            return Err(invalid("metering.key_offset_ev", "нужно конечное число"));
        }
        require_positive("metering.max_step_down_ev", self.max_step_down_ev)?;
        require_positive("metering.max_step_up_ev", self.max_step_up_ev)?;
        require_positive("metering.cold_start_max_step_down_ev", self.cold_start_max_step_down_ev)?;
        require_positive("metering.cold_start_max_step_up_ev", self.cold_start_max_step_up_ev)?;
        Ok(())
    }
}

/// Контроллер цели: скорость профилей, мёртвая зона видео, блокировка, частота публикаций.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ExposureControllerConfig {
    /// Постоянная времени профиля `Photo`, с.
    pub photo_time_constant_seconds: f32,
    /// Постоянная времени профиля `Video`, с (медленно: без скачков).
    pub video_time_constant_seconds: f32,
    /// Видео: ошибка (EV), с которой цель начинает двигаться.
    pub video_dead_band_start_ev: f32,
    /// Видео: ошибка (EV), ниже которой начатое движение останавливается (гистерезис).
    pub video_dead_band_stop_ev: f32,
    /// Допуск «цель совпала с оценкой сцены» (`Settled`), EV.
    pub settled_tolerance_ev: f32,
    /// Гистерезис: из `Settled` обратно в `Searching` только при ошибке больше этого, EV
    /// (иначе дрожание у границы допуска порождало бы публикации на каждом замере).
    pub unsettle_tolerance_ev: f32,
    /// Максимальный шаг времени сглаживания, с (пауза или смена потока не должны прыгать целью).
    pub max_step_seconds: f32,
    /// Сколько ждём `Settled` после блокировки, прежде чем заморозить цель как есть, с.
    pub lock_settle_timeout_seconds: f32,
    /// Не чаще одной публикации предложения за столько миллисекунд (по меткам сенсора).
    pub publish_min_interval_ms: u32,
    /// Не публиковать изменение цели меньше этого, EV (1/24 EV — ниже шага квантования выдержки).
    pub publish_min_change_ev: f32,
    /// Предел остаточной подтяжки ISP, EV по модулю.
    pub max_residual_ev: f32,
}

impl Default for ExposureControllerConfig {
    /// Оценка, сверить в 08c/09: τ 0,15 с для фото (около 9 кадров — на уровне задержки сенсора) и
    /// 0,8 с для видео (плавно, как у камер); мёртвая зона видео 0,25 → 0,05 EV (глаз не видит
    /// дрейф меньше ~0,1 EV, гистерезис против «дыхания»); допуск 0,1 EV; шаг времени ≤ 0,25 с;
    /// блокировка ждёт сходимость до 1 с; публикации не чаще 20 раз в секунду; остаточная подтяжка до ±0,5 EV (решение 1 владельца).
    fn default() -> Self {
        Self {
            photo_time_constant_seconds: 0.15,
            video_time_constant_seconds: 0.8,
            video_dead_band_start_ev: 0.25,
            video_dead_band_stop_ev: 0.05,
            settled_tolerance_ev: 0.1,
            unsettle_tolerance_ev: 0.2,
            max_step_seconds: 0.25,
            lock_settle_timeout_seconds: 1.0,
            publish_min_interval_ms: 50,
            publish_min_change_ev: 1.0 / 24.0,
            max_residual_ev: 0.5,
        }
    }
}

impl ExposureControllerConfig {
    /// Проверка диапазонов и порядка порогов мёртвой зоны.
    pub fn validate(&self) -> Result<(), ConfigError> {
        require_positive("exposure.photo_time_constant_seconds", self.photo_time_constant_seconds)?;
        require_positive("exposure.video_time_constant_seconds", self.video_time_constant_seconds)?;
        require_positive("exposure.video_dead_band_stop_ev", self.video_dead_band_stop_ev)?;
        require_positive("exposure.video_dead_band_start_ev", self.video_dead_band_start_ev)?;
        if self.video_dead_band_stop_ev >= self.video_dead_band_start_ev {
            return Err(invalid(
                "exposure.video_dead_band_stop_ev",
                "порог остановки должен быть меньше порога начала (иначе нет гистерезиса)",
            ));
        }
        require_positive("exposure.settled_tolerance_ev", self.settled_tolerance_ev)?;
        if !(self.unsettle_tolerance_ev.is_finite() && self.unsettle_tolerance_ev >= self.settled_tolerance_ev) {
            return Err(invalid(
                "exposure.unsettle_tolerance_ev",
                "нужно конечное число не меньше settled_tolerance_ev",
            ));
        }
        require_positive("exposure.max_step_seconds", self.max_step_seconds)?;
        require_positive("exposure.lock_settle_timeout_seconds", self.lock_settle_timeout_seconds)?;
        require_non_negative("exposure.publish_min_change_ev", self.publish_min_change_ev)?;
        require_non_negative("exposure.max_residual_ev", self.max_residual_ev)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
