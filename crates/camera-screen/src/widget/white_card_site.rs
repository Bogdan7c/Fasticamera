//! Дом карточки белого: экран PHOTO или раздел калибровки (решения 292, 294).
//!
//! Одни и те же кирпичи (число оси, шкала, ряд «Phone» и света, «×») живут в двух местах: подменю у затвора в PHOTO и
//! нижней карточке раздела внутри повёрнутого корня. Различий немного, и все они собраны ЗДЕСЬ, а не размазаны по
//! кирпичам `if section`:
//! * геометрия: абсолютная геометрия экрана или та же, но на глазном прямоугольнике раздела (`ScreenGeometry::in_screen`);
//! * номера кнопок и пана: у карточки PHOTO свои, у раздела свои ([`ButtonId::SectionWhite`]) — иначе событие ушло бы
//!   в карточку PHOTO, которая живёт всегда;
//! * оси: пан приходит в осях поверхности, а раздел думает глазами ([`SectionSpace::eye_pan`]);
//! * данные касания и смещения содержимого: в разделе кнопка стоит под повёрнутым корнем, поэтому положение пальца
//!   переводится в рамку узла, а сдвиги (въезд карточки, тряска) прикладываются в осях узла, а не поверхности;
//! * состав ряда: в разделе «Auto» нет (решение 294);
//! * «открыта ли» — спрашивается у своего владельца состояния.

use std::sync::Arc;

use ui_core::gesture::{GestureData, PanDirection};
use ui_core::id::NodeId;
use ui_draw::transform::Affine2;
use ui_kit::pan::PanUpdate;

use super::calibration::space::{EyeAxis, SectionSpace};
use super::error::WidgetError;
use super::geometry::ScreenGeometry;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use crate::calibration::SectionCard;
use crate::state::{PanelState, ScreenState};
use crate::tools::ToolId;
use crate::white::WhiteAxis;
use crate::white_row::{WhiteRowLayout, WhiteRowSlot};

/// Кнопка карточки белого — одинаково названа в обоих домах (соответствие номерам кнопок решает [`WhiteCardSite`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WhiteCardButton {
    /// «×» карточки.
    Close,
    /// Число оси (тап отдаёт шкалу этой оси).
    Axis(WhiteAxis),
    /// Кнопка ряда: «Auto», «Phone» или свет.
    Row(WhiteRowSlot),
}

/// Где стоит карточка.
#[derive(Debug, Clone)]
enum Home {
    /// Подменю у затвора на экране PHOTO.
    Photo,
    /// Нижняя карточка раздела калибровки в его глазном пространстве.
    /// Пространство раздела большое (раскладка по значению), поэтому общее.
    Section(Arc<SectionSpace>),
}

/// Дом карточки белого: геометрия и всё, что от неё зависит (см. модуль).
#[derive(Debug, Clone)]
pub(crate) struct WhiteCardSite {
    /// Общая (`Arc`): сайт копируется в кирпичи карточки, а геометрия велика — по значению она раздула бы стек сборки.
    geometry: Arc<ScreenGeometry>,
    home: Home,
}

impl WhiteCardSite {
    /// Карточка PHOTO: геометрия экрана как есть.
    pub(crate) fn photo(geometry: &ScreenGeometry) -> Self {
        Self {
            geometry: Arc::new(geometry.clone()),
            home: Home::Photo,
        }
    }

    /// Карточка раздела: `eye` — геометрия на глазном прямоугольнике раздела (`ScreenGeometry::in_screen`).
    pub(crate) fn section(eye: &ScreenGeometry, space: &SectionSpace) -> Self {
        Self {
            geometry: Arc::new(eye.clone()),
            home: Home::Section(Arc::new(*space)),
        }
    }

    /// Геометрия, по которой строится карточка.
    pub(crate) fn geometry(&self) -> &ScreenGeometry {
        &self.geometry
    }

    /// Состав ряда: в разделе без «Auto».
    pub(crate) fn row_layout(&self) -> WhiteRowLayout {
        match &self.home {
            Home::Photo => WhiteRowLayout::WithAuto,
            Home::Section(_) => WhiteRowLayout::WithoutAuto,
        }
    }

    /// Открыта ли карточка именно в этом доме (карточка соседнего дома шкалу и кнопки этого не будит).
    pub(crate) fn is_open(&self, state: &ScreenState) -> bool {
        match &self.home {
            Home::Photo => state.panel() == PanelState::Submenu(ToolId::White),
            Home::Section(_) => state.calibration().card() == SectionCard::White,
        }
    }

    /// Номер кнопки в этом доме.
    pub(crate) fn button_id(&self, button: WhiteCardButton) -> Result<ButtonId, WidgetError> {
        if matches!(&self.home, Home::Section(_)) {
            return Ok(ButtonId::SectionWhite(button));
        }
        Ok(match button {
            WhiteCardButton::Close => ButtonId::WhiteClose,
            WhiteCardButton::Axis(axis) => ButtonId::WhiteAxis(axis),
            WhiteCardButton::Row(WhiteRowSlot::Auto) => ButtonId::WhiteAuto,
            WhiteCardButton::Row(WhiteRowSlot::Phone) => ButtonId::WhitePhone,
            WhiteCardButton::Row(WhiteRowSlot::Light(light)) => {
                ButtonId::WhiteLight(u8::try_from(light).map_err(|_| {
                    WidgetError::Inconsistent(format!("кнопок света {} больше, чем помещается в номер", light + 1))
                })?)
            }
        })
    }

    /// Какая кнопка карточки стоит за номером `id` в этом доме; `None` — номер чужой.
    pub(crate) fn button_of(&self, id: ButtonId) -> Option<WhiteCardButton> {
        match (&self.home, id) {
            (Home::Section(_), ButtonId::SectionWhite(button)) => Some(button),
            (Home::Section(_), _) => None,
            (Home::Photo, ButtonId::WhiteClose) => Some(WhiteCardButton::Close),
            (Home::Photo, ButtonId::WhiteAxis(axis)) => Some(WhiteCardButton::Axis(axis)),
            (Home::Photo, ButtonId::WhiteAuto) => Some(WhiteCardButton::Row(WhiteRowSlot::Auto)),
            (Home::Photo, ButtonId::WhitePhone) => Some(WhiteCardButton::Row(WhiteRowSlot::Phone)),
            (Home::Photo, ButtonId::WhiteLight(light)) => {
                Some(WhiteCardButton::Row(WhiteRowSlot::Light(usize::from(light))))
            }
            (Home::Photo, _) => None,
        }
    }

    /// Пан шкалы: направление распознавателя на поверхности и номер жеста.
    pub(crate) fn ruler_pan(&self) -> (PanDirection, ScreenGesture) {
        match &self.home {
            Home::Photo => (PanDirection::Horizontal, ScreenGesture::WhiteRulerPan),
            Home::Section(space) => (
                space.pan_direction(EyeAxis::Horizontal),
                ScreenGesture::SectionWhiteRulerPan,
            ),
        }
    }

    /// Пан закрытия карточки (вниз глазами): направление и номер жеста.
    pub(crate) fn close_pan(&self) -> (PanDirection, ScreenGesture) {
        match &self.home {
            Home::Photo => (PanDirection::Down, ScreenGesture::DrawerClosePan),
            Home::Section(space) => (
                space.pan_direction(EyeAxis::Vertical),
                ScreenGesture::CalibrationMenuClosePan,
            ),
        }
    }

    /// Обновление пана в осях шкалы: в разделе поворачивается в глазные оси.
    pub(crate) fn scale_pan(&self, update: PanUpdate) -> PanUpdate {
        match &self.home {
            Home::Photo => update,
            Home::Section(space) => space.eye_pan(update),
        }
    }

    /// Положение пальца в рамке узла кнопки: в разделе узел под повёрнутым корнем, в PHOTO данные как есть.
    pub(crate) fn press_data(&self, kit: &SceneKit, node: NodeId, data: GestureData) -> GestureData {
        match &self.home {
            Home::Photo => data,
            Home::Section(_) => kit.gesture_data_in_node(node, data),
        }
    }

    /// Узел значка: в PHOTO он крутится вместе с телефоном, в разделе стоит (поворачивает корень).
    pub(crate) fn icon_node(
        &self,
        kit: &mut SceneKit,
        host: NodeId,
        label: &'static str,
    ) -> Result<NodeId, WidgetError> {
        match &self.home {
            Home::Photo => kit.add_rotating_content(host, label),
            Home::Section(_) => kit.add_fixed_content(host, label),
        }
    }

    /// Поворот значка «×» (внутри раздела корень повёрнут сам).
    pub(crate) fn icon_turn(&self) -> super::side_button::IconTurn {
        match &self.home {
            Home::Photo => super::side_button::IconTurn::FollowsPhone,
            Home::Section(_) => super::side_button::IconTurn::Fixed,
        }
    }

    /// Прикладывает сдвиг `offset` (въезд карточки, тряска, смещение значка) к размещению `placed`. В PHOTO сдвиг
    /// идёт ПОСЛЕ размещения (в осях поверхности — так исторически рисуется карточка), в разделе ДО него (в осях
    /// узла, чтобы сдвиг в альбоме шёл вдоль глазной вертикали, а не вбок).
    pub(crate) fn offset_placed(&self, placed: Affine2, offset: Affine2) -> Affine2 {
        match &self.home {
            Home::Photo => placed.then(offset),
            Home::Section(_) => offset.then(placed),
        }
    }
}

#[cfg(test)]
#[path = "white_card_site_tests.rs"]
mod tests;
