//! Цикл рендера в swapchain surface кодека: acquire → clear → submit → present с замерами.

use std::time::{Duration, Instant};

use ash::vk;

use super::instance::PresentDevice;
use super::policy::{RenderParams, desired_present_time_ns, frame_clear_color, median_u64};
use super::report::RenderReport;
use super::swapchain::{FRAMES_IN_FLIGHT, FrameSlot, Rig};
use crate::vulkan::vk_step;

/// Таймаут ожидания fence/acquire: кодек может притормаживать, но минуту ждать незачем.
const WAIT_TIMEOUT_NS: u64 = 1_000_000_000;

/// Текущее время CLOCK_MONOTONIC в наносекундах (та же шкала, что у PTS кадров Android).
fn monotonic_now_ns() -> u64 {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: now — валидный out-параметр; CLOCK_MONOTONIC всегда доступен.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    if status != 0 {
        return 0;
    }
    (now.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(now.tv_nsec as u64)
}

/// Накопитель замеров по кадрам.
#[derive(Default)]
struct Stats {
    acquire_us: Vec<u64>,
    present_us: Vec<u64>,
    presented: u32,
    suboptimal: u32,
    out_of_date: u32,
}

fn micros(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Записывает: UNDEFINED → TRANSFER_DST, clear, → PRESENT_SRC.
fn record_clear(rig: &Rig, slot: &FrameSlot, image: vk::Image, color: [f32; 4]) -> Result<(), String> {
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    let to_clear = vk::ImageMemoryBarrier::default()
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(range);
    let to_present = to_clear
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::empty());
    let clear_value = vk::ClearColorValue { float32: color };
    let begin = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    let cmd = slot.command_buffer;
    // SAFETY: cmd принадлежит pool с RESET_COMMAND_BUFFER и не исполняется (fence слота сигнален).
    unsafe {
        vk_step(rig.device.begin_command_buffer(cmd, &begin), "vkBeginCommandBuffer")?;
        rig.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_clear],
        );
        rig.device.cmd_clear_color_image(
            cmd,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &clear_value,
            &[range],
        );
        rig.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_present],
        );
        vk_step(rig.device.end_command_buffer(cmd), "vkEndCommandBuffer")
    }
}

fn submit(rig: &Rig, present: &PresentDevice, slot: &FrameSlot, image_index: u32) -> Result<(), String> {
    let waits = [slot.image_available];
    let stages = [vk::PipelineStageFlags::TRANSFER];
    let buffers = [slot.command_buffer];
    let signals = [rig.render_finished[image_index as usize]];
    let info = vk::SubmitInfo::default()
        .wait_semaphores(&waits)
        .wait_dst_stage_mask(&stages)
        .command_buffers(&buffers)
        .signal_semaphores(&signals);
    // SAFETY: очередь используется только этим потоком; массивы живут до конца вызова.
    vk_step(
        unsafe { rig.device.queue_submit(present.queue, &[info], slot.in_flight) },
        "vkQueueSubmit",
    )
}

/// Исход одного present: что произошло с кадром.
enum PresentOutcome {
    Presented { suboptimal: bool },
    OutOfDate,
}

fn present_frame(
    rig: &Rig,
    present: &PresentDevice,
    image_index: u32,
    desired_time: Option<(u32, u64)>,
) -> Result<PresentOutcome, String> {
    let waits = [rig.render_finished[image_index as usize]];
    let swapchains = [rig.swapchain];
    let indices = [image_index];
    let mut info = vk::PresentInfoKHR::default()
        .wait_semaphores(&waits)
        .swapchains(&swapchains)
        .image_indices(&indices);
    // Время для VK_GOOGLE_display_timing: loader пробрасывает его в метку времени буфера (PTS кодека).
    let times = desired_time.map(|(id, time)| {
        [vk::PresentTimeGOOGLE::default()
            .present_id(id)
            .desired_present_time(time)]
    });
    let mut times_info = times.as_ref().map(|t| vk::PresentTimesInfoGOOGLE::default().times(t));
    if let Some(extra) = times_info.as_mut() {
        info = info.push_next(extra);
    }
    // SAFETY: все структуры в info живы до конца вызова; очередь используем только мы.
    match unsafe { rig.swapchain_loader.queue_present(present.queue, &info) } {
        Ok(suboptimal) => Ok(PresentOutcome::Presented { suboptimal }),
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(PresentOutcome::OutOfDate),
        Err(code) => Err(format!("vkQueuePresentKHR: {code:?}")),
    }
}

/// Один кадр. `Ok(false)` — swapchain устарел, цикл надо прервать.
fn render_frame(
    rig: &Rig,
    present: &PresentDevice,
    index: u32,
    params: &RenderParams,
    base_time_ns: Option<u64>,
    stats: &mut Stats,
) -> Result<bool, String> {
    let slot = &rig.slots[index as usize % FRAMES_IN_FLIGHT];
    // SAFETY: fence принадлежит слоту; device жив.
    vk_step(
        unsafe { rig.device.wait_for_fences(&[slot.in_flight], true, WAIT_TIMEOUT_NS) },
        "vkWaitForFences",
    )?;

    let acquire_started = Instant::now();
    // SAFETY: swapchain и семафор слота живы; семафор не ожидается другими операциями (fence сигнален).
    let acquired = unsafe {
        rig.swapchain_loader
            .acquire_next_image(rig.swapchain, WAIT_TIMEOUT_NS, slot.image_available, vk::Fence::null())
    };
    stats.acquire_us.push(micros(acquire_started));
    let (image_index, acquire_suboptimal) = match acquired {
        Ok(result) => result,
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            stats.out_of_date += 1;
            return Ok(false);
        }
        Err(code) => return Err(format!("vkAcquireNextImageKHR: {code:?}")),
    };
    stats.suboptimal += u32::from(acquire_suboptimal);

    // SAFETY: fence не используется GPU (дождались выше).
    vk_step(unsafe { rig.device.reset_fences(&[slot.in_flight]) }, "vkResetFences")?;
    record_clear(rig, slot, rig.images[image_index as usize], frame_clear_color(index))?;
    submit(rig, present, slot, image_index)?;

    let desired = base_time_ns.map(|base| (index, desired_present_time_ns(base, index, params.frame_interval_ns)));
    let present_started = Instant::now();
    let outcome = present_frame(rig, present, image_index, desired)?;
    stats.present_us.push(micros(present_started));
    match outcome {
        PresentOutcome::Presented { suboptimal } => {
            stats.presented += 1;
            stats.suboptimal += u32::from(suboptimal);
            Ok(true)
        }
        PresentOutcome::OutOfDate => {
            stats.out_of_date += 1;
            Ok(false)
        }
    }
}

/// Рендерит `params.frames` кадров с паузой до `frame_interval_ns`; возвращает отчёт (ошибка — внутри него).
pub fn run_render_loop(rig: &Rig, present: &PresentDevice, params: &RenderParams) -> RenderReport {
    let use_times = present.has_display_timing;
    let base_time_ns = use_times.then(monotonic_now_ns);
    let mut stats = Stats::default();
    let mut error = None;
    let wall_started = Instant::now();
    let interval = Duration::from_nanos(params.frame_interval_ns);
    for index in 0..params.frames {
        match render_frame(rig, present, index, params, base_time_ns, &mut stats) {
            Ok(true) => {}
            Ok(false) => break,
            Err(message) => {
                error = Some(message);
                break;
            }
        }
        // Surface кодека может не блокировать present — держим темп сами.
        let next_frame_at = wall_started + interval.saturating_mul(index + 1);
        if let Some(pause) = next_frame_at.checked_duration_since(Instant::now()) {
            std::thread::sleep(pause);
        }
    }
    RenderReport {
        frames_requested: params.frames,
        frames_presented: stats.presented,
        suboptimal: stats.suboptimal,
        out_of_date: stats.out_of_date,
        acquire_us_median: median_u64(&stats.acquire_us),
        present_us_median: median_u64(&stats.present_us),
        acquire_us_max: stats.acquire_us.iter().copied().max().unwrap_or(0),
        present_us_max: stats.present_us.iter().copied().max().unwrap_or(0),
        wall_ms: u64::try_from(wall_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        present_times_used: use_times,
        error,
    }
}
