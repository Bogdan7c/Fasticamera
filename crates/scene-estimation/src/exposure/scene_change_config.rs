//! Настройки детектора «сцена под точкой тапа сменилась» ([`SceneChangeConfig`]).
//!
//! Все числа — **оценка до телефона** (решение 241, приёмка на телефоне после S1–S3): источник каждого значения
//! по умолчанию написан рядом с ним. Секция конфига рендера — `viewfinder.auto_exposure.scene_change`.

use serde::{Deserialize, Serialize};

use super::config::{invalid, require_positive, require_unit};
use crate::error::ConfigError;

/// Настройки детектора смены сцены.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SceneChangeConfig {
    /// Столбцов грубой сетки подписи кадра. Если у статистики плиток меньше — берётся её число.
    pub grid_columns: u32,
    /// Строк грубой сетки подписи кадра.
    pub grid_rows: u32,
    /// Порог ячейки, EV: свет ячейки изменился, если её `log2` ушёл не меньше чем на столько.
    pub cell_threshold_ev: f32,
    /// Доля голосующих ячеек с изменением, с которой кадр «не похож» на эталон.
    pub changed_share: f32,
    /// Сколько миллисекунд (по меткам сенсора) «не похож» должен держаться непрерывно.
    pub confirm_ms: u32,
    /// Сколько измеренных кадров подряд «не похож» должен держаться (не только по времени: редкий кадр
    /// не подтверждает смену сам по себе).
    pub confirm_frames: u32,
    /// Сколько ячеек должно голосовать, чтобы у кадра был вывод; меньше — вывода нет (не «сменилась»). На малой
    /// сетке статистики нужно не больше четверти ячеек подписи (и не меньше одной).
    pub min_voting_cells: u32,
    /// Доля пересвеченных ячеек статистики в ячейке сетки, с которой её свет известен лишь снизу (среднее по всем
    /// ячейкам, пересвеченные — как белые).
    pub clipped_cell_share: f32,
    /// Зелёный ниже этого уровня (доли белого сенсора, до деления на произведение) считается темнотой: свет
    /// известен лишь сверху.
    pub dark_green_level: f32,
}

impl Default for SceneChangeConfig {
    /// Оценка до телефона:
    /// * сетка 8×6 — по ячейке 1/48 кадра: прохожий (20–30 % кадра) закрывает 10–15 ячеек, а ячейка ещё
    ///   усредняет сотни плиток шума;
    /// * порог ячейки 0,5 EV и доля 0,4 подобраны моделью на 12 своих RAW-кадрах (сдвиг окна кадра и пары разных
    ///   сцен при РАВНОМ среднем свете — худший случай): дрожь на 6 % кадра не снимает точку ни разу, сдвиг на 12 % —
    ///   в четверти кадров, на половину кадра — всегда, чужую сцену узнаёт в 96 % пар. Первая оценка 0,7 EV и 0,5
    ///   пропускала пятую часть чужих сцен и две пятых переводов на полкадра: точка снова прилипала бы;
    /// * мерцание сети ±10 % (≈ 0,14 EV) и прохожий в трети кадра порогов не достигают;
    /// * confirm 500 мс и 3 кадра — решение владельца (~0,5 с); при 15 Гц статистики это 7 кадров;
    /// * эталон — с первого годного кадра тапа (подпись от выдержки не зависит), а ячейки эталона с границей
    ///   (пересвет, темнота) уточняются измеренными значениями на кадрах без смены — застрять на сплошном
    ///   белом или чёрном эталоне нельзя;
    /// * 12 голосующих ячеек из 48 (четверть), но не больше четверти ячеек подписи — меньше вывод строить не из чего;
    /// * пересвет ячейки с 1 % — уже такая доля нижней границы нужна: рост экспозиции выжигает ячейки по одной,
    ///   и среднее по оставшимся дало бы ложный голос «стало темнее»; среднее считается по ВСЕМ ячейкам, пересвеченные
    ///   как белые;
    /// * тьма 0,004 белого (≈ −8 EV) — шум чтения на уровне долей процента (как `dark_tile_luminance` замера);
    fn default() -> Self {
        Self {
            grid_columns: 8,
            grid_rows: 6,
            cell_threshold_ev: 0.5,
            changed_share: 0.4,
            confirm_ms: 500,
            confirm_frames: 3,
            min_voting_cells: 12,
            clipped_cell_share: 0.01,
            dark_green_level: 0.004,
        }
    }
}

impl SceneChangeConfig {
    /// Проверка диапазонов; имя поля — в ошибке.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in [
            ("scene_change.grid_columns", self.grid_columns),
            ("scene_change.grid_rows", self.grid_rows),
            ("scene_change.confirm_frames", self.confirm_frames),
            ("scene_change.confirm_ms", self.confirm_ms),
        ] {
            if value == 0 {
                return Err(invalid(field, "нужно число >= 1"));
            }
        }
        require_positive("scene_change.cell_threshold_ev", self.cell_threshold_ev)?;
        require_unit("scene_change.changed_share", self.changed_share)?;
        require_unit("scene_change.clipped_cell_share", self.clipped_cell_share)?;
        require_positive("scene_change.dark_green_level", self.dark_green_level)?;
        let cells = u64::from(self.grid_columns) * u64::from(self.grid_rows);
        if self.min_voting_cells == 0 || u64::from(self.min_voting_cells) > cells {
            return Err(invalid(
                "scene_change.min_voting_cells",
                "нужно число от 1 до числа ячеек сетки (иначе вывода не будет никогда)",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "scene_change_config_tests.rs"]
mod tests;
