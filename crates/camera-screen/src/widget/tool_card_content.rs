//! Содержимое нижней карточки по инструменту: заголовок, крупное значение, ряд вариантов, выбранный вариант.
//!
//! Чистые функции от `(состояние, конфиг)` без сцены и рисования: что написано в карточке — решает этот
//! модуль, где и как это нарисовано — `tool_card.rs` и `tool_card_options.rs`. Тексты на английском (UI).
//! Содержимое Timer/Flash/RAW/Grid/Level — `[estimate]` по аналогии с Aspect (видео D бета-27 показывает тот
//! же шаблон «заголовок — значение — ряд»); Aspect и Exposure измерены (`[measured: C t=40.0, t=54.0]`).

use ui_draw::icons::IconId;

use super::exposure_link::format_ev;
use crate::config::ScreenConfig;
use crate::settings::{AspectRatio, FlashMode, GridSetting, LevelSetting, RawCapture, TimerSetting};
use crate::state::{PanelState, ScreenState};
use crate::tools::{ToolId, ToolOption};

/// Как нарисован вариант в ряду: значок или короткий текст.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OptionFace {
    Icon(IconId),
    Label(String),
}

/// Вариант ряда: что выбирается и как выглядит.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OptionEntry {
    pub option: ToolOption,
    pub face: OptionFace,
}

/// Инструмент открытой карточки: подменю `tool` или панель Exposure. `None` — карточки нет.
pub(crate) fn open_card_tool(state: &ScreenState) -> Option<ToolId> {
    match state.panel() {
        PanelState::Submenu(tool) => Some(tool),
        PanelState::ExposurePanel => Some(ToolId::Exposure),
        PanelState::Closed | PanelState::ControlsMenu => None,
    }
}

/// Имя инструмента ЗАГЛАВНЫМИ: заголовок карточки и подпись пункта шторки Controls (одна функция на оба места,
/// чтобы подписи не разошлись).
pub(crate) fn tool_title(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Timer => "TIMER",
        ToolId::Flash => "FLASH",
        ToolId::Aspect => "ASPECT",
        ToolId::Raw => "RAW",
        ToolId::Exposure => "EXPOSURE",
        ToolId::Grid => "GRID",
        ToolId::Level => "LEVEL",
        ToolId::Live => "LIVE",
        ToolId::Styles => "STYLES",
        ToolId::Night => "NIGHT",
    }
}

fn on_off(is_on: bool) -> &'static str {
    if is_on { "On" } else { "Off" }
}

/// Подпись таймера: `Off` или `3s`.
fn timer_value(config: &ScreenConfig, setting: TimerSetting) -> String {
    config
        .timer
        .seconds_for(setting)
        .map_or_else(|| "Off".to_owned(), |seconds| format!("{seconds}s"))
}

fn aspect_label(aspect: AspectRatio) -> &'static str {
    match aspect {
        AspectRatio::FourByThree => "4:3",
        AspectRatio::SixteenByNine => "16:9",
        AspectRatio::Square => "1:1",
    }
}

/// Значок режима вспышки: молния / молния с «A» / перечёркнутая молния. Один источник для значка над
/// кадром, пункта шторки и карточки.
pub(crate) const fn flash_icon(mode: FlashMode) -> IconId {
    match mode {
        FlashMode::Off => IconId::FlashOff,
        FlashMode::On => IconId::FlashOn,
        FlashMode::Auto => IconId::FlashAuto,
    }
}

/// Подпись режима вспышки для крупного значения карточки.
fn flash_label(mode: FlashMode) -> &'static str {
    match mode {
        FlashMode::Auto => "Auto",
        FlashMode::On => "On",
        FlashMode::Off => "Off",
    }
}

/// Крупное значение карточки по текущему состоянию («Off», «3s», «4:3», «−2.0»).
pub(crate) fn card_value(state: &ScreenState, config: &ScreenConfig, tool: ToolId) -> String {
    let settings = state.settings();
    match tool {
        ToolId::Timer => timer_value(config, settings.timer),
        ToolId::Flash => flash_label(settings.flash).to_owned(),
        ToolId::Raw => on_off(settings.raw == RawCapture::On).to_owned(),
        ToolId::Aspect => aspect_label(settings.aspect).to_owned(),
        ToolId::Grid => on_off(settings.grid == GridSetting::On).to_owned(),
        ToolId::Level => on_off(settings.level == LevelSetting::On).to_owned(),
        ToolId::Exposure => format_ev(state.exposure().bias(&config.exposure).ev()),
        ToolId::Live | ToolId::Styles | ToolId::Night => String::new(),
    }
}

fn label(option: ToolOption, text: &str) -> OptionEntry {
    OptionEntry {
        option,
        face: OptionFace::Label(text.to_owned()),
    }
}

fn icon(option: ToolOption, icon: IconId) -> OptionEntry {
    OptionEntry {
        option,
        face: OptionFace::Icon(icon),
    }
}

/// Есть ли у инструмента ряд вариантов. Дёшево (без выделений памяти): виджет спрашивает это на каждом кадре,
/// а сам ряд строит [`option_entries`] только при смене инструмента.
pub(crate) fn has_option_row(tool: ToolId) -> bool {
    match tool {
        ToolId::Timer | ToolId::Flash | ToolId::Aspect | ToolId::Raw | ToolId::Grid | ToolId::Level => true,
        ToolId::Exposure | ToolId::Live | ToolId::Styles | ToolId::Night => false,
    }
}

/// Ряд вариантов инструмента слева направо. Выбор «Auto» вспышки не предлагается (инвариант 6); у вспышки
/// порядок iOS без Auto: «On, Off». У Exposure ряда нет (там линейка).
pub(crate) fn option_entries(config: &ScreenConfig, tool: ToolId) -> Vec<OptionEntry> {
    let off_on = |off: ToolOption, on: ToolOption| vec![label(off, "Off"), label(on, "On")];
    match tool {
        ToolId::Timer => {
            // Первым стоит значок «таймер выключен», дальше — секунды из конфига.
            let mut row = vec![icon(ToolOption::Timer(TimerSetting::Off), IconId::Timer)];
            for setting in [TimerSetting::Seconds3, TimerSetting::Seconds5, TimerSetting::Seconds10] {
                let seconds = config
                    .timer
                    .seconds_for(setting)
                    .map_or_else(String::new, |n| n.to_string());
                row.push(label(ToolOption::Timer(setting), &seconds));
            }
            row
        }
        ToolId::Flash => vec![
            // Порядок карточки iOS 27: Auto, On, Off.
            icon(ToolOption::Flash(FlashMode::Auto), flash_icon(FlashMode::Auto)),
            icon(ToolOption::Flash(FlashMode::On), flash_icon(FlashMode::On)),
            icon(ToolOption::Flash(FlashMode::Off), flash_icon(FlashMode::Off)),
        ],
        ToolId::Aspect => [
            AspectRatio::FourByThree,
            AspectRatio::SixteenByNine,
            AspectRatio::Square,
        ]
        .into_iter()
        .map(|aspect| label(ToolOption::Aspect(aspect), aspect_label(aspect)))
        .collect(),
        ToolId::Raw => off_on(ToolOption::Raw(RawCapture::Off), ToolOption::Raw(RawCapture::On)),
        ToolId::Grid => off_on(ToolOption::Grid(GridSetting::Off), ToolOption::Grid(GridSetting::On)),
        ToolId::Level => off_on(
            ToolOption::Level(LevelSetting::Off),
            ToolOption::Level(LevelSetting::On),
        ),
        ToolId::Exposure | ToolId::Live | ToolId::Styles | ToolId::Night => Vec::new(),
    }
}

/// Выбранный сейчас вариант инструмента (владелец выбора — состояние экрана).
pub(crate) fn current_option(state: &ScreenState, tool: ToolId) -> Option<ToolOption> {
    let settings = state.settings();
    match tool {
        ToolId::Flash => Some(ToolOption::Flash(settings.flash)),
        ToolId::Raw => Some(ToolOption::Raw(settings.raw)),
        ToolId::Timer => Some(ToolOption::Timer(settings.timer)),
        ToolId::Aspect => Some(ToolOption::Aspect(settings.aspect)),
        ToolId::Grid => Some(ToolOption::Grid(settings.grid)),
        ToolId::Level => Some(ToolOption::Level(settings.level)),
        ToolId::Exposure | ToolId::Live | ToolId::Styles | ToolId::Night => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::ScreenAction;
    use crate::test_support::Screen;

    #[test]
    fn titles_are_uppercase_tool_names() {
        let titles: Vec<_> = [
            ToolId::Timer,
            ToolId::Flash,
            ToolId::Aspect,
            ToolId::Raw,
            ToolId::Exposure,
        ]
        .into_iter()
        .map(tool_title)
        .collect();
        assert_eq!(titles, ["TIMER", "FLASH", "ASPECT", "RAW", "EXPOSURE"]);
    }

    #[test]
    fn has_option_row_agrees_with_the_entries_it_guards() {
        let config = ScreenConfig::default();
        for tool in [
            ToolId::Flash,
            ToolId::Raw,
            ToolId::Timer,
            ToolId::Exposure,
            ToolId::Aspect,
            ToolId::Grid,
            ToolId::Level,
            ToolId::Live,
            ToolId::Styles,
            ToolId::Night,
        ] {
            assert_eq!(
                has_option_row(tool),
                !option_entries(&config, tool).is_empty(),
                "{tool:?}"
            );
        }
    }

    #[test]
    fn option_rows_follow_the_approved_sketch() {
        let config = ScreenConfig::default();
        let faces = |tool| -> Vec<OptionFace> {
            option_entries(&config, tool)
                .into_iter()
                .map(|entry| entry.face)
                .collect()
        };
        let text = |value: &str| OptionFace::Label(value.to_owned());
        assert_eq!(
            faces(ToolId::Timer),
            [OptionFace::Icon(IconId::Timer), text("3"), text("5"), text("10")]
        );
        assert_eq!(
            faces(ToolId::Flash),
            [
                OptionFace::Icon(IconId::FlashAuto),
                OptionFace::Icon(IconId::FlashOn),
                OptionFace::Icon(IconId::FlashOff)
            ]
        );
        assert_eq!(faces(ToolId::Aspect), [text("4:3"), text("16:9"), text("1:1")]);
        assert_eq!(faces(ToolId::Raw), [text("Off"), text("On")]);
        assert!(faces(ToolId::Exposure).is_empty(), "у Exposure вместо ряда линейка");
    }

    #[test]
    fn values_follow_the_state_after_selection() {
        let config = ScreenConfig::default();
        let mut screen = Screen::ready();
        assert_eq!(card_value(&screen.state, &config, ToolId::Timer), "Off");
        screen.step(ScreenAction::ControlsMenuOpenRequested);
        screen.step(ScreenAction::ControlsItemSelected(ToolId::Timer));
        screen.step(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
            TimerSetting::Seconds5,
        )));
        assert_eq!(card_value(&screen.state, &config, ToolId::Timer), "5s");
        assert_eq!(
            current_option(&screen.state, ToolId::Timer),
            Some(ToolOption::Timer(TimerSetting::Seconds5))
        );
        assert_eq!(open_card_tool(&screen.state), Some(ToolId::Timer));
        assert_eq!(card_value(&screen.state, &config, ToolId::Aspect), "4:3");
        assert_eq!(card_value(&screen.state, &config, ToolId::Flash), "Off");
        for (mode, label) in [
            (FlashMode::Auto, "Auto"),
            (FlashMode::On, "On"),
            (FlashMode::Off, "Off"),
        ] {
            screen.state.settings.flash = mode;
            assert_eq!(card_value(&screen.state, &config, ToolId::Flash), label);
        }
    }
}
