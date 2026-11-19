//! Упаковка атласа полками: без наложений, в пределах ширины, детерминированно.

// Это тест: вспомогательные функции падают через expect/panic, как и сами проверки.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use support::pack::{CellSize, pack_shelves};

fn sample_cells() -> Vec<CellSize> {
    (0..120)
        .map(|index| CellSize {
            width: 20 + (index * 7) % 45,
            height: 30 + (index * 13) % 50,
        })
        .collect()
}

#[test]
fn cells_never_overlap_and_stay_inside_the_atlas() {
    let cells = sample_cells();
    let gutter = 1;
    let packing = pack_shelves(&cells, 512, gutter).expect("упаковка");
    for (index, (cell, place)) in cells.iter().zip(&packing.placements).enumerate() {
        assert!(place.x + cell.width <= 512, "ячейка {index} вылезла за ширину");
        assert!(
            place.y + cell.height <= packing.atlas_height,
            "ячейка {index} вылезла за высоту"
        );
        for (other_index, (other, other_place)) in cells.iter().zip(&packing.placements).enumerate().skip(index + 1) {
            // Прямоугольники с зазором не пересекаются, если разделены хотя бы по одной оси.
            let separated = place.x + cell.width + gutter <= other_place.x
                || other_place.x + other.width + gutter <= place.x
                || place.y + cell.height + gutter <= other_place.y
                || other_place.y + other.height + gutter <= place.y;
            assert!(separated, "ячейки {index} и {other_index} пересекаются");
        }
    }
}

#[test]
fn packing_is_deterministic_and_rejects_cells_wider_than_the_atlas() {
    let cells = sample_cells();
    assert_eq!(pack_shelves(&cells, 512, 1), pack_shelves(&cells, 512, 1));
    let too_wide = [CellSize { width: 600, height: 10 }];
    assert!(pack_shelves(&too_wide, 512, 1).is_err());
}
