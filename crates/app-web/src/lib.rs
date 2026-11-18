#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;

use renderer_core::{backend, Renderer};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

thread_local! {
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

fn js_error(message: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&message.to_string())
}

/// 从页面 canvas 初始化 WebGPU 渲染器。
#[wasm_bindgen]
pub async fn init(canvas_id: &str) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let window = web_sys::window().ok_or_else(|| js_error("no browser window"))?;
    let document = window
        .document()
        .ok_or_else(|| js_error("no browser document"))?;
    let element = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| js_error(format!("canvas #{canvas_id} not found")))?;
    let canvas: HtmlCanvasElement = element
        .dyn_into()
        .map_err(|_| js_error("element is not a canvas"))?;

    let instance = backend::create_instance_for(wgpu::Backends::BROWSER_WEBGPU);
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(js_error)?;
    let renderer = Renderer::new(
        instance,
        surface,
        (canvas.width(), canvas.height()),
        wgpu::Backends::BROWSER_WEBGPU,
    )
    .await
    .map_err(js_error)?;

    RENDERER.with(|slot| *slot.borrow_mut() = Some(renderer));
    Ok(())
}

/// 由 JavaScript 的 requestAnimationFrame 驱动。
#[wasm_bindgen]
pub fn render_frame() -> Result<(), JsValue> {
    RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let renderer = slot
            .as_mut()
            .ok_or_else(|| js_error("renderer not initialized"))?;
        renderer.render_frame().map_err(js_error)
    })
}
