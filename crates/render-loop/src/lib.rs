//! `render-loop` — поток рендера Fasticamera (решение владельца 31, сессия 03).
//!
//! Владеет surface экрана после маршрутизатора событий: принимает её через почтовый ящик,
//! поднимает swapchain-цель, рисует кадр на каждый тик vsync и отпускает surface (с квитанцией)
//! только после того, как GPU закончил работу и swapchain уничтожен.
//!
//! Чего НЕ делает: не читает очередь событий оболочки (это маршрутизатор `fasticamera-app`)
//! и не решает, что показывать на экране камеры (будущий `camera-screen`).

pub mod backend;
pub mod camera_diagnostics;
pub mod config;
pub mod mailbox;
pub mod pacing;
pub mod still;
pub mod still_diagnostics;
pub mod surface_state;
pub mod thread;
pub mod ui_overlay;
pub mod viewfinder;
pub mod vulkan_backend;
