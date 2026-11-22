//! Картинки UI (сессия 14g): паритет GPU ↔ эталон `shade_image_pixel`, загрузка один раз (кэш), порядок
//! рисования рядом с фигурами, пределы, вытеснение с отложенным освобождением памяти, пересоздание рендера.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::mirror::worst_difference;
use support::pictures::{
    flat_picture, gradient_picture, image_item, record_keeping_slot, record_without_submit, render_with_images,
};
use support::{CLEAR, Rig, assert_no_validation_errors, empty_list, gpu_context_or_skip, pixel, shape, shape_item};
use ui_draw::color::{DisplayColor, DisplayPrimaries};
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::PxSize;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_render_vulkan::{BackdropInput, LimitKind, UiRenderError};

const ABSENT: BackdropInput<'static> = BackdropInput::Absent {
    fill: DisplayColor::BLACK,
};

/// Допуск паритета картинок: 2/255 на канал (измерено на RADV: sRGB 0,0034 ≈ 0,9/255, P3 0,0038 ≈ 1/255).
/// Источники расхождения: сэмплер GPU считает веса билинейной
/// фильтрации в фиксированной точке (8 бит дробной части: до 1/512 шага между текселями, у наших картинок шаг
/// ≤ 0,25 → ≲ 0,001), цель Rgba8 округляет запись (±0,5/255) и смешивание (ещё ±0,5/255), `pow` кривой sRGB
/// у GPU и CPU различается в последних битах f32, а в P3 кривая стоит дважды и у тёмных значений усиливает
/// ошибку до ×12,92. Всё вместе — около 1/255; формула, порядок «выборка → кривая → матрица» или
/// неверный номер текстуры дают на порядок больше.
const IMAGE_TOLERANCE: f32 = 2.0 / 255.0;

fn rig_with(context: &gpu_vulkan::context::GpuContext, primaries: DisplayPrimaries) -> Rig {
    Rig::with_config(
        context,
        (80, 60),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
        |config| config.display_primaries = primaries,
    )
}

/// Пиксель равен цвету очистки с точностью записи в 8 бит.
fn is_clear(pixel: &[f32; 4]) -> bool {
    (0..3).all(|channel| (pixel[channel] - CLEAR[channel]).abs() < 0.005)
}

fn image_list(items: Vec<DrawItem>) -> ui_draw::draw_list::DrawList {
    let mut list = empty_list();
    list.items = items;
    list
}

/// Повёрнутая на 90° рамка 40×30 с центром в (40, 30), прозрачность 0,5: кадрирование Cover по ширине
/// становится вертикальным на экране.
fn rotated_translucent_image(picture: &ui_draw::picture::UiPicture) -> DrawItem {
    let transform = Affine2::rotate(std::f32::consts::FRAC_PI_2).then(Affine2::translate(40.0, 30.0));
    DrawItem::Image(image_item(picture, (40.0, 30.0), transform, 0.5))
}

#[test]
fn gpu_image_matches_the_cpu_reference_for_srgb_and_display_p3() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let picture = gradient_picture(12, 8);
    let list = image_list(vec![rotated_translucent_image(&picture)]);
    let mut renders = Vec::new();
    for primaries in [DisplayPrimaries::Srgb, DisplayPrimaries::DisplayP3] {
        let mut rig = rig_with(&context, primaries);
        let (report, pixels) = rig.render(&list, ABSENT);
        assert_eq!(
            (report.image_instances, report.picture_uploads, report.draw_calls),
            (1, 1, 1)
        );
        let expected = render_with_images(&list, 80, 60, CLEAR, primaries);
        let (worst, at) = worst_difference(&pixels, &expected, 80);
        assert!(
            worst <= IMAGE_TOLERANCE,
            "{primaries:?}: наибольшее расхождение {worst} в пикселе {at:?}, допуск {IMAGE_TOLERANCE}"
        );
        // Страховка от «сравнили пустое с пустым»: картинка действительно нарисована в центре и обрезана кругом.
        let centre = pixel(&pixels, 80, 40, 30);
        assert!(
            (centre[0] - CLEAR[0]).abs() > 0.05 || (centre[1] - CLEAR[1]).abs() > 0.05,
            "{centre:?}"
        );
        assert!(is_clear(&pixel(&pixels, 80, 2, 2)), "вне круга цель не тронута");
        renders.push(pixels);
        drop(rig);
    }
    // P3 обязан отличаться от sRGB на насыщенных пикселях: иначе матрицу можно было бы удалить, а тест остался бы зелёным.
    let (difference, _) = worst_difference(&renders[0], &renders[1], 80);
    assert!(difference > 0.02, "P3 и sRGB почти не отличаются: {difference}");
    drop(picture);
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn a_picture_is_uploaded_once_and_the_next_frame_is_a_cache_hit() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = rig_with(&context, DisplayPrimaries::Srgb);
    let picture = gradient_picture(12, 8);
    let list = image_list(vec![DrawItem::Image(image_item(
        &picture,
        (30.0, 30.0),
        Affine2::translate(40.0, 30.0),
        1.0,
    ))]);
    let (first, first_pixels) = rig.render(&list, ABSENT);
    let (second, second_pixels) = rig.render(&list, ABSENT);
    assert_eq!((first.picture_uploads, first.picture_cache_hits), (1, 0));
    assert_eq!((second.picture_uploads, second.picture_cache_hits), (0, 1));
    assert_eq!(
        first_pixels, second_pixels,
        "кадр из кэша выглядит так же, как кадр с загрузкой"
    );
    let counters = rig.renderer.counters();
    assert_eq!(
        (
            counters.picture_uploads,
            counters.picture_cache_hits,
            counters.images_drawn
        ),
        (1, 1, 2)
    );
    assert_eq!(rig.renderer.cached_picture_count(), 1);

    // Та же картинка дважды в одном кадре: один сэмплер, одна загрузка (в кэше уже есть — попадание).
    let twice = image_list(vec![
        DrawItem::Image(image_item(&picture, (20.0, 20.0), Affine2::translate(20.0, 30.0), 1.0)),
        DrawItem::Image(image_item(&picture, (20.0, 20.0), Affine2::translate(60.0, 30.0), 1.0)),
    ]);
    let (report, pixels) = rig.render(&twice, ABSENT);
    assert_eq!(
        (
            report.image_instances,
            report.picture_uploads,
            report.picture_cache_hits
        ),
        (2, 0, 1)
    );
    let expected = render_with_images(&twice, 80, 60, CLEAR, DisplayPrimaries::Srgb);
    assert!(worst_difference(&pixels, &expected, 80).0 <= IMAGE_TOLERANCE);
    drop((rig, list, twice, picture));
    assert_no_validation_errors(&context);
}

/// Ревью 14g: кадр с загрузкой картинки записан успешно, но его буфер не отправлен (сбой сабмита). Копия в
/// текстуру не выполнилась — кэш обязан её забыть, иначе следующий кадр взял бы «попадание в кэш» и рисовал
/// текстуру в раскладке `UNDEFINED` (мусор и ошибка validation), пока картинка не сменится.
#[test]
fn a_picture_whose_upload_frame_was_not_submitted_is_uploaded_again() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = rig_with(&context, DisplayPrimaries::Srgb);
    let picture = gradient_picture(12, 8);
    let list = image_list(vec![DrawItem::Image(image_item(
        &picture,
        (30.0, 30.0),
        Affine2::translate(40.0, 30.0),
        1.0,
    ))]);
    let recorded = record_without_submit(&mut rig, 0, &list).unwrap();
    assert_eq!(
        recorded.picture_uploads, 1,
        "копия картинки записана в неотправленный кадр"
    );
    // Сессия сообщает об отказе отправки, затем кольцо снова выдаёт слот (обычное освобождение).
    rig.renderer.frame_not_submitted(0).unwrap();
    rig.renderer.release_slot_after_fence(0).unwrap();
    assert_eq!(rig.renderer.counters().pictures_discarded_unsubmitted, 1);
    assert_eq!(
        rig.renderer.cached_picture_count(),
        0,
        "незагруженная текстура не осталась в кэше"
    );

    let (report, pixels) = rig.render(&list, ABSENT);
    assert_eq!(
        (report.picture_uploads, report.picture_cache_hits),
        (1, 0),
        "картинка загружается заново, а не берётся из кэша"
    );
    let expected = render_with_images(&list, 80, 60, CLEAR, DisplayPrimaries::Srgb);
    let (worst, _) = worst_difference(&pixels, &expected, 80);
    assert!(
        worst <= IMAGE_TOLERANCE,
        "кадр после повторной загрузки совпадает с эталоном: {worst}"
    );
    drop((rig, list, picture));
    assert_no_validation_errors(&context);
}

#[test]
fn two_pictures_interleaved_with_a_shape_keep_list_order_and_their_own_textures() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut rig = rig_with(&context, DisplayPrimaries::DisplayP3);
    let red = flat_picture(4, 4, [255, 0, 0, 255]);
    let blue = flat_picture(6, 3, [0, 0, 255, 255]);
    // Порядок: красная слева, белая фигура поверх её правого края, синяя поверх фигуры — вправо.
    let list = image_list(vec![
        DrawItem::Image(image_item(&red, (36.0, 36.0), Affine2::translate(24.0, 30.0), 1.0)),
        shape(shape_item(
            ShapeGeometry::Circle { diameter: 30.0 },
            Affine2::translate(40.0, 30.0),
            DisplayColor::WHITE,
        )),
        DrawItem::Image(image_item(&blue, (36.0, 36.0), Affine2::translate(56.0, 30.0), 1.0)),
    ]);
    let (report, pixels) = rig.render(&list, ABSENT);
    assert_eq!(
        (
            report.image_instances,
            report.shape_instances,
            report.draw_calls,
            report.picture_uploads
        ),
        (2, 1, 3, 2),
        "картинка, фигура, картинка — три вызова рисования; две разные картинки загружены"
    );
    let expected = render_with_images(&list, 80, 60, CLEAR, DisplayPrimaries::DisplayP3);
    let (worst, at) = worst_difference(&pixels, &expected, 80);
    assert!(worst <= IMAGE_TOLERANCE, "расхождение {worst} в {at:?}");
    // Цвет на каждом участке — «чей» он: слева красный, справа синий, в перекрытии (x = 40) синяя поверх белой.
    let (left, right) = (pixel(&pixels, 80, 14, 30), pixel(&pixels, 80, 66, 30));
    assert!(left[0] > 0.6 && left[2] < 0.3, "{left:?}");
    assert!(right[2] > 0.6 && right[0] < 0.3, "{right:?}");
    assert_eq!(rig.renderer.cached_picture_count(), 2);
    drop((rig, list, red, blue));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn too_many_images_is_an_honest_error_that_records_nothing_and_creates_nothing() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = rig_with(&context, DisplayPrimaries::Srgb);
    let picture = gradient_picture(12, 8);
    let items = (0..3)
        .map(|index| {
            DrawItem::Image(image_item(
                &picture,
                (10.0, 10.0),
                Affine2::translate(15.0 + 25.0 * index as f32, 30.0),
                1.0,
            ))
        })
        .collect();
    let list = image_list(items);
    let objects_before = context.live_objects();
    let (result, pixels) = rig.render_with_clear(&list, ABSENT, CLEAR);
    assert!(
        matches!(
            result,
            Err(UiRenderError::LimitExceeded {
                kind: LimitKind::Images,
                requested: 3,
                limit: 2
            })
        ),
        "{result:?}"
    );
    assert!(pixels.iter().all(is_clear), "цель не тронута");
    let counters = rig.renderer.counters();
    assert_eq!(
        (
            counters.limit_overflows,
            counters.picture_uploads,
            counters.images_drawn
        ),
        (1, 0, 0)
    );
    assert_eq!(rig.renderer.cached_picture_count(), 0);
    assert_eq!(
        context.live_objects(),
        objects_before,
        "память под картинку не выделялась"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn an_oversized_picture_is_rejected_by_side_with_a_counter_before_any_allocation() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::with_config(
        &context,
        (80, 60),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
        |config| config.limits.max_image_side_px = 16,
    );
    let wide = flat_picture(17, 2, [10, 20, 30, 255]);
    let fits = flat_picture(16, 2, [10, 20, 30, 255]);
    let objects_before = context.live_objects();
    let rejected = image_list(vec![DrawItem::Image(image_item(
        &wide,
        (20.0, 20.0),
        Affine2::translate(40.0, 30.0),
        1.0,
    ))]);
    let (result, pixels) = rig.render_with_clear(&rejected, ABSENT, CLEAR);
    match result {
        Err(UiRenderError::PictureTooLarge { version, size, limit }) => {
            assert_eq!(version, wide.version());
            assert_eq!((size.width, size.height, limit), (17, 2, 16));
        }
        other => panic!("ожидали PictureTooLarge, получили {other:?}"),
    }
    assert!(pixels.iter().all(is_clear), "цель не тронута");
    assert_eq!(rig.renderer.counters().pictures_rejected, 1);
    assert_eq!(context.live_objects(), objects_before);

    let accepted = image_list(vec![DrawItem::Image(image_item(
        &fits,
        (20.0, 20.0),
        Affine2::translate(40.0, 30.0),
        1.0,
    ))]);
    let (report, _) = rig.render(&accepted, ABSENT);
    assert_eq!(report.picture_uploads, 1, "ровно на пределе — принимается");
    drop((rig, rejected, accepted, wide, fits));
    assert_no_validation_errors(&context);
}

#[test]
fn an_idle_picture_is_evicted_and_its_memory_waits_for_every_slot_that_could_read_it() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::with_config(
        &context,
        (80, 60),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
        |config| config.pictures.idle_frames_before_eviction = 1,
    );
    let picture = gradient_picture(12, 8);
    let with_picture = image_list(vec![DrawItem::Image(image_item(
        &picture,
        (30.0, 30.0),
        Affine2::translate(40.0, 30.0),
        1.0,
    ))]);
    let empty = empty_list();
    let renderer_only = context.live_objects().total();

    // Кадр 0 (слот 0) загружает картинку: появляются текстура и staging.
    let (result, _) = record_keeping_slot(&mut rig, 0, &with_picture);
    assert_eq!(result.unwrap().picture_uploads, 1);
    let after_upload = context.live_objects().total();
    assert!(after_upload > renderer_only, "текстура и staging созданы");
    // Fence слота 0 пройден: staging больше не нужен, текстура остаётся (картинка в кэше).
    rig.renderer.release_slot_after_fence(0).unwrap();
    let texture_only = context.live_objects().total();
    assert!(
        texture_only > renderer_only && texture_only < after_upload,
        "staging освобождён, текстура жива"
    );

    // Кадр 1 (слот 1) и кадр 2 (слот 0) без картинки: на кадре 2 она простаивает дольше предела и вытесняется.
    record_keeping_slot(&mut rig, 1, &empty).0.unwrap();
    assert_eq!(rig.renderer.cached_picture_count(), 1, "пока не вытеснена");
    record_keeping_slot(&mut rig, 0, &empty).0.unwrap();
    assert_eq!(rig.renderer.cached_picture_count(), 0, "вытеснена из кэша");
    assert_eq!(rig.renderer.counters().picture_evictions, 1);
    assert_eq!(
        context.live_objects().total(),
        texture_only,
        "слот 1 ещё в полёте и мог читать текстуру: память не освобождена"
    );
    rig.renderer.release_slot_after_fence(0).unwrap();
    assert_eq!(
        context.live_objects().total(),
        texture_only,
        "слот 1 всё ещё не отпущен"
    );
    rig.renderer.release_slot_after_fence(1).unwrap();
    assert_eq!(
        context.live_objects().total(),
        renderer_only,
        "после всех слотов память отдана"
    );

    // Картинка снова нужна: загружается заново из памяти процесса.
    let (result, _) = record_keeping_slot(&mut rig, 0, &with_picture);
    let report = result.unwrap();
    assert_eq!((report.picture_uploads, report.picture_cache_hits), (1, 0));
    rig.renderer.release_slot_after_fence(0).unwrap();
    drop((rig, with_picture, picture));
    assert_no_validation_errors(&context);
}

#[test]
fn a_recreated_renderer_uploads_the_picture_again_from_memory_and_draws_the_same_pixels() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let picture = gradient_picture(12, 8);
    let list = image_list(vec![rotated_translucent_image(&picture)]);
    let mut first = rig_with(&context, DisplayPrimaries::DisplayP3);
    let (first_report, first_pixels) = first.render(&list, ABSENT);
    assert_eq!(first_report.picture_uploads, 1);
    drop(first);
    assert_eq!(context.live_objects(), baseline, "старый рендер унёс свои текстуры");

    let mut second = rig_with(&context, DisplayPrimaries::DisplayP3);
    let (second_report, second_pixels) = second.render(&list, ABSENT);
    assert_eq!(
        (second_report.picture_uploads, second_report.picture_cache_hits),
        (1, 0),
        "новый рендер не знает прежнюю текстуру и загружает картинку заново"
    );
    assert_eq!(first_pixels, second_pixels);
    drop((second, list, picture));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn image_items_do_not_disturb_neighbouring_pipelines_state() {
    // Картинка после текста и фигуры: счётчики видов независимы, а размер плана не «съедает» чужие пределы.
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = rig_with(&context, DisplayPrimaries::Srgb);
    let picture = flat_picture(2, 2, [255, 255, 255, 255]);
    let list = image_list(vec![
        shape(shape_item(
            ShapeGeometry::Capsule {
                size: PxSize::new(30.0, 12.0),
            },
            Affine2::translate(20.0, 12.0),
            DisplayColor::rgb(0.9, 0.2, 0.2),
        )),
        DrawItem::Image(image_item(&picture, (20.0, 20.0), Affine2::translate(55.0, 35.0), 1.0)),
    ]);
    let (report, pixels) = rig.render(&list, ABSENT);
    assert_eq!(
        (report.shape_instances, report.image_instances, report.glyph_instances),
        (1, 1, 0)
    );
    let expected = render_with_images(&list, 80, 60, CLEAR, DisplayPrimaries::Srgb);
    assert!(worst_difference(&pixels, &expected, 80).0 <= IMAGE_TOLERANCE);
    assert!(pixel(&pixels, 80, 55, 35)[0] > 0.95, "белая картинка в центре");
    drop((rig, list, picture));
    assert_no_validation_errors(&context);
}
