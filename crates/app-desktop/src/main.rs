use std::{env, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use renderer_core::{backend, Renderer};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

fn max_frames_from_env() -> Option<u32> {
    env::var("TRIANGLE_MAX_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    max_frames: Option<u32>,
    frame_count: u32,
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

        let renderer =
            match pollster::block_on(Renderer::new(instance, surface, (size.width, size.height))) {
                Ok(renderer) => renderer,
                Err(error) => {
                    self.fail(
                        event_loop,
                        format!("renderer initialization failed: {error:#}"),
                    );
                    return;
                }
            };

        println!("Backend: {}", backend::BACKEND_LABEL);
        println!("Adapter: {}", renderer.adapter_name());
        println!("Window: {}x{}", size.width, size.height);

        self.window = Some(window);
        self.renderer = Some(renderer);
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
                        println!("rendered {} frames", self.frame_count);
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }

        if self.max_frames.is_none() {
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    let mut app = App {
        max_frames: max_frames_from_env(),
        ..Default::default()
    };
    event_loop.run_app(&mut app)?;

    if let Some(error) = app.error {
        return Err(anyhow::anyhow!(error));
    }
    Ok(())
}
