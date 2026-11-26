//! Раздел калибровки: ручки, основа тона, приборы, «чистяк», выбор полосы и меню «шесть точек» — карточка пресетов (решения 185–203).
//!
//! Инварианты модуля:
//! * все действия ручек допустимы только в режиме с разделом калибровки (`ModeBehaviour::calibration`);
//! * значения ручек берутся из `LookFacts::current` — экран своих значений не держит (решение 178);
//! * запрос прибора (`Effect::Scopes`) выдаёт не каждый обработчик, а одна сверка после любого действия
//!   (`sync_after_action`): рендеру уходит только РАЗНИЦА между тем, что нужно, и тем, что уже заказано. Так
//!   смена прибора, оси, поворот, смена размера surface и выход из режима подчиняются одному правилу;
//! * «чистяк» не залипает: выход из режима отпускает его.

use look_contract::{BandAxis, CleanView, GlobalKnob, HueBandId, LookControl, LookIntent, LookKnob, ToneBaseModel};
use scope_contract::{ScopeIntent, ScopeKind, VectorscopeAxes};

use super::Ctx;
use super::look::{Availability, emit_look_from_facts, read_facts};
use crate::calibration::{CleanViewState, ControlViewKind, SectionCard};
use crate::effect::Effect;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};

/// Глобальная ручка потянута: точечное намерение `SetKnob` — только эта ручка и её значение.
///
/// Факты нужны лишь для проверки (значение отличается от текущего). Полный набор значений из фактов
/// НЕ собирается: два жеста на разных ручках в одном кадре иначе несли бы оба набор из устаревших фактов, и второй
/// затёр бы первый (найдено ревью 14b). Применяет намерение библиотека к своим текущим значениям.
pub(super) fn on_knob_dragged(ctx: &mut Ctx<'_>, knob: GlobalKnob, value: LookKnob) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        if facts.current.settings.knob(knob) == value {
            return Err(ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
        }
        Ok(LookIntent::SetKnob { knob, value })
    })
}

/// Ось полосы потянута: точечное намерение `SetBandAxis` (соседние оси и полосы не трогаются; причина — как у
/// [`on_knob_dragged`]).
pub(super) fn on_band_knob_dragged(
    ctx: &mut Ctx<'_>,
    band: HueBandId,
    axis: BandAxis,
    value: LookKnob,
) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        if !facts.bands.iter().any(|known| known.id == band) {
            return Err(ActionOutcome::Rejected(Rejection::UnknownBand));
        }
        let current_adjustment = facts.current.settings.bands.for_band(&band);
        if current_adjustment.axis(axis) == value {
            return Err(ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
        }
        Ok(LookIntent::SetBandAxis { band, axis, value })
    })
}

/// Модель основы выбрана свайпом.
pub(super) fn on_tone_base_selected(ctx: &mut Ctx<'_>, base: ToneBaseModel) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        if facts.current.base == base {
            return Err(ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
        }
        Ok(LookIntent::SetToneBase(base))
    })
}

/// «↻» у органа управления: сброс к значению пресета только если оно отличается (тусклая «↻» — `NoChange`).
pub(super) fn on_reset_control(ctx: &mut Ctx<'_>, control: LookControl) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        if !facts.control_differs(&control) {
            return Err(ActionOutcome::NoChange(NoChangeReason::ControlMatchesPreset));
        }
        Ok(LookIntent::ResetControl(control))
    })
}

/// Проверка режима для действий, которые не читают факты (выбор прибора, «чистяк», меню).
pub(super) fn require_calibration(ctx: &Ctx<'_>) -> Result<(), ActionOutcome> {
    if ctx.state.mode_behaviour().has_calibration_surface() {
        Ok(())
    } else {
        Err(ActionOutcome::Rejected(Rejection::NotInCalibrationMode))
    }
}

/// Прибор выбран свайпом. Запрос рендеру уйдёт сверкой `sync_after_action`.
pub(super) fn on_scope_kind_selected(ctx: &mut Ctx<'_>, kind: ScopeKind) -> ActionOutcome {
    if let Err(outcome) = require_calibration(ctx) {
        return outcome;
    }
    ctx.state.calibration.set_scope_kind(kind);
    ActionOutcome::Applied
}

/// Стандарт осей вектороскопа выбран в полоске прибора (решение 258).
pub(super) fn on_axes_selected(ctx: &mut Ctx<'_>, axes: VectorscopeAxes) -> ActionOutcome {
    if let Err(outcome) = require_calibration(ctx) {
        return outcome;
    }
    ctx.state.calibration.set_vectorscope_axes(axes);
    ActionOutcome::Applied
}

/// «Чистяк» нажат: показать основу. Повторное нажатие без отпускания — `NoChange` (идемпотентно).
pub(super) fn on_clean_view_pressed(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if let Err(outcome) = require_calibration(ctx) {
        return outcome;
    }
    if ctx.state.calibration.clean_view() == CleanViewState::HoldingBase {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    ctx.state.calibration.set_clean_view(CleanViewState::HoldingBase);
    ctx.emit(Effect::CleanView(CleanView::ShowBase));
    ActionOutcome::Applied
}

/// «Чистяк» отпущен: вернуть правки. Отпускание без нажатия — `NoChange`.
pub(super) fn on_clean_view_released(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if let Err(outcome) = require_calibration(ctx) {
        return outcome;
    }
    if release_clean_view(ctx) {
        ActionOutcome::Applied
    } else {
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    }
}

/// Отпускает «чистяк», если он держался (выдаёт `ShowEdited`). `true` — держался.
pub(super) fn release_clean_view(ctx: &mut Ctx<'_>) -> bool {
    if ctx.state.calibration.clean_view() == CleanViewState::ShowingEdited {
        return false;
    }
    ctx.state.calibration.set_clean_view(CleanViewState::ShowingEdited);
    ctx.emit(Effect::CleanView(CleanView::ShowEdited));
    true
}

/// Выбрана полоса оттенка.
pub(super) fn on_band_selected(ctx: &mut Ctx<'_>, band: HueBandId) -> ActionOutcome {
    let known = read_facts(ctx, Availability::CalibrationOnly, |facts| {
        Ok(facts.bands.iter().any(|known| known.id == band))
    });
    match known {
        Err(outcome) => outcome,
        Ok(false) => ActionOutcome::Rejected(Rejection::UnknownBand),
        Ok(true) if ctx.state.calibration.selected_band() == Some(&band) => {
            ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
        }
        Ok(true) => {
            ctx.state.calibration.set_selected_band(Some(band));
            ActionOutcome::Applied
        }
    }
}

/// После новых фактов выбранная полоса обязана существовать: по умолчанию первая, при пропаже — снова первая;
/// полос нет — выбора нет.
pub(super) fn reselect_band_after_facts(ctx: &mut Ctx<'_>) {
    let Some(facts) = ctx.state.look.facts() else {
        return;
    };
    let still_present = ctx
        .state
        .calibration
        .selected_band()
        .is_some_and(|selected| facts.bands.iter().any(|known| &known.id == selected));
    if still_present {
        return;
    }
    let first = facts.bands.first().map(|known| known.id.clone());
    ctx.state.calibration.set_selected_band(first);
}

// ----- Меню «шесть точек» раздела ---------------------------------------------------------------------------------

/// Кнопка меню в режиме с разделом: сразу открыть карточку пресетов (решение 283).
pub(super) fn open_menu(ctx: &mut Ctx<'_>) -> ActionOutcome {
    // Карточка белого уже закрывает кнопку меню (нижний ряд скрыт, решение 292): действие не от виджета — отказ.
    if ctx.state.calibration.card() == SectionCard::White {
        return ActionOutcome::Rejected(Rejection::PanelBlocksAction);
    }
    ctx.state.calibration.set_card(SectionCard::Presets);
    ActionOutcome::Applied
}

/// Тап по капсуле белого в полоске прибора: открывает карточку белого, повторный тап её закрывает (решения 290, 292).
/// Открывается карточка всегда с шкалы Кельвинов, как в PHOTO. Пока открыта карточка пресетов — отказ: завеса её
/// модальная, капсула лежит под ней.
pub(super) fn toggle_white_card(ctx: &mut Ctx<'_>) -> ActionOutcome {
    match ctx.state.calibration.card() {
        SectionCard::Presets => ActionOutcome::Rejected(Rejection::PanelBlocksAction),
        SectionCard::White => {
            ctx.state.calibration.set_card(SectionCard::Closed);
            ActionOutcome::Applied
        }
        SectionCard::Closed => {
            ctx.state.calibration.set_card(SectionCard::White);
            ctx.state.white.reset_axis();
            ActionOutcome::Applied
        }
    }
}

/// Закрыть меню (свайп вниз, тап по видоискателю). Закрывать нечего — `NoChange`.
pub(super) fn close_menu(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if ctx.state.calibration.card() == SectionCard::Closed {
        return ActionOutcome::NoChange(NoChangeReason::NothingToClose);
    }
    ctx.state.calibration.set_card(SectionCard::Closed);
    ActionOutcome::Applied
}

// ----- Сверка геометрии и спроса на прибор ------------------------------------------------------------------------

/// Рендер потерял спрос на прибор: забываем, что он заказан, — сверка ниже закажет заново.
pub(super) fn on_scope_demand_lost(ctx: &mut Ctx<'_>) -> ActionOutcome {
    ctx.state.calibration.set_announced_scope(None);
    ActionOutcome::Applied
}

/// Сверка после любого действия: пересчитать раскладку раздела и выдать рендеру разницу по запросу прибора.
pub(super) fn sync_after_action(ctx: &mut Ctx<'_>) {
    super::look::release_startup_problem_in_section(ctx);
    refresh_layout(ctx);
    sync_scope_demand(ctx);
}

/// Раскладка раздела строится, только пока режим её имеет и есть размер surface. Неудачная сборка считается отказом
/// `InvalidSurface` ОДИН раз на каждую новую ошибку: иначе счётчик рос бы с каждым тиком и каждое действие
/// становилось бы «изменением» состояния.
fn refresh_layout(ctx: &mut Ctx<'_>) {
    let behaviour = ctx.state.mode_behaviour();
    let built = match ctx.state.layout {
        Some(screen) if behaviour.has_calibration_surface() => Some(screen.calibration_layout(
            &ctx.config.calibration,
            behaviour.layout,
            ctx.state.orientation,
            ctx.state.safe_area,
        )),
        _ => None,
    };
    let (layout, fault) = match built {
        Some(Ok(layout)) => (Some(layout), None),
        Some(Err(fault)) => (None, Some(fault)),
        None => (None, None),
    };
    ctx.state.calibration.set_layout(layout);
    if ctx.state.calibration.note_layout_fault(fault) {
        ctx.state.rejections.record(Rejection::InvalidSurface);
    }
    // Вид колёс может не поместиться в колонку на низкой поверхности: раздел при этом живёт (шапка и переключатель
    // на месте, зона колёс пуста), а причина считается так же — один раз на смену.
    let wheels_fault = match (layout, ctx.state.pixels_per_point()) {
        (Some(layout), Some(pixels_per_point)) if ctx.state.calibration.control_view() == ControlViewKind::Wheels => {
            crate::calibration::wheel_pad_placement(&layout, &ctx.config.calibration.widgets, pixels_per_point).err()
        }
        _ => None,
    };
    if ctx.state.calibration.note_wheels_fault(wheels_fault) {
        ctx.state.rejections.record(Rejection::InvalidSurface);
    }
}

/// Нужен прибор — просим (или меняем запрос); не нужен, а заказан — скрываем. Ничего не изменилось — молчим.
fn sync_scope_demand(ctx: &mut Ctx<'_>) {
    let wanted = ctx.state.calibration.desired_scope_request();
    if wanted == ctx.state.calibration.announced_scope() {
        return;
    }
    ctx.emit(Effect::Scopes(match wanted {
        Some(request) => ScopeIntent::Show(request),
        None => ScopeIntent::Hide,
    }));
    ctx.state.calibration.set_announced_scope(wanted);
}
