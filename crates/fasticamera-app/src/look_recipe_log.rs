//! Строки лога рецепта вида для release (сессия 14o, пункты 6 и «снимок = видоискатель»).
//!
//! Зачем. Подобранные значения вида раньше можно было снять только с debug через `run-as`. Release пишет в logcat
//! (фильтр `info`, буфер 256 КиБ) по одной машиночитаемой строке в ТОЧКАХ ФИКСАЦИИ: старт, смена пресета,
//! остановка правок, переключение чистого вида, заявка и снятие заявки сценария, нажатие снимка. Не на каждый
//! шаг ручки: буфер небольшой, а разбору нужны итоги, а не траектория пальца.
//!
//! Формат (читает `scripts/look_recipe_log.py`; менять только вместе с ним):
//! ```text
//! look_recipe origin=<origin> fp=<16 hex> base=<agx|aces|filmic|legacy> settings=<компактный json>
//! look_still_press fp=<16 hex> view_fp=<16 hex|unknown> clean_view=<show_edited|show_base> scenario_claim=<held|free>
//! ```
//! Отпечаток `fp` — [`LookRecipe::fingerprint`] (одна функция и для видоискателя, и для снимка).
//! Модуль только форматирует и пишет; решать, когда писать, — дело вызывающего.

use look_contract::{CleanView, LookRecipe};

/// Цель трассировки (та же, что у исполнителя вида: один фильтр logcat).
const LOG_TARGET: &str = "fasticamera::look";

/// Точка фиксации: почему записана строка рецепта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeOrigin {
    /// Рецепт, с которым запущен процесс (активный пресет из файла).
    Startup,
    /// Смена или правка набора пресетов: выбор, сохранение, перезапись, удаление, сброс правок.
    PresetChanged,
    /// Правки ручек остановились (покой на несколько кадров): итог жеста.
    EditSettled,
    /// Нажат затвор, а правки ещё не «осели»: фиксируются перед снимком, чтобы снимок сверялся с записанным.
    EditBeforeStill,
    /// Чистый вид включён (показ основы).
    CleanViewShowBase,
    /// Чистый вид выключен (показ правленого вида).
    CleanViewShowEdited,
    /// Сценарий замера заявил вид на рендере: записан рецепт, который он показывает.
    ScenarioClaim,
    /// Сценарий снял заявку: записан рецепт продукта, к которому вернулся рендер.
    ScenarioRelease,
}

impl RecipeOrigin {
    /// Имя в строке лога (`snake_case`, читает разбор).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::PresetChanged => "preset_changed",
            Self::EditSettled => "edit_settled",
            Self::EditBeforeStill => "edit_before_still",
            Self::CleanViewShowBase => "clean_view_show_base",
            Self::CleanViewShowEdited => "clean_view_show_edited",
            Self::ScenarioClaim => "scenario_claim",
            Self::ScenarioRelease => "scenario_release",
        }
    }
}

/// Сколько кадров подряд без правок ручек нужно, чтобы жест считался законченным и его итог попал в лог.
/// `[log]` Не настройка вида (вид мы не подбираем): лишь частота записи в буфер logcat. 30 кадров ≈ полсекунды на
/// 60 к/с — пауза между жестами, но короче, чем владелец успевает нажать затвор после правки.
pub const EDIT_SETTLE_FRAMES: u32 = 30;

/// Следит за правками ручек и говорит, когда жест закончился (правок нет [`EDIT_SETTLE_FRAMES`] кадров подряд).
/// Считает кадры, а не секунды: без часов, поэтому поведение детерминированно и проверяется тестами.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditSettleTracker {
    /// `Some(n)` — есть правка, не попавшая в лог; `n` — сколько кадров прошло без новых правок.
    quiet_frames: Option<u32>,
}

impl EditSettleTracker {
    /// Применена правка ручек: жест идёт, отсчёт покоя начинается заново.
    pub fn note_edit(&mut self) {
        self.quiet_frames = Some(0);
    }

    /// Прошёл кадр. `true` — правки осели ровно сейчас: итог пора записать (один раз на жест).
    pub fn tick(&mut self) -> bool {
        let Some(quiet) = self.quiet_frames else {
            return false;
        };
        let quiet = quiet + 1;
        if quiet >= EDIT_SETTLE_FRAMES {
            self.quiet_frames = None;
            return true;
        }
        self.quiet_frames = Some(quiet);
        false
    }

    /// Забирает незаписанную правку досрочно (нажат затвор). `true` — правка была.
    pub fn take_pending(&mut self) -> bool {
        self.quiet_frames.take().is_some()
    }

    /// Правка, которую ещё не записали в лог, есть.
    pub fn has_pending(&self) -> bool {
        self.quiet_frames.is_some()
    }
}

/// Строка `look_recipe …` без префикса logcat.
pub fn recipe_line(origin: RecipeOrigin, recipe: &LookRecipe) -> String {
    // Сериализация проверенных значений не падает; если всё же упала, строка честно говорит об этом, а не молчит.
    let settings = serde_json::to_string(&recipe.settings)
        .unwrap_or_else(|error| format!("{{\"serialization_error\":\"{error}\"}}"));
    format!(
        "look_recipe origin={} fp={} base={} settings={}",
        origin.name(),
        recipe.fingerprint_hex(),
        recipe.base.name(),
        settings
    )
}

/// Пишет строку рецепта уровнем `info` (release-фильтр его пропускает).
pub fn log_recipe(origin: RecipeOrigin, recipe: &LookRecipe) {
    tracing::info!(target: LOG_TARGET, "{}", recipe_line(origin, recipe));
}

/// Что рендер показывает по доставленным командам в момент нажатия.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewfinderAtPress {
    /// Подтверждённый показ: основа и ручки доставлены.
    Confirmed(LookRecipe),
    /// Доставка не подтверждена (рендер мог остаться на другом виде).
    Unknown,
}

/// Состояние заявки сценария в момент нажатия.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioClaimAtPress {
    /// Сценарий владеет видом на рендере.
    Held,
    /// Заявки нет.
    Free,
}

/// Строка `look_still_press …`: отпечаток рецепта, с которым уходит снимок, и отпечаток вида на рендере.
pub fn still_press_line(
    still: &LookRecipe,
    viewfinder: &ViewfinderAtPress,
    clean_view: CleanView,
    claim: ScenarioClaimAtPress,
) -> String {
    let view_fp = match viewfinder {
        ViewfinderAtPress::Confirmed(recipe) => recipe.fingerprint_hex(),
        ViewfinderAtPress::Unknown => "unknown".to_owned(),
    };
    let clean = match clean_view {
        CleanView::ShowEdited => "show_edited",
        CleanView::ShowBase => "show_base",
    };
    let claim = match claim {
        ScenarioClaimAtPress::Held => "held",
        ScenarioClaimAtPress::Free => "free",
    };
    format!(
        "look_still_press fp={} view_fp={view_fp} clean_view={clean} scenario_claim={claim}",
        still.fingerprint_hex()
    )
}

/// Пишет строку нажатия уровнем `info`.
pub fn log_still_press(
    still: &LookRecipe,
    viewfinder: &ViewfinderAtPress,
    clean_view: CleanView,
    claim: ScenarioClaimAtPress,
) {
    tracing::info!(target: LOG_TARGET, "{}", still_press_line(still, viewfinder, clean_view, claim));
}

#[cfg(test)]
#[path = "look_recipe_log_tests.rs"]
mod tests;
