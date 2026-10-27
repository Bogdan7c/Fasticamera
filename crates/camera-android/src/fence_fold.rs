//! Слияние fence-ов освобождения кадра в один перед `AImage_deleteAsync` (нейтральная часть).
//!
//! Кадр читают несколько потребителей (видоискатель, кольцо снимков, запись), у каждого свой
//! fence «GPU закончил читать». Камере нужен **один**: `AImage_deleteAsync(image, fd)`. Fence-ы
//! сливаются попарно (`sync_merge`): на каждом шаге из накопленного и очередного получается новый,
//! а оба входных закрываются.
//!
//! # Инварианты
//! * Каждый дескриптор закрывается ровно один раз: либо планировщиком после слияния, либо уходит
//!   как итоговый fence новому владельцу (NDK). Закрытие — это `Drop` владеющего типа (`OwnedFd`),
//!   поэтому двойное закрытие невозможно по построению.
//! * Если слияние не удалось, итогового fence нет, и буфер **нельзя** отдавать камере: GPU мог
//!   ещё читать его. Вызывающий обязан оставить кадр (утечка со счётчиком, решение 43). Все
//!   оставшиеся fence-ы при этом закрываются — они уже никому не нужны.
//!
//! Системный вызов спрятан за [`FenceMerger`], поэтому логика проверяется на ПК с поддельным
//! слиянием и настоящими дескрипторами (`UnixStream::pair`).

/// Слияние двух fence-ов в третий (на телефоне — `sync_merge`).
pub trait FenceMerger {
    /// Владеющий тип fence-а: его `Drop` закрывает дескриптор.
    type Fence;
    /// Причина отказа слияния (код `sync_merge`).
    type Error;

    /// Новый fence, сигналящий, когда сигнальны оба входных. Входные остаются валидными:
    /// закрывает их вызывающий (то есть [`fold_fences`]).
    fn merge(&self, first: &Self::Fence, second: &Self::Fence) -> Result<Self::Fence, Self::Error>;
}

/// Итог слияния списка fence-ов.
#[derive(Debug, PartialEq, Eq)]
pub enum FoldOutcome<Fence, Error> {
    /// Ждать нечего: буфер можно вернуть камере сразу (`AImage_deleteAsync(image, -1)`).
    NothingToWait,
    /// Один fence, сигнальный когда сигнальны все исходные.
    Merged(Fence),
    /// Слияние не удалось на шаге `failed_merge` (1 — первое слияние). Итогового fence нет,
    /// все исходные и промежуточные уже закрыты.
    Failed { failed_merge: usize, error: Error },
}

/// Сливает все fence-ы в один. Порядок слияния — слева направо.
pub fn fold_fences<Merger: FenceMerger>(
    merger: &Merger,
    fences: Vec<Merger::Fence>,
) -> FoldOutcome<Merger::Fence, Merger::Error> {
    let mut remaining = fences.into_iter();
    let Some(mut accumulated) = remaining.next() else {
        return FoldOutcome::NothingToWait;
    };
    for (index, next) in remaining.enumerate() {
        match merger.merge(&accumulated, &next) {
            // Старый накопленный и очередной закрываются здесь, при выходе из итерации.
            Ok(merged) => accumulated = merged,
            // `accumulated`, `next` и ещё не взятые fence-ы закрываются при выходе из функции.
            Err(error) => {
                return FoldOutcome::Failed {
                    failed_merge: index + 1,
                    error,
                };
            }
        }
    }
    FoldOutcome::Merged(accumulated)
}

#[cfg(test)]
#[path = "fence_fold_tests.rs"]
mod tests;
