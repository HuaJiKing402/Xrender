#[cfg(not(target_vendor = "apple"))]
fn main() {
    eprintln!("app-apple only builds on macOS/iOS targets");
}

#[cfg(target_vendor = "apple")]
mod apple {
    use std::sync::Arc;
    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, EventLoop},
        keyboard::{Key, NamedKey},
        window::{Window, WindowId},
    };

    use renderer_core::{backend, Renderer};

    struct App {
        window: Option<Arc<Window>>,
        renderer: Option<Renderer>,
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let window = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title("CrossRender")
                            .with_inner_size(winit::dpi::PhysicalSize::new(800, 600)),
                    )
                    .expect("failed to create Apple window"),
            );
            let instance = backend::create_instance_for(wgpu::Backends::METAL);
            let surface = instance
                .create_surface(window.clone())
                .expect("failed to create Metal surface");
            let size = window.inner_size();
            let renderer = pollster::block_on(Renderer::new(
                instance,
                surface,
                (size.width, size.height),
                wgpu::Backends::METAL,
            ))
            .expect("failed to create renderer");
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
                    if let Some(renderer) = self.renderer.as_mut() {
                        if let Err(error) = renderer.render_frame() {
                            log::error!("render failed: {error:#}");
                            event_loop.exit();
                        }
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    pub fn run() -> anyhow::Result<()> {
        env_logger::init();
        let event_loop = EventLoop::new()?;
        let mut app = App {
            window: None,
            renderer: None,
        };
        event_loop.run_app(&mut app)?;
        Ok(())
    }
}

#[cfg(target_vendor = "apple")]
fn main() {
    apple::run().expect("app-apple failed");
}
