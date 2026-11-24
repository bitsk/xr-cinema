use serde::Serialize;

/// Center of the optical window on the front guard, in grip-space metres.
pub const EMITTER: [f32; 3] = [0., 0.057, -0.069];
pub const SECONDARY_REACH: f32 = 0.32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    pub orientation: [f32; 4],
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            position: [0.; 3],
            orientation: [0., 0., 0., 1.],
        }
    }
}

impl Pose {
    pub fn transform_point(&self, point: [f32; 3]) -> [f32; 3] {
        let [x, y, z, w] = self.orientation;
        let [px, py, pz] = point;
        let tx = 2. * (y * pz - z * py);
        let ty = 2. * (z * px - x * pz);
        let tz = 2. * (x * py - y * px);
        [
            self.position[0] + px + w * tx + y * tz - z * ty,
            self.position[1] + py + w * ty + z * tx - x * tz,
            self.position[2] + pz + w * tz + x * ty - y * tx,
        ]
    }

    /// OpenXR aim poses point along local -Z.
    pub fn forward(&self) -> [f32; 3] {
        let [x, y, z, w] = self.orientation;
        [
            -2. * (x * z + w * y),
            -2. * (y * z - w * x),
            2. * (x * x + y * y) - 1.,
        ]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RayState {
    #[default]
    Idle,
    Hover,
    Pressed,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Ray {
    pub origin: [f32; 3],
    pub end: [f32; 3],
    pub state: RayState,
    pub active: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HandVisual {
    pub grip: Option<Pose>,
    pub ray: Option<Ray>,
    pub trigger: f32,
    pub squeeze: bool,
    pub primary_button: bool,
    pub secondary_button: bool,
    pub stick: [f32; 2],
}

#[derive(Clone, Copy, Default)]
pub struct ControllerFrame {
    pub hands: [HandVisual; 2],
    pub pointer: Option<[f32; 2]>,
}

// Diagnostics for the last submitted stereo frame, in left/right order.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ControllerStatus {
    pub model_visible: bool,
    pub ray: Option<RayState>,
    pub active_pointer: bool,
}

impl From<HandVisual> for ControllerStatus {
    fn from(hand: HandVisual) -> Self {
        Self {
            model_visible: hand.grip.is_some(),
            ray: hand.ray.map(|ray| ray.state),
            active_pointer: hand.ray.is_some_and(|ray| ray.active),
        }
    }
}

pub fn panel_point([u, v]: [f32; 2]) -> [f32; 3] {
    let [width, height, center_y, plane_z] = crate::renderer::PANEL;
    [(u - 0.5) * width, center_y + (0.5 - v) * height, plane_z]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_endpoint_and_panel_control_use_the_same_point() {
        for uv in [[0.14, 0.76], [0.53, 0.87], [0.88, 0.94]] {
            let end = panel_point(uv);
            let origin = [-0.24, -0.26, -0.39];
            let direction = std::array::from_fn(|i| end[i] - origin[i]);
            let hit = crate::renderer::panel_hit(origin, direction).unwrap();
            assert!((hit[0] - uv[0]).abs() < 1e-5 && (hit[1] - uv[1]).abs() < 1e-5);
        }
        let side = std::f32::consts::FRAC_1_SQRT_2;
        let rotated = Pose {
            orientation: [0., side, 0., side],
            ..Pose::default()
        };
        let direction = rotated.forward();
        assert!((direction[0] + 1.).abs() < 1e-5 && direction[2].abs() < 1e-5);
    }

    #[test]
    fn optical_window_rotates_with_the_grip_and_translates_with_the_hand() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let grip = Pose {
            position: [0.24, -0.18, -0.42],
            orientation: [0., half, 0., half],
        };
        let expected = [
            grip.position[0] + EMITTER[2],
            grip.position[1] + EMITTER[1],
            grip.position[2],
        ];
        for (actual, expected) in grip.transform_point(EMITTER).into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }
}
