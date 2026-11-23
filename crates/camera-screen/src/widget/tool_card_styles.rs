//! Содержимое нижней карточки STYLES (решения 187, 201): ряд имён пресетов и «↻» «вернуть правки к пресету».
//!
//! Карточка та же, что у Aspect и Timer (`tool_card.rs`); здесь только то, чего у остальных инструментов нет:
//! * ряд имён — общий [`ChipRow`] (его же берёт меню раздела калибровки), состав — `ScreenState::preset_entries()`,
//!   активный пресет подсвечен жёлтым;
//! * номера таблеток соответствуют `PresetId`, пока ряд не пересобран: соответствие хранит [`StylesRow`], а не
//!   карточка и не редьюсер (редьюсер получает `ToolOption::Style(PresetId)`);
//! * «↻» ведёт в `DiscardEditsTapped` и тускнеет, когда правок поверх пресета нет (`EditState::Clean`).

use look_contract::{EditState, PresetId};
use ui_core::geometry::{Point, Rect};
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;

use super::Ctx;
use super::chip_row::{ChipFace, ChipRow, ChipRowIds, ChipTone, RowEvent, RowPaint};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, VisualIndex};
use super::text_runs::TextRuns;
use crate::action::ScreenAction;
use crate::state::ScreenState;
use crate::tools::ToolOption;

/// Есть ли правки поверх активного пресета. Без фактов о виде правок нет (карточка тогда пуста, «↻» тусклая).
pub(crate) fn styles_have_edits(state: &ScreenState) -> bool {
    state
        .look_facts()
        .is_some_and(|facts| facts.edits == EditState::EditedOnTop)
}

/// Крупное значение карточки: имя активного пресета (пусто, пока фактов нет).
pub(crate) fn active_preset_name(state: &ScreenState) -> String {
    let Some(facts) = state.look_facts() else {
        return String::new();
    };
    facts
        .presets
        .iter()
        .find(|entry| entry.id == facts.active)
        .map_or_else(String::new, |entry| entry.name.as_str().to_owned())
}

/// Таблетки по пресетам: активный выделен. Общий строитель для карточки Styles и ряда имён меню раздела
/// (решение 84: один код на оба места).
pub(crate) fn preset_chip_faces(state: &ScreenState) -> Vec<ChipFace> {
    let active = state.look_facts().map(|facts| facts.active);
    state
        .preset_entries()
        .iter()
        .map(|entry| {
            let tone = if Some(entry.id) == active {
                ChipTone::Highlighted
            } else {
                ChipTone::Plain
            };
            ChipFace::new(entry.name.as_str(), tone)
        })
        .collect()
}

/// Пресеты в том же порядке, что таблетки [`preset_chip_faces`]: по номеру таблетки находится пресет.
pub(crate) fn preset_ids(state: &ScreenState) -> Vec<PresetId> {
    state.preset_entries().iter().map(|entry| entry.id).collect()
}

/// Ряд имён карточки Styles экрана PHOTO.
pub(crate) struct StylesRow {
    row: ChipRow,
    /// Пресеты в порядке таблеток на момент последней сверки.
    presets: Vec<PresetId>,
}

impl StylesRow {
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (card_node, card_rect): (NodeId, Rect),
        row_center: Point,
        row_width_pt: f64,
    ) -> Result<Self, WidgetError> {
        let ids = ChipRowIds {
            chip: ButtonId::StyleChip,
            pan: Some(ScreenGesture::StyleChipsPan),
            chip_label: "styles_chip",
        };
        Ok(Self {
            row: ChipRow::build(
                kit,
                ctx,
                (card_node, card_rect),
                (row_center, row_width_pt),
                (ids, None),
            )?,
            presets: Vec::new(),
        })
    }

    /// Сверяет ряд с состоянием: состав и подсветка берутся из фактов на каждом кадре.
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.presets = preset_ids(ctx.state);
        self.row.set_faces(kit, ctx, preset_chip_faces(ctx.state))?;
        self.row.set_touchable(kit, true)?;
        self.row.sync(kit, ctx)
    }

    pub(crate) fn stop_touches(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        self.row.stop_touches(kit)
    }

    /// Карточка закрылась целиком: ряд забывает состав.
    pub(crate) fn forget(&mut self) {
        self.row.forget();
        self.presets.clear();
    }

    /// Таблетки, не поместившиеся в ряд (диагностика).
    pub(crate) fn overflowed_chips(&self) -> usize {
        self.row.overflowed_chips()
    }

    /// Тап по таблетке → выбор пресета; пан → прокрутка ряда.
    pub(crate) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        actions: &mut Vec<ScreenAction>,
    ) -> Result<bool, WidgetError> {
        match self.row.handle_gesture(kit, event)? {
            RowEvent::NotMine => Ok(false),
            RowEvent::Consumed => Ok(true),
            RowEvent::Activated(index) => {
                if let Some(preset) = self.presets.get(index) {
                    actions.push(ScreenAction::SubmenuOptionSelected(ToolOption::Style(*preset)));
                }
                Ok(true)
            }
        }
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        paint: RowPaint,
    ) -> Result<(), WidgetError> {
        self.row.draw(ctx, visuals, layers, texts, paint)
    }
}
