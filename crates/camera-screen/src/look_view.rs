//! Что экран знает о виде картинки: факты от библиотеки пресетов и плашка проблемы (решения 178, 196).
//!
//! Владелец данных о виде — библиотека пресетов приложения; экран хранит лишь **последний присланный снимок**
//! (`LookFacts`) и никогда не копит собственных значений ручек: шкалы и «↻» читают значения отсюда, а
//! намерения собираются из `facts.current`. Здесь же живёт «проблема к показу» — плашка, которую рисует виджет:
//! и проблема библиотеки (по смене серийного номера), и локальная проблема экрана (неверное имя, нет клавиатуры).

use look_contract::{LookFacts, LookProblemKind, PresetNameError};

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
}

impl LookView {
    /// Последние факты о виде.
    pub fn facts(&self) -> Option<&LookFacts> {
        self.facts.as_ref()
    }

    /// Плашка проблемы, которую надо показать.
    pub fn banner(&self) -> Option<&ProblemBanner> {
        self.banner.as_ref()
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
