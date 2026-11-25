//! GPU 计时接口的契约：要么给出真实耗时，要么明确说"这台设备测不了"。
//!
//! 这台开发机的 RX 5700 XT 会声称支持时间戳却只写出 0，正好把两种分支都覆盖上：
//! 测试不允许出现"0 ms"这种看起来正常、实际没测到的结果。

use std::path::Path;

use renderer_core::{backend, model, Renderer};

#[test]
fn gpu_timing_is_optional_and_never_fake() {
    let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models");
    let mesh =
        model::load_from_path(models.join("test_cube_stack.obj")).expect("test model should load");
    let mut renderer = pollster::block_on(Renderer::new_headless(
        backend::create_instance(),
        (160, 120),
        backend::BACKENDS,
        mesh,
        &models,
    ))
    .expect("headless renderer should initialize");

    match renderer
        .measure_gpu_time()
        .expect("measuring GPU time must not fail")
    {
        Some(duration) => assert!(
            duration.as_nanos() > 0,
            "a measured frame must take a non-zero amount of time"
        ),
        None => println!("adapter exposes no usable GPU clock; timing reported as unavailable"),
    }
}
