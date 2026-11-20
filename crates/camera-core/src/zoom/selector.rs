//! Выбор камеры по запрошенному коэффициенту зума с гистерезисом (чистая функция).
//!
//! Правило: объектив показывает только коэффициенты **не меньше своего** (`native_ratio`) —
//! шире, чем он видит, кадр не сделать. Поэтому:
//! * кандидат — самый длинный объектив, чей `native ≤ коэффициент` (нет такого — самый короткий);
//! * если коэффициент **ниже** текущего объектива, текущий показать его не может — переход к
//!   кандидату немедленный;
//! * плавный жест (колесо, щипок), едем вверх: переключаемся на самый длинный объектив, чей
//!   `native × (1 + switch_up_hysteresis)` уже пройден (промежуточный объектив не пропускается);
//!   пока порога нет — короткий объектив просто кропает. Колебание пальца вокруг 1× не
//!   заставляет камеры мигать;
//! * явный выбор (тап по кнопке зума, переворот): гистерезиса нет, сразу кандидат.

use camera_contract::ids::LensRole;
use camera_contract::zoom::{LensGroup, LensZoomPoint, ZoomGesture, ZoomRatio};

use super::ZoomConfig;

/// Камера для первого показа группы (текущей камеры нет): без гистерезиса, по кандидату.
/// `None` — в группе нет камер.
pub fn select_initial_lens(group: &LensGroup, ratio: ZoomRatio) -> Option<LensRole> {
    candidate_lens(group, ratio).map(|lens| lens.role)
}

/// Камера для коэффициента `ratio`, если сейчас выбрана `current`. `None` — в группе нет камер.
/// Если `current` не из этой группы (после переворота на другое направление), выбор — как у
/// [`select_initial_lens`].
///
/// `gesture` решает, нужен ли гистерезис: явный выбор ([`ZoomGesture::Discrete`]) идёт прямо к
/// кандидату, плавный ([`ZoomGesture::Continuous`]) — через гистерезис вверх.
pub fn select_lens(
    group: &LensGroup,
    ratio: ZoomRatio,
    current: LensRole,
    gesture: ZoomGesture,
    config: &ZoomConfig,
) -> Option<LensRole> {
    let candidate = candidate_lens(group, ratio)?;
    let Some(current_lens) = group.lens_for_role(current) else {
        return Some(candidate.role);
    };
    if gesture == ZoomGesture::Discrete {
        // Тап по «1» из ширика обязан дать основную камеру, а не кроп ×1,7 ширика.
        return Some(candidate.role);
    }
    let wanted = ratio.value();
    if wanted < current_lens.native_ratio.value() {
        // Текущий объектив не умеет показать такой коэффициент: переход обязателен.
        return Some(candidate.role);
    }
    // Едем вверх: берём самый длинный объектив, до порога которого коэффициент уже дошёл. Идём
    // по всем более длинным, а не только по кандидату: иначе при 3,1 с ширика (кандидат — телевик,
    // его порог 3,15) мы остались бы на ширике, хотя порог основной давно пройден.
    let switch_up = group
        .lenses
        .iter()
        .rev()
        .filter(|lens| lens.native_ratio.value() > current_lens.native_ratio.value())
        .find(|lens| wanted >= lens.native_ratio.value() * (1.0 + config.switch_up_hysteresis));
    Some(switch_up.map_or(current_lens.role, |lens| lens.role))
}

/// Самый длинный объектив с `native ≤ ratio`; если все длиннее — самый короткий.
/// Камеры группы идут по возрастанию `native_ratio` (инвариант построения каталога).
fn candidate_lens(group: &LensGroup, ratio: ZoomRatio) -> Option<&LensZoomPoint> {
    group
        .lenses
        .iter()
        .rev()
        .find(|lens| lens.native_ratio.value() <= ratio.value())
        .or_else(|| group.lenses.first())
}
