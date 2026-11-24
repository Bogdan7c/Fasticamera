//! Тесты формата файла кэша пайплайнов (без GPU): что принимается и как именно отвергается каждая порча.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

use super::*;

fn sample_payload() -> Vec<u8> {
    (0..300_u32).map(|index| (index * 7 % 251) as u8).collect()
}

#[test]
fn a_sealed_file_round_trips_to_exactly_the_same_payload() {
    let payload = sample_payload();
    let file = seal_cache_file(&payload);
    assert_eq!(file.len(), FILE_HEADER_BYTES + payload.len());
    assert_eq!(unseal_cache_file(&file).unwrap(), payload.as_slice());
    // Пустые данные драйвера — тоже допустимый файл (пустой кэш).
    assert_eq!(unseal_cache_file(&seal_cache_file(&[])).unwrap(), &[] as &[u8]);
}

#[test]
fn a_file_cut_at_any_length_is_refused_and_never_reaches_the_driver() {
    let file = seal_cache_file(&sample_payload());
    // Все длины: ноль, внутри заголовка, ровно заголовок, середина, «на байт короче».
    for length in (0..file.len())
        .step_by(7)
        .chain([0, 1, FILE_HEADER_BYTES - 1, FILE_HEADER_BYTES, file.len() - 1])
    {
        let cut = &file[..length];
        let result = unseal_cache_file(cut);
        let expected = if length < FILE_HEADER_BYTES {
            FileDamage::TooShort
        } else {
            FileDamage::LengthMismatch
        };
        assert_eq!(result, Err(expected), "длина {length} из {}", file.len());
    }
}

#[test]
fn a_flipped_bit_in_any_part_of_the_body_is_caught_by_the_checksum() {
    let file = seal_cache_file(&sample_payload());
    for position in [
        FILE_HEADER_BYTES,
        FILE_HEADER_BYTES + 1,
        file.len() / 2 + 7,
        file.len() - 1,
    ] {
        for bit in [0_u8, 3, 7] {
            let mut damaged = file.clone();
            damaged[position] ^= 1 << bit;
            assert_eq!(
                unseal_cache_file(&damaged),
                Err(FileDamage::ChecksumMismatch),
                "байт {position}, бит {bit}"
            );
        }
    }
}

#[test]
fn a_foreign_magic_a_garbage_tail_and_an_old_format_version_are_told_apart() {
    let file = seal_cache_file(&sample_payload());
    let mut foreign = file.clone();
    foreign[0] ^= 0xFF;
    assert_eq!(unseal_cache_file(&foreign), Err(FileDamage::WrongMagic));

    let mut with_tail = file.clone();
    with_tail.extend_from_slice(&[0xAB; 5]);
    assert_eq!(unseal_cache_file(&with_tail), Err(FileDamage::LengthMismatch));

    let mut old = file.clone();
    old[8..12].copy_from_slice(&(FORMAT_VERSION - 1).to_le_bytes());
    assert_eq!(
        unseal_cache_file(&old),
        Err(FileDamage::WrongVersion {
            found: FORMAT_VERSION - 1
        })
    );

    // Сырые данные драйвера без нашей обёртки (формат прошлого шага I6-02) — чужой файл, а не кэш.
    assert!(matches!(
        unseal_cache_file(&sample_payload()),
        Err(FileDamage::WrongMagic)
    ));
}

#[test]
fn origins_have_distinct_short_names_for_reports() {
    let names = [
        PipelineCacheOrigin::Empty,
        PipelineCacheOrigin::Loaded { bytes: 1 },
        PipelineCacheOrigin::IgnoredByDriver { offered_bytes: 1 },
        PipelineCacheOrigin::RejectedByDriver,
        PipelineCacheOrigin::DamagedFile(FileDamage::TooShort),
        PipelineCacheOrigin::OversizedFile { bytes: 1 },
    ]
    .map(PipelineCacheOrigin::short_name);
    let mut unique = names.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len());
}
