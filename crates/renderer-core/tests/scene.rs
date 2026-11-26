//! 多网格与多实例的行为验证。
//!
//! 黄金图只能证明"单实例画面没变"；这里补的是"多加一份资源/实例后，
//! 画面里确实多画了一块"，以及资源与实例确实是分离的两层。

use std::path::{Path, PathBuf};

use glam::{Mat4, Vec3};
use renderer_core::{backend, capture::FrameImage, model, Renderer};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const ORBIT_YAW_PIXELS: f32 = 140.0;
const ORBIT_PITCH_PIXELS: f32 = 70.0;
/// 与背景色差超过这个值就算"画了东西"的像素。
const VISIBLE_DELTA: u8 = 12;

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models")
}

fn headless(model_file: &str) -> Renderer {
    let models = models_dir();
    let mesh = model::load_from_path(models.join(model_file))
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
}

fn render(renderer: &mut Renderer) -> FrameImage {
    renderer
        .render_offscreen()
        .unwrap_or_else(|error| panic!("headless render failed: {error:#}"));
    renderer
        .read_scene_rgba()
        .unwrap_or_else(|error| panic!("readback failed: {error:#}"))
}

/// 数一数画面上有多少像素不是背景色。
fn visible_pixels(frame: &FrameImage) -> usize {
    let background = frame.pixel(0, 0).expect("the corner pixel should exist");
    let mut count = 0;
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            let Some(pixel) = frame.pixel(x, y) else {
                continue;
            };
            let delta = pixel
                .iter()
                .zip(background.iter())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            if delta > VISIBLE_DELTA {
                count += 1;
            }
        }
    }
    count
}

/// 同一个网格再挂一个实例：几何只上传一次，但画面上多出一份。
#[test]
fn a_second_instance_reuses_the_mesh_and_draws_more() {
    let mut renderer = headless("test_cube.obj");
    let single = visible_pixels(&render(&mut renderer));

    let instance = renderer
        .add_instance(0, Mat4::from_translation(Vec3::new(1.4, 0.0, 0.0)))
        .expect("adding an instance of mesh 0 should work");
    assert_eq!(instance, 1);

    // 资源还是那一份，实例多了一个。
    assert_eq!(renderer.mesh_count(), 1);
    assert_eq!(renderer.instance_count(), 2);

    let doubled = visible_pixels(&render(&mut renderer));
    assert!(
        doubled > single,
        "a second instance should add pixels: {single} -> {doubled}"
    );
    assert!(
        doubled < single * 3,
        "a second instance should not explode the coverage: {single} -> {doubled}"
    );
}

/// 加一份独立网格资源：实例数与资源数都增加，画面同样多出内容。
#[test]
fn a_second_mesh_is_a_separate_resource() {
    let mut renderer = headless("test_cube.obj");
    let single = visible_pixels(&render(&mut renderer));

    let models = models_dir();
    let stack = model::load_from_path(models.join("test_cube_stack.obj"))
        .expect("test_cube_stack.obj should load");
    renderer
        .add_mesh(
            stack,
            &models,
            Mat4::from_translation(Vec3::new(1.6, 0.0, 0.0)),
        )
        .expect("adding a second mesh should work");

    assert_eq!(renderer.mesh_count(), 2);
    assert_eq!(renderer.instance_count(), 2);
    assert_eq!(renderer.mesh_name(1), Some("test_cube_stack"));

    let doubled = visible_pixels(&render(&mut renderer));
    assert!(
        doubled > single,
        "a second mesh should add pixels: {single} -> {doubled}"
    );
}

/// 实例的变换真的参与渲染，而不是只写进 uniform 就完事。
#[test]
fn moving_an_instance_moves_its_pixels() {
    let mut renderer = headless("test_cube.obj");
    let _second = renderer
        .add_instance(0, Mat4::from_translation(Vec3::new(1.4, 0.0, 0.0)))
        .expect("adding an instance should work");
    let before = render(&mut renderer);

    renderer
        .set_instance_transform(1, Mat4::from_translation(Vec3::new(-1.4, 0.0, 0.0)))
        .expect("moving the instance should work");
    let after = render(&mut renderer);

    let mean = before
        .channel_difference(&after)
        .expect("frames should have the same size");
    assert!(
        mean > 1.0,
        "moving an instance should change the picture, mean delta was {mean:.2}"
    );
}

/// 不存在的资源/实例下标要报错，而不是悄悄画错。
#[test]
fn unknown_indices_are_rejected() {
    let mut renderer = headless("test_cube.obj");
    assert!(renderer.add_instance(7, Mat4::IDENTITY).is_err());
    assert!(renderer.set_instance_transform(9, Mat4::IDENTITY).is_err());
}
