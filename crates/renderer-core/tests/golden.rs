//! 画面回归：同一模型、同一相机、同一尺寸，渲染结果必须与基线 PNG 一致。
//!
//! 基线是仓库里提交的图片。第一次跑或故意改了着色后，用
//!
//! ```text
//! $env:UPDATE_GOLDEN='1'; cargo test -p renderer-core --test golden
//! ```
//!
//! 重新生成，并在提交里说明为什么画面变了。
//!
//! 阈值留了余量：不同 GPU/驱动在光栅化与采样上的细微差别不该让测试红掉，
//! 但"某块区域整片画错"必须能被抓到。

use std::path::{Path, PathBuf};

use renderer_core::{backend, capture::FrameImage, model, Renderer};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

/// 平均每通道允许的最大差异。
const MAX_MEAN_DIFFERENCE: f32 = 3.0;
/// 单像素任一通道差超过这个值就算"这片画得不一样"。
const PIXEL_TOLERANCE: u8 = 8;
/// 这样的像素最多允许占多少比例。
const MAX_DIFFERING_RATIO: f32 = 0.01;

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models")
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

/// 固定相机：默认轨道相机正对 -Z，只看得见屏幕平行的正面；转一点成三分之四视角，
/// 每个面才有不同明暗，遮挡关系也看得出来。
const ORBIT_YAW_PIXELS: f32 = 140.0;
const ORBIT_PITCH_PIXELS: f32 = 70.0;

/// 用固定相机把模型渲染到离屏目标，取回像素。
fn render(model_file: &str) -> FrameImage {
    let models = models_dir();
    let mesh = model::load_from_path(&models.join(model_file))
        .unwrap_or_else(|error| panic!("failed to load {model_file}: {error:#}"));
    let mut renderer = pollster::block_on(Renderer::new_headless(
        backend::create_instance(),
        (WIDTH, HEIGHT),
        backend::BACKENDS,
        mesh,
        &models,
    ))
    .unwrap_or_else(|error| panic!("failed to create the headless renderer: {error:#}"));
    renderer.orbit(ORBIT_YAW_PIXELS, ORBIT_PITCH_PIXELS);
    renderer
        .render_offscreen()
        .unwrap_or_else(|error| panic!("headless render failed: {error:#}"));
    renderer
        .read_scene_rgba()
        .unwrap_or_else(|error| panic!("readback failed: {error:#}"))
}

fn check(name: &str, frame: &FrameImage) {
    let path = golden_path(name);

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        frame
            .save_png(&path)
            .unwrap_or_else(|error| panic!("failed to write baseline: {error:#}"));
        println!("baseline updated: {}", path.display());
        return;
    }

    let baseline = FrameImage::load_png(&path).unwrap_or_else(|error| {
        panic!(
            "{error:#}\nno baseline yet: run with UPDATE_GOLDEN=1 to create {}",
            path.display()
        )
    });

    let mean = frame
        .channel_difference(&baseline)
        .expect("baseline size differs from the rendered frame");
    let ratio = frame
        .differing_ratio(&baseline, PIXEL_TOLERANCE)
        .expect("baseline size differs from the rendered frame");
    println!("{name}: mean channel difference {mean:.2}, {:.2}% pixels off", ratio * 100.0);

    assert!(
        mean <= MAX_MEAN_DIFFERENCE,
        "{name}: mean channel difference {mean:.2} exceeds {MAX_MEAN_DIFFERENCE}"
    );
    assert!(
        ratio <= MAX_DIFFERING_RATIO,
        "{name}: {:.2}% of pixels differ by more than {PIXEL_TOLERANCE}, limit is {:.2}%",
        ratio * 100.0,
        MAX_DIFFERING_RATIO * 100.0
    );
}

/// 单块立方体：模型装载、UV、默认材质与光照的最基本画面对照。
#[test]
fn test_cube_matches_golden() {
    let frame = render("test_cube.obj");
    check("test_cube.png", &frame);
}

/// 三块穿插立方体：深度 prepass 与遮挡关系的画面对照。
#[test]
fn cube_stack_matches_golden() {
    let frame = render("test_cube_stack.obj");
    check("test_cube_stack.png", &frame);
}
