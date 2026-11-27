//! Заявка на «какой вид сейчас на рендере» (сессия 14o): общий объект между сценарием замера и исполнителем вида.
//!
//! # Кто чем владеет
//! Вид на рендере продукта ведёт [`LookEffects`](crate::look_effects::LookEffects): он знает библиотеку пресетов и
//! режим чистого вида и шлёт рендеру `SetLook` / `SetToneBase`. Сценарию замера (`viewfinder-stage-budget`) нужно
//! ПОКАЗАТЬ на рендере другой вид (другая основа, без колёс, нейтраль), не трогая библиотеку и файл пресетов
//! владельца. Если бы сценарий просто слал команды рендеру, исполнитель вида при ближайшей синхронизации «поправил
//! бы» рендер обратно и окно замера оказалось бы не тем.
//!
//! Поэтому сценарий ЗАЯВЛЯЕТ владение видом на рендере ([`LookRenderClaim::hold`]):
//! * пока заявка держится, исполнитель вида рендеру ничего не шлёт, но продолжает вести намерения и режим показа;
//! * сценарий снимает заявку ([`LookRenderClaim::release`]) — на любом исходе, как остальные заявки на рендер;
//! * снятие увеличивает счётчик снятий; исполнитель замечает его на следующем кадре, считает показанное на рендере
//!   неизвестным и досылает то, что должно быть показано (с учётом чистого вида).
//!
//! Заявка живёт у управления рендером ([`RenderControl::look_claim`](crate::viewfinder_control::RenderControl)):
//! это один и тот же объект и у `GpuServices` сценариев, и у исполнителя эффектов экрана, поэтому проводки в
//! `shell_wiring` не нужно.
//!
//! Модуль чисто про учёт: рендеру команд он не шлёт и библиотеки не знает.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use look_contract::LookRecipe;

/// Внутреннее состояние заявки.
#[derive(Debug, Default)]
struct ClaimState {
    /// `Some` — заявка держится; внутри рецепт, к которому вернуть рендер при снятии (рецепт продукта на момент
    /// ПЕРВОЙ заявки: повторные заявки одного сценария его не затирают).
    restore_to: Option<LookRecipe>,
    /// Сколько раз заявку снимали (растёт только при снятии удерживаемой заявки).
    releases: u64,
}

/// Что видит исполнитель вида за один замок: держится ли заявка и сколько раз её снимали.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookClaimStatus {
    /// Сценарий сейчас владеет видом на рендере.
    pub held: bool,
    /// Сколько раз заявку снимали за жизнь процесса.
    pub releases: u64,
}

/// Общая заявка на вид рендера (клоны делят состояние).
#[derive(Debug, Clone, Default)]
pub struct LookRenderClaim {
    state: Arc<Mutex<ClaimState>>,
}

impl LookRenderClaim {
    fn lock(&self) -> MutexGuard<'_, ClaimState> {
        // Состояние — рецепт и счётчик: «отравленный» замок не портит их смысла.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Сценарий заявляет вид; `restore_to` — рецепт продукта, к которому вернуть рендер при снятии. Повторная заявка
    /// при уже удерживаемой сохраняет прежний `restore_to` (продукт не менялся: между строками сценария вид остаётся
    /// заявленным).
    pub fn hold(&self, restore_to: LookRecipe) {
        let mut state = self.lock();
        if state.restore_to.is_none() {
            state.restore_to = Some(restore_to);
        }
    }

    /// Снимает заявку. Возвращает рецепт, к которому надо вернуть рендер (`None` — заявка не держалась: снимать
    /// нечего, счётчик снятий не растёт).
    pub fn release(&self) -> Option<LookRecipe> {
        let mut state = self.lock();
        let restore_to = state.restore_to.take();
        if restore_to.is_some() {
            state.releases += 1;
        }
        restore_to
    }

    /// Держится ли заявка и сколько раз её снимали (одним замком).
    pub fn status(&self) -> LookClaimStatus {
        let state = self.lock();
        LookClaimStatus {
            held: state.restore_to.is_some(),
            releases: state.releases,
        }
    }

    /// Держится ли заявка.
    pub fn is_held(&self) -> bool {
        self.status().held
    }
}

#[cfg(test)]
#[path = "look_render_claim_tests.rs"]
mod tests;
