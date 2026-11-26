//! Что экран знает о виде картинки: факты от библиотеки пресетов и плашка проблемы (решения 178, 196).
//!
//! Владелец данных о виде — библиотека пресетов приложения; экран хранит лишь **последний присланный снимок**
//! (`LookFacts`) и никогда не копит собственных значений ручек: шкалы и «↻» читают значения отсюда, а
//! намерения собираются из `facts.current`. Здесь же живёт «проблема к показу» — плашка, которую рисует виджет:
//! и проблема библиотеки (по смене серийного номера), и локальная проблема экрана (неверное имя, нет клавиатуры).

use look_contract::{GlobalKnob, KnobSideStops, KnobStop, LookFacts, LookProblemKind, LookWhiteStops, PresetNameError};

/// Какая именно проблема показана плашкой.
#[derive(Debug, Clone, PartialEq)]
pub enum ProblemKind {
    /// Проблема от библиотеки пресетов (имя занято, запись не удалась…): текст по варианту выбирает виджет.
    Look(LookProblemKind),
    /// Введённое имя пресета не прошло проверку на экране (пусто, длинное, управляющий символ).
    InvalidPresetName(PresetNameError),
    /// Системная клавиатура недоступна: ввести имя нельзя.
    KeyboardUnavailable,
}

/// Плашка проблемы. `number` растёт с каждой новой плашкой: две одинаковые подряд различимы по `==`, значит
/// виджет покажет и вторую (тот же приём, что серийный номер `LookProblem`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProblemBanner {
    /// Номер плашки на этом экране (с единицы).
    pub number: u64,
    /// Что случилось.
    pub kind: ProblemKind,
}

/// Снимок вида и плашка проблемы.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LookView {
    /// Последние факты; `None` — библиотеки пресетов нет (ручки отключены, список пресетов пуст).
    facts: Option<LookFacts>,
    /// Плашка, которую надо показать сейчас.
    banner: Option<ProblemBanner>,
    /// Сколько плашек экран уже поднимал (нумерует новые).
    banners_raised: u64,
    /// Серийный номер последней проблемы библиотеки, по которой уже поднята плашка.
    last_seen_look_problem: Option<u64>,
    /// Проблема запуска (решение 287), которая ждёт первого входа в раздел калибровки: плашка поднимается ровно
    /// один раз при входе, а не при старте, когда раздела на экране нет, и не теряется, если вход случится позже.
    startup_problem_awaiting_section: Option<LookProblemKind>,
    /// Сколько проблем запуска уже поднято плашкой (подтверждение «показано» для приложения: пока оно 0, приложение
    /// обязано держать проблему и отдавать её каждому новому экрану).
    startup_problems_shown: u64,
    /// Упоры ручек теплоты и оттенка по последнему сообщению рендера (решения 288, 293). До первого сообщения —
    /// `Unknown`: отметок на шкалах нет.
    white_stops: LookWhiteStops,
}

impl LookView {
    /// Последние факты о виде.
    pub fn facts(&self) -> Option<&LookFacts> {
        self.facts.as_ref()
    }

    /// Сколько проблем запуска уже показано плашкой. Приложение снимает проблему со своей стороны только после
    /// того, как это число стало больше нуля: экран, пересозданный до первого входа в раздел, получит её снова.
    pub fn startup_problems_shown(&self) -> u64 {
        self.startup_problems_shown
    }

    /// Плашка проблемы, которую надо показать.
    pub fn banner(&self) -> Option<&ProblemBanner> {
        self.banner.as_ref()
    }

    /// Упоры шкалы ручки `knob` для отметок. Упоры бывают только у теплоты и оттенка; у остальных ручек, а также
    /// пока упоры неизвестны или не применимы (`Unknown`, `NotApplicable`), отметок нет ([`KnobStop::None`]).
    pub fn knob_stops(&self, knob: GlobalKnob) -> KnobSideStops {
        const NO_MARKS: KnobSideStops = KnobSideStops {
            minus: KnobStop::None,
            plus: KnobStop::None,
        };
        match (self.white_stops, knob) {
            (LookWhiteStops::Known(stops), GlobalKnob::Warmth) => stops.warmth,
            (LookWhiteStops::Known(stops), GlobalKnob::Tint) => stops.tint,
            _ => NO_MARKS,
        }
    }

    /// Принимает новые упоры от рендера; `false` — они такие же, как были (состояние не менялось).
    pub(crate) fn store_white_stops(&mut self, stops: LookWhiteStops) -> bool {
        if self.white_stops == stops {
            return false;
        }
        self.white_stops = stops;
        true
    }

    /// Принимает новый снимок фактов. Проблема библиотеки с НОВЫМ серийным номером поднимает плашку; тот же
    /// номер повторно (факты пришли снова по другой причине) плашку не воскрешает.
    pub(crate) fn store_facts(&mut self, facts: LookFacts) {
        if let Some(problem) = &facts.problem
            && self.last_seen_look_problem != Some(problem.serial)
        {
            self.last_seen_look_problem = Some(problem.serial);
            self.raise(ProblemKind::Look(problem.kind.clone()));
        }
        self.facts = Some(facts);
    }

    /// Запоминает проблему запуска до первого входа в раздел калибровки. Вторая такая же (приложение прислало
    /// повторно) заменяет первую, а не копится: плашка всё равно одна.
    pub(crate) fn defer_startup_problem(&mut self, kind: LookProblemKind) {
        self.startup_problem_awaiting_section = Some(kind);
    }

    /// Поднимает отложенную проблему запуска, если экран сейчас в разделе калибровки (`section_is_open`). Поднятая
    /// проблема забывается: повторный вход плашку не повторяет. Возвращает, поднялась ли плашка.
    pub(crate) fn raise_startup_problem_if_section_is_open(&mut self, section_is_open: bool) -> bool {
        if !section_is_open {
            return false;
        }
        match self.startup_problem_awaiting_section.take() {
            Some(kind) => {
                self.raise(ProblemKind::Look(kind));
                self.startup_problems_shown += 1;
                true
            }
            None => false,
        }
    }

    /// Поднимает локальную проблему экрана (неверное имя, нет клавиатуры).
    pub(crate) fn raise_local_problem(&mut self, kind: ProblemKind) {
        self.raise(kind);
    }

    /// Убирает плашку с номером `number` (истёк показ или пользователь закрыл). `false` — такой плашки сейчас нет
    /// (её не было вовсе или её уже сменила новая): текущая плашка остаётся нетронутой.
    pub(crate) fn dismiss_banner(&mut self, number: u64) -> bool {
        if self.banner.as_ref().is_some_and(|banner| banner.number == number) {
            self.banner = None;
            true
        } else {
            false
        }
    }

    fn raise(&mut self, kind: ProblemKind) {
        self.banners_raised = self.banners_raised.saturating_add(1);
        self.banner = Some(ProblemBanner {
            number: self.banners_raised,
            kind,
        });
    }
}
