use glam::{Mat4, Vec3};

const ORBIT_RATE: f32 = 0.005;
const ZOOM_RATE: f32 = 0.15;
const MIN_DISTANCE: f32 = 0.6;
const MAX_DISTANCE: f32 = 60.0;
const MAX_PITCH: f32 = 1.45;
const DEFAULT_DISTANCE: f32 = 3.4;
const DEFAULT_PITCH: f32 = 0.0;
const DEFAULT_FOV_Y: f32 = 45.0_f32 * std::f32::consts::PI / 180.0;

/// 轨道相机：围绕 target 旋转、平移和缩放。
#[derive(Clone, Debug)]
pub struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov_y: f32,
    viewport: (u32, u32),
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            distance: DEFAULT_DISTANCE,
            fov_y: DEFAULT_FOV_Y,
            viewport: (800, 600),
        }
    }
}

impl OrbitCamera {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn target(&self) -> Vec3 {
        self.target
    }

    pub fn distance(&self) -> f32 {
        self.distance
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport = (width.max(1), height.max(1));
    }

    pub fn eye(&self) -> Vec3 {
        let cos_pitch = self.pitch.cos();
        self.target
            + Vec3::new(
                self.yaw.sin() * cos_pitch,
                self.pitch.sin(),
                self.yaw.cos() * cos_pitch,
            ) * self.distance
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    pub fn projection(&self) -> Mat4 {
        let aspect = self.viewport.0 as f32 / self.viewport.1 as f32;
        Mat4::perspective_rh(self.fov_y, aspect, 0.1, 100.0)
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection() * self.view()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw = wrap_angle(self.yaw - dx * ORBIT_RATE);
        self.pitch = (self.pitch + dy * ORBIT_RATE).clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        let view_inverse = self.view().inverse();
        let right = view_inverse.x_axis.truncate().normalize();
        let up = view_inverse.y_axis.truncate().normalize();
        let world_per_pixel =
            2.0 * self.distance * (self.fov_y * 0.5).tan() / self.viewport.1.max(1) as f32;
        self.target += (-right * dx + up * dy) * world_per_pixel;
    }

    pub fn zoom(&mut self, notches: f32) {
        let notches = notches.clamp(-50.0, 50.0);
        let next = self.distance * (-ZOOM_RATE * notches).exp();
        if next.is_finite() {
            self.distance = next.clamp(MIN_DISTANCE, MAX_DISTANCE);
        }
    }

    pub fn reset(&mut self) {
        let viewport = self.viewport;
        *self = Self::default();
        self.viewport = viewport;
    }
}

fn wrap_angle(angle: f32) -> f32 {
    let mut wrapped = angle.rem_euclid(std::f32::consts::TAU);
    if wrapped > std::f32::consts::PI {
        wrapped -= std::f32::consts::TAU;
    }
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_mat_close(actual: Mat4, expected: Mat4) {
        for (a, e) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
        {
            assert!((a - e).abs() < 1e-5, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn default_view_matches_fixed_camera() {
        let camera = OrbitCamera::default();
        let expected = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.4), Vec3::ZERO, Vec3::Y);
        assert_mat_close(camera.view(), expected);
        assert!((camera.eye().distance(Vec3::new(0.0, 0.0, 3.4))).abs() < 1e-5);
    }

    #[test]
    fn orbit_clamps_pitch_and_wraps_yaw() {
        let mut camera = OrbitCamera::default();
        camera.orbit(0.0, 1000.0);
        assert!(camera.pitch() <= MAX_PITCH);
        camera.orbit(0.0, -2000.0);
        assert!(camera.pitch() >= -MAX_PITCH);
        camera.orbit(1000.0, 0.0);
        assert!(camera.yaw().abs() <= std::f32::consts::PI);
    }

    #[test]
    fn zoom_clamps_distance() {
        let mut camera = OrbitCamera::default();
        camera.zoom(1000.0);
        assert!((camera.distance() - MIN_DISTANCE).abs() < 1e-5);
        camera.zoom(-1000.0);
        assert!((camera.distance() - MAX_DISTANCE).abs() < 1e-5);
    }

    #[test]
    fn pan_moves_target_in_view_plane() {
        let mut camera = OrbitCamera::default();
        camera.pan(100.0, 0.0);
        assert!(camera.target().x < 0.0);
        assert!(camera.target().y.abs() < 1e-5);

        camera.reset();
        camera.pan(0.0, 100.0);
        assert!(camera.target().y > 0.0);
        assert!(camera.target().x.abs() < 1e-5);
    }

    #[test]
    fn reset_preserves_viewport() {
        let mut camera = OrbitCamera::default();
        camera.set_viewport(1024, 768);
        camera.orbit(100.0, 100.0);
        camera.zoom(5.0);
        camera.reset();
        assert_eq!(camera.viewport, (1024, 768));
        assert!((camera.distance() - DEFAULT_DISTANCE).abs() < 1e-5);
        assert!((camera.pitch() - DEFAULT_PITCH).abs() < 1e-5);
    }
}
