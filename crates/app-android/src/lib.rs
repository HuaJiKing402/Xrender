#![cfg(target_os = "android")]

use std::{ffi::c_void, ptr::NonNull, time::Duration};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use raw_window_handle::{AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle};
use renderer_core::{backend, Renderer};

#[no_mangle]
pub extern "C" fn android_main(app: AndroidApp) {
    let instance = backend::create_instance_for(wgpu::Backends::VULKAN);
    let mut renderer: Option<Renderer> = None;

    loop {
        let mut exit = false;
        app.poll_events(Some(Duration::from_millis(16)), |event| match event {
            PollEvent::Main(MainEvent::Terminated { .. }) => exit = true,
            PollEvent::Main(MainEvent::NativeWindowResized { .. }) => {
                if let (Some(native), Some(renderer)) =
                    (app.native_window().as_ref(), &mut renderer)
                {
                    renderer.resize((native.width() as u32, native.height() as u32));
                }
            }
            _ => {}
        });
        if exit {
            break;
        }

        if renderer.is_none() {
            if let Some(native) = app.native_window().as_ref() {
                let pointer = NonNull::new(native.ptr().as_ptr() as *mut c_void).unwrap();
                let raw_window_handle =
                    RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(pointer));
                let surface = unsafe {
                    instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: Some(RawDisplayHandle::Android(Default::default())),
                        raw_window_handle,
                    })
                }
                .expect("failed to create Android Vulkan surface");
                renderer = Some(
                    pollster::block_on(Renderer::new(
                        instance.clone(),
                        surface,
                        (native.width() as u32, native.height() as u32),
                        wgpu::Backends::VULKAN,
                    ))
                    .expect("failed to create renderer"),
                );
            }
        }

        if let Some(renderer) = renderer.as_mut() {
            if let Err(error) = renderer.render_frame() {
                log::error!("render failed: {error:#}");
                break;
            }
        }
    }
}
