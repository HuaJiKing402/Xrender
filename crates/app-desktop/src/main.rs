use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use mmd::vmd;
use renderer_core::{backend, model, Renderer};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, NamedKey, PhysicalKey},
    window::{Window, WindowId},
};

const INTERACTIVE_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

fn max_frames_from_env() -> Option<u32> {
    env::var("MAX_FRAMES")
        .or_else(|_| env::var("TRIANGLE_MAX_FRAMES"))
        .ok()
        .and_then(|value| value.parse().ok())
}

fn model_path_from_args_or_env() -> Option<String> {
    env::args().nth(1).or_else(|| env::var("MODEL_PATH").ok())
}

fn vmd_path_from_args_or_env() -> Option<String> {
    env::args().nth(2).or_else(|| env::var("VMD_PATH").ok())
}

/// MMD 专有格式（PMX）走资源层，其余交给内核的通用装载器。
fn load_model(path: &str) -> Result<model::Mesh> {
    if path.to_ascii_lowercase().ends_with(".pmx") {
        mmd::pmx::load_pmx_from_path(path)
    } else {
        model::load_from_path(path)
    }
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    model_path: Option<String>,
    vmd_path: Option<String>,
    max_frames: Option<u32>,
    /// 设置 `CROSS_RENDER_CAPTURE` 时，最后一帧渲染完写一张 PNG 到该路径。
    capture_path: Option<String>,
    /// 设置 `CROSS_RENDER_GPU_TIME` 时，跑完打印一次 GPU 帧时间。
    report_gpu_time: bool,
    frame_count: u32,
    last_frame: Option<Instant>,
    drag_button: Option<MouseButton>,
    last_cursor: Option<PhysicalPosition<f64>>,
    error: Option<String>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        eprintln!("{message}");
        self.error = Some(message);
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() || self.error.is_some() {
            return;
        }

        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title("CrossRender")
                .with_inner_size(winit::dpi::PhysicalSize::new(800, 600)),
        ) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(event_loop, format!("failed to create window: {error}"));
                return;
            }
        };

        let instance = backend::create_instance();
        let surface = match instance.create_surface(window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("failed to create Vulkan surface: {error}"),
                );
                return;
            }
        };
        let size = window.inner_size();

        let mesh = match &self.model_path {
            Some(path) => match load_model(path) {
                Ok(mesh) => mesh,
                Err(error) => {
                    self.fail(
                        event_loop,
                        format!("failed to load model {path}: {error:#}"),
                    );
                    return;
                }
            },
            None => model::Mesh::triangle(),
        };
        println!(
            "Model: {} ({} vertices, {} triangles, {} materials, {} textures, {} bones)",
            mesh.name,
            mesh.vertex_count(),
            mesh.triangle_count(),
            mesh.materials.len(),
            mesh.texture_paths.len(),
            mesh.bones.len()
        );
        let texture_base_dir = self
            .model_path
            .as_deref()
            .and_then(|path| Path::new(path).parent())
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        let mut renderer = match pollster::block_on(Renderer::new_with_mesh_and_textures(
            instance,
            surface,
            (size.width, size.height),
            backend::BACKENDS,
            mesh,
            &texture_base_dir,
        )) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("renderer initialization failed: {error:#}"),
                );
                return;
            }
        };

        if let Some(path) = &self.vmd_path {
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.fail(event_loop, format!("failed to read VMD {path}: {error}"));
                    return;
                }
            };
            let clip = match vmd::parse_vmd(&bytes) {
                Ok(clip) => clip,
                Err(error) => {
                    self.fail(event_loop, format!("failed to parse VMD {path}: {error:#}"));
                    return;
                }
            };
            println!(
                "Animation: {} ({} bone tracks)",
                clip.name,
                clip.bone_tracks.len()
            );
            renderer.set_animation(clip);
        }

        if env::var("CROSS_RENDER_FPS").ok().as_deref() != Some("0") {
            let font_path = env::var("FONT_PATH").ok().map(PathBuf::from);
            let enabled = renderer.enable_fps_overlay(font_path.as_deref());
            if enabled {
                println!("Overlay: FPS");
            } else {
                println!("Overlay: FPS disabled (no usable font)");
            }
        }

        println!("Backend: {}", backend::BACKEND_LABEL);
        println!("Adapter: {}", renderer.adapter_name());
        println!("Window: {}x{}", size.width, size.height);

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.last_frame = Some(Instant::now());
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize((size.width, size.height));
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.logical_key == Key::Named(NamedKey::Escape) {
                    event_loop.exit();
                }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && matches!(event.physical_key, PhysicalKey::Code(KeyCode::KeyR))
                {
                    if let Some(renderer) = self.renderer.as_mut() {
                        renderer.reset_camera();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => {
                    if matches!(
                        button,
                        MouseButton::Left | MouseButton::Right | MouseButton::Middle
                    ) {
                        self.drag_button = Some(button);
                    }
                }
                ElementState::Released => {
                    if self.drag_button == Some(button) {
                        self.drag_button = None;
                    }
                }
            },
            WindowEvent::CursorMoved { position, .. } => {
                let previous = self.last_cursor.replace(position);
                if let (Some(previous), Some(button)) = (previous, self.drag_button) {
                    if let Some(renderer) = self.renderer.as_mut() {
                        let dx = (position.x - previous.x) as f32;
                        let dy = (position.y - previous.y) as f32;
                        match button {
                            MouseButton::Left => renderer.orbit(dx, dy),
                            MouseButton::Right | MouseButton::Middle => renderer.pan(dx, dy),
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 50.0,
                };
                if lines != 0.0 {
                    if let Some(renderer) = self.renderer.as_mut() {
                        renderer.zoom(lines.clamp(-3.0, 3.0));
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.drag_button = None;
                self.last_cursor = None;
            }
            WindowEvent::Focused(false) => {
                self.drag_button = None;
                self.last_cursor = None;
            }
            WindowEvent::RedrawRequested => {
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                if let Err(error) = renderer.render_frame() {
                    self.fail(event_loop, format!("render error: {error:#}"));
                    return;
                }
                self.frame_count += 1;
                if let Some(limit) = self.max_frames {
                    if self.frame_count >= limit {
                        if let Some(path) = self.capture_path.clone() {
                            match renderer.capture_png(Path::new(&path)) {
                                Ok(()) => println!("captured {path}"),
                                Err(error) => {
                                    self.fail(event_loop, format!("capture failed: {error:#}"));
                                    return;
                                }
                            }
                        }
                        if self.report_gpu_time {
                            match renderer.measure_gpu_time() {
                                Ok(Some(duration)) => println!(
                                    "GPU frame time: {:.3} ms",
                                    duration.as_secs_f64() * 1000.0
                                ),
                                Ok(None) => {
                                    println!("GPU frame time: unavailable (no usable clock on this driver)")
                                }
                                Err(error) => {
                                    self.fail(event_loop, format!("gpu timing failed: {error:#}"));
                                    return;
                                }
                            }
                        }
                        println!("rendered {} frames", self.frame_count);
                        event_loop.exit();
                    }
                }
                self.last_frame = Some(Instant::now());
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.max_frames.is_some() || self.error.is_some() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        let deadline = self.last_frame.unwrap_or_else(Instant::now) + INTERACTIVE_FRAME_INTERVAL;
        if Instant::now() >= deadline {
            self.last_frame = Some(Instant::now());
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    let mut app = App {
        model_path: model_path_from_args_or_env(),
        vmd_path: vmd_path_from_args_or_env(),
        max_frames: max_frames_from_env(),
        capture_path: env::var("CROSS_RENDER_CAPTURE").ok(),
        report_gpu_time: env::var("CROSS_RENDER_GPU_TIME").is_ok(),
        ..Default::default()
    };
    event_loop.run_app(&mut app)?;

    if let Some(error) = app.error {
        return Err(anyhow::anyhow!(error));
    }
    Ok(())
}
