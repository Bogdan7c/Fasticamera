//! Состояние экрана панели белого. Значения белого здесь НЕТ: его держит движок, экран читает факты
//! ([`crate::white_facts::WhiteFacts`]). Здесь только то, что принадлежит экрану: ось шкалы и учёт исходов
//! команды «взять» (чтобы отказ тряхнул кнопку и дал вибрацию ровно один раз).

use crate::white_facts::{WhiteTakeFacts, WhiteTakeState};

/// Какая шкала открыта на карточке (одна шкала с переключателем, WB-вопрос 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WhiteAxis {
    /// Шкала Кельвинов.
    #[default]
    Temperature,
    /// Шкала оттенка.
    Tint,
}

/// Что экран решил по очередным фактам об исходе команды «взять».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TakeVerdict {
    /// Ничего нового или исход молчаливый (взято, отменено, ещё ждём, увиденное раньше).
    Quiet,
    /// Новый отказ: кнопка трясётся, играет вибрация «ошибка».
    NewRefusal,
}

/// Состояние панели белого на экране.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WhiteScreenState {
    axis: WhiteAxis,
    /// Номер последнего исхода (не «ждём»), который экран уже разобрал: повторная доставка тех же фактов
    /// отказ не повторяет. Сравнение — по РАВЕНСТВУ, а не «больше»: номера команд начинаются заново у нового
    /// движка (перезапуск камеры), и отказ с меньшим новым номером — настоящий отказ. `Waiting` сюда не пишется
    /// (быстрый ответ после увиденного «ждём» не потерять), а наоборот СТИРАЕТ запись: новая заявка начинает
    /// счёт заново, и отказ с номером, случайно равным старому, тоже виден.
    last_settled_serial: Option<u64>,
    /// Приходили ли уже факты. Исход, лежавший в самых первых фактах, принимается молча как увиденный
    /// (экран пересоздали, а не пользователь только что получил отказ).
    facts_seen: bool,
    /// Монотонный номер отказов: виджет запускает тряску, когда номер вырос.
    refusal_serial: u64,
}

impl WhiteScreenState {
    /// Выбранная шкала.
    pub fn axis(&self) -> WhiteAxis {
        self.axis
    }

    /// Номер отказа: растёт на 1 с каждым новым отказом (по факту или недоставленной команде).
    pub fn refusal_serial(&self) -> u64 {
        self.refusal_serial
    }

    pub(crate) fn select_axis(&mut self, axis: WhiteAxis) {
        self.axis = axis;
    }

    /// Карточка открывается заново: всегда с шкалы Кельвинов.
    pub(crate) fn reset_axis(&mut self) {
        self.axis = WhiteAxis::Temperature;
    }

    /// Команда «взять» не дошла до движка (очередь полна, движка нет): это тоже отказ для человека.
    pub(crate) fn register_undelivered_take(&mut self) {
        self.refusal_serial += 1;
    }

    /// Разбирает исход команды «взять» из очередных фактов.
    pub(crate) fn observe_take(&mut self, take: Option<WhiteTakeFacts>) -> TakeVerdict {
        let first_facts = !self.facts_seen;
        self.facts_seen = true;
        let Some(take) = take else {
            // Своей команды нет (новый движок ещё ничего не делал): его номера начнутся заново, и прежний
            // разобранный номер к ним не относится.
            self.last_settled_serial = None;
            return TakeVerdict::Quiet;
        };
        if take.state == WhiteTakeState::Waiting {
            // Идёт новая заявка: прежний разобранный исход к ней не относится.
            self.last_settled_serial = None;
            return TakeVerdict::Quiet;
        }
        if self.last_settled_serial == Some(take.serial) {
            return TakeVerdict::Quiet;
        }
        self.last_settled_serial = Some(take.serial);
        if first_facts || take.state != WhiteTakeState::Refused {
            return TakeVerdict::Quiet;
        }
        self.refusal_serial += 1;
        TakeVerdict::NewRefusal
    }
}
