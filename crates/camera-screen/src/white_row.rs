//! Места кнопок в ряду карточки белого (WB48): «Auto», «Phone», потом кнопки света по порядку конфига.
//!
//! Единственный владелец соответствия «кнопка ↔ место в ряду». Виджет ряда, цели касаний для стендов, проверка
//! ширины и тесты спрашивают место здесь и не считают номера сами: порядок меняется в одном месте
//! ([`WhiteRowSlot::position`]), а число кнопок-источников ([`SOURCE_BUTTON_COUNT`]) выводится из него же.

/// Кнопка ряда белого. Порядок слева направо задаёт [`WhiteRowSlot::position`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WhiteRowSlot {
    /// «Auto»: следить за белым телефона (WB44).
    Auto,
    /// «Phone»: взять белое телефона один раз.
    Phone,
    /// Кнопка света по номеру в конфиге (с нуля).
    Light(usize),
}

/// Сколько кнопок-источников стоит в ряду до кнопок света («Auto» и «Phone»). Выводится из порядка мест:
/// первая кнопка света стоит сразу за источниками.
pub const SOURCE_BUTTON_COUNT: usize = WhiteRowSlot::Light(0).position();

impl WhiteRowSlot {
    /// Номер места слева направо (с нуля).
    pub const fn position(self) -> usize {
        match self {
            WhiteRowSlot::Auto => 0,
            WhiteRowSlot::Phone => 1,
            // Кнопки света идут сразу за «Auto» и «Phone». Это единственное место, где «2» написано числом:
            // `SOURCE_BUTTON_COUNT` берёт его отсюда же, а не хранит второй копией.
            WhiteRowSlot::Light(index) => 2 + index,
        }
    }

    /// Сколько кнопок в ряду при `light_count` кнопках света.
    pub const fn row_len(light_count: usize) -> usize {
        SOURCE_BUTTON_COUNT + light_count
    }

    /// Все кнопки ряда слева направо: «Auto», «Phone», свет.
    pub fn row(light_count: usize) -> impl Iterator<Item = WhiteRowSlot> {
        [WhiteRowSlot::Auto, WhiteRowSlot::Phone]
            .into_iter()
            .chain((0..light_count).map(WhiteRowSlot::Light))
    }
}

/// Состав ряда карточки белого: полный (PHOTO) или без «Auto» (раздел калибровки, решение 294). Единственный владелец
/// ответа «где стоит кнопка в этом ряду»: ряд без «Auto» центрируется заново, а не получает дыру слева.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteRowLayout {
    /// «Auto», «Phone», свет (карточка PHOTO).
    WithAuto,
    /// «Phone», свет: слежение в разделе не включают.
    WithoutAuto,
}

impl WhiteRowLayout {
    /// Кнопки ряда слева направо при `light_count` кнопках света.
    pub fn slots(self, light_count: usize) -> impl Iterator<Item = WhiteRowSlot> {
        WhiteRowSlot::row(light_count).filter(move |slot| self.position_of(*slot).is_some())
    }

    /// Сколько кнопок в ряду.
    pub fn len(self, light_count: usize) -> usize {
        self.slots(light_count).count()
    }

    /// Номер места кнопки слева направо; `None` — такой кнопки в этом составе нет.
    pub fn position_of(self, slot: WhiteRowSlot) -> Option<usize> {
        match (self, slot) {
            (WhiteRowLayout::WithAuto, slot) => Some(slot.position()),
            (WhiteRowLayout::WithoutAuto, WhiteRowSlot::Auto) => None,
            // Без «Auto» все места сдвинуты на одно влево.
            (WhiteRowLayout::WithoutAuto, slot) => Some(slot.position() - 1),
        }
    }
}

#[cfg(test)]
#[path = "white_row_tests.rs"]
mod white_row_tests;
