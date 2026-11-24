use super::Eye;
use crate::hands::{HandVisual, RayState, EMITTER};
use std::f32::consts::TAU;

// Static geometry in metres, centred on OpenXR grip space. The shader mirrors
// it for the left hand and articulates the controls before applying the pose.
type V3 = [f32; 3];
type Finish = [f32; 4]; // display RGB, then polish (above one = illuminated)
const CERAMIC: Finish = [0.78, 0.83, 0.87, 0.56];
const SILVER: Finish = [0.61, 0.69, 0.75, 0.81];
const GRAPHITE: Finish = [0.19, 0.23, 0.29, 0.46];
const RUBBER: Finish = [0.15, 0.18, 0.22, 0.12];
const SEAM: Finish = [0.075, 0.095, 0.12, 0.29];
const BUTTON: Finish = [0.30, 0.35, 0.42, 0.64];
const INK: Finish = [0.86, 0.93, 0.98, 0.33];
const LIGHT: Finish = [0.25, 0.76, 0.98, 1.5];
const FIXED: u32 = 0;
const TRIGGER: u32 = 1;
const SQUEEZE: u32 = 2;
const STICK: u32 = 3;
const PRIMARY: u32 = 4;
const SECONDARY: u32 = 5;
const A: u32 = 6;
const B: u32 = 7;
const X: u32 = 8;
const Y: u32 = 9;
const BEAM: u32 = 10;
const RETICLE: u32 = 11;
const PALM_PAD: u32 = 12;
const LENS: u32 = 13;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct HandVertex {
    pub position: V3,
    pub normal: V3,
    pub color: Finish,
    pub part: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct HandConstants {
    eye_rotation: [f32; 4],
    eye_origin: [f32; 4],
    tangents: [f32; 4],
    grip_rotation: [f32; 4],
    grip_origin: [f32; 4],
    ray_origin: [f32; 4],
    ray_direction: [f32; 4],
    controls: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<HandVertex>() == 44);
const _: () = assert!(std::mem::size_of::<HandConstants>() == 128);

impl HandConstants {
    pub(super) fn new(eye: Eye, side: usize, hand: &HandVisual, srgb: bool) -> Self {
        let grip = hand.grip.unwrap_or_default();
        let ray = hand.ray.unwrap_or_default();
        let delta = sub(ray.end, ray.origin);
        let length = delta.iter().map(|x| x * x).sum::<f32>().sqrt();
        let has_ray = hand.ray.is_some() && length.is_finite() && length > 0.0001;
        let flags = hand.grip.is_some() as u32
            | ((has_ray as u32) << 1)
            | ((ray.active as u32) << 2)
            | (((ray.state != RayState::Idle) as u32) << 3)
            | (((ray.state == RayState::Pressed) as u32) << 4)
            | ((hand.squeeze as u32) << 5)
            | ((hand.primary_button as u32) << 6)
            | ((hand.secondary_button as u32) << 7);
        let four = |[x, y, z]: V3, w| [x, y, z, w];
        Self {
            eye_rotation: eye.orientation,
            eye_origin: four(eye.position, if side == 0 { -1.0 } else { 1.0 }),
            tangents: eye.fov.map(f32::tan),
            grip_rotation: grip.orientation,
            grip_origin: four(grip.position, hand.trigger.clamp(0.0, 1.0)),
            ray_origin: four(ray.origin, flags as f32),
            ray_direction: if has_ray {
                four(delta.map(|x| x / length), length)
            } else {
                [0.0, 0.0, -1.0, 0.0]
            },
            controls: [hand.stick[0], hand.stick[1], 0.0, u8::from(srgb) as f32],
        }
    }
}

fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] - b[i])
}
fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] + b[i])
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(v: V3) -> V3 {
    let len = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.map(|x| x / len.max(f32::EPSILON))
}

// Sloping thumb deck: positive v points towards the operator's wrist.
const SLOPE: f32 = 0.27;
fn deck(center: V3, u: f32, v: f32, h: f32) -> V3 {
    let (s, c) = SLOPE.sin_cos();
    add(center, [u, h * c - v * s, v * c + h * s])
}

#[derive(Clone, Copy)]
struct Section {
    center: V3,
    radii: [f32; 2],
}
fn section(x: f32, y: f32, z: f32, rx: f32, rz: f32) -> Section {
    Section {
        center: [x, y, z],
        radii: [rx, rz],
    }
}

struct Mesh(Vec<HandVertex>);
impl Mesh {
    fn quad(&mut self, vertices: [HandVertex; 4]) {
        for i in [0, 1, 2, 0, 2, 3] {
            self.0.push(vertices[i]);
        }
    }

    // Join successive elliptical cross-sections; local slopes determine
    // normals, including the taper and swept-forward shape of the grip.
    fn loft(&mut self, profile: &[Section], segments: usize, color: Finish, part: u32) {
        let at = |s: Section, theta: f32| {
            let (sin, cos) = theta.sin_cos();
            deck(s.center, s.radii[0] * cos, s.radii[1] * sin, 0.0)
        };
        let mut rings = Vec::with_capacity(profile.len() * segments);
        for (j, s) in profile.iter().copied().enumerate() {
            for i in 0..segments {
                let t = TAU * i as f32 / segments as f32;
                let along = sub(
                    at(profile[(j + 1).min(profile.len() - 1)], t),
                    at(profile[j.saturating_sub(1)], t),
                );
                let around = sub(at(s, t + 0.01), at(s, t - 0.01));
                rings.push(HandVertex {
                    position: at(s, t),
                    normal: unit(cross(along, around)),
                    color,
                    part,
                });
            }
        }
        for j in 0..profile.len() - 1 {
            for i in 0..segments {
                let a = j * segments + i;
                let b = j * segments + (i + 1) % segments;
                self.quad([rings[a], rings[a + segments], rings[b + segments], rings[b]]);
            }
        }
        for (j, sign) in [(0, -1.0), (profile.len() - 1, 1.0)] {
            let normal = deck([0.; 3], 0.0, 0.0, sign);
            let v = |position| HandVertex {
                position,
                normal,
                color,
                part,
            };
            for i in 0..segments {
                self.0.extend([
                    v(profile[j].center),
                    v(rings[j * segments + i].position),
                    v(rings[j * segments + (i + 1) % segments].position),
                ]);
            }
        }
    }

    fn disc(&mut self, center: V3, profile: &[(f32, f32, f32)], color: Finish, part: u32) {
        let rings: Vec<_> = profile
            .iter()
            .map(|&(h, rx, rz)| Section {
                center: deck(center, 0., 0., h),
                radii: [rx, rz],
            })
            .collect();
        self.loft(&rings, 24, color, part);
    }

    // A flattened magnesium tracking guard. A narrow luminous inlay follows
    // only its forward edge, leaving the thumb surface unobstructed.
    fn guard(&mut self, inlay: bool) {
        let segments = 32;
        let sides = 8;
        let limit = if inlay { 1.32 } else { 2.30 };
        let centerline = |theta: f32| {
            let (s, c) = theta.sin_cos();
            // The rear feet curve inward and bury 6 mm into the shoulders.
            let u = ((theta.abs() - 1.35) / 0.95).clamp(0., 1.);
            let foot = u * u * (3. - 2. * u);
            [
                (0.046 - 0.018 * foot) * s,
                0.037 + 0.020 * c - 0.010 * foot,
                -0.010 - 0.051 * c - 0.008 * foot,
            ]
        };
        let mut verts = Vec::new();
        for i in 0..=segments {
            let t = -limit + 2.0 * limit * i as f32 / segments as f32;
            let (sn, cs) = t.sin_cos();
            let tangent = unit(sub(centerline(t + 0.002), centerline(t - 0.002)));
            let up = unit(cross(tangent, [sn, 0., -cs]));
            let n = unit(cross(up, tangent));
            let radius = if inlay { 0.0010 } else { 0.0038 };
            let height = if inlay { 0.00076 } else { 0.0060 };
            let mut center = centerline(t);
            if inlay {
                center = add(add(center, n.map(|x| x * 0.0009)), up.map(|x| x * 0.0057));
            }
            for edge in 0..sides {
                let (s, c) = (TAU * edge as f32 / sides as f32).sin_cos();
                let normal = unit(add(n.map(|x| x * c / radius), up.map(|x| x * s / height)));
                let position = add(
                    center,
                    add(n.map(|x| x * radius * c), up.map(|x| x * height * s)),
                );
                verts.push(HandVertex {
                    position,
                    normal,
                    color: if inlay { LIGHT } else { SILVER },
                    part: FIXED,
                });
            }
        }
        for i in 0..segments {
            for j in 0..sides {
                let a = i * sides + j;
                let b = i * sides + (j + 1) % sides;
                self.quad([verts[a], verts[b], verts[b + sides], verts[a + sides]]);
            }
        }
    }

    // Recess the optical assembly into the centre of the guard. Depth is
    // measured backwards from its front glass, along local +Z. The central
    // lens vertex and the visual ray's starting point share EMITTER exactly.
    fn optical_window(&mut self) {
        let rings = [
            (0.0090, 0.0100, 0.0045, GRAPHITE),
            (0.0055, 0.0105, 0.0049, GRAPHITE),
            (0.0020, 0.0090, 0.0043, GRAPHITE),
            (0.0011, 0.0080, 0.0036, SILVER),
            (0.0011, 0.0055, 0.0024, SEAM),
            (0.0006, 0.0040, 0.0017, SEAM),
        ];
        let sample = |depth, rx, ry, angle: f32| {
            let (s, c) = angle.sin_cos();
            add(EMITTER, [rx * c, ry * s, depth])
        };
        let segments = 24;
        for i in 0..segments {
            let a = TAU * i as f32 / segments as f32;
            let b = TAU * (i + 1) as f32 / segments as f32;
            for pair in rings.windows(2) {
                let (d0, x0, y0, color) = pair[0];
                let (d1, x1, y1, _) = pair[1];
                let vertex = |angle: f32, front: bool| {
                    let p0 = sample(d0, x0, y0, angle);
                    let p1 = sample(d1, x1, y1, angle);
                    let tangent = sub(
                        sample(d0, x0, y0, angle + 0.01),
                        sample(d0, x0, y0, angle - 0.01),
                    );
                    HandVertex {
                        position: if front { p1 } else { p0 },
                        normal: unit(cross(sub(p1, p0), tangent)),
                        color,
                        part: FIXED,
                    }
                };
                self.quad([
                    vertex(a, false),
                    vertex(b, false),
                    vertex(b, true),
                    vertex(a, true),
                ]);
            }
            let lens = |position| HandVertex {
                position,
                normal: [0., 0., -1.],
                color: LIGHT,
                part: LENS,
            };
            let (d, x, y, _) = rings[rings.len() - 1];
            self.0.extend([
                lens(EMITTER),
                lens(sample(d, x, y, a)),
                lens(sample(d, x, y, b)),
            ]);
        }
    }

    fn palm_pad(&mut self, shell: &[Section]) {
        let surface = |height: f32, theta: f32| {
            let pair = shell
                .windows(2)
                .find(|pair| pair[1].center[1] >= height)
                .unwrap();
            let [a, b] = [pair[0], pair[1]];
            let mix = (height - a.center[1]) / (b.center[1] - a.center[1]);
            let (sin, cos) = theta.sin_cos();
            let at = |s: Section| deck(s.center, s.radii[0] * cos, s.radii[1] * sin, 0.);
            add(at(a), sub(at(b), at(a)).map(|x| x * mix))
        };
        let mut rows = Vec::new();
        for (height, span) in [
            (-0.082, 0.05),
            (-0.079, 0.44),
            (-0.075, 0.66),
            (-0.051, 0.78),
            (-0.018, 0.67),
            (-0.010, 0.56),
            (-0.006, 0.39),
            (-0.004, 0.12),
        ] {
            let row: [HandVertex; 15] = std::array::from_fn(|i| {
                let across = (i as f32 - 7.) / 7.;
                let theta = TAU / 4. + span * across;
                let along = sub(
                    surface(height + 0.00035, theta),
                    surface(height - 0.00035, theta),
                );
                let around = sub(
                    surface(height, theta + 0.012),
                    surface(height, theta - 0.012),
                );
                let normal = unit(cross(along, around));
                let raised = 0.0006 + 0.0010 * (1. - across.powi(6));
                let position = add(surface(height, theta), normal.map(|x| x * raised));
                let color = if i == 0 || i == 14 {
                    SEAM
                } else {
                    [0.28, 0.32, 0.37, 0.23]
                };
                HandVertex {
                    position,
                    normal,
                    color,
                    part: PALM_PAD,
                }
            });
            rows.push(row);
        }
        for pair in rows.windows(2) {
            for i in 0..14 {
                self.quad([pair[0][i], pair[1][i], pair[1][i + 1], pair[0][i + 1]]);
            }
        }
    }

    // Button legends are vector strokes on the face caps. Keeping the mirrored
    // X/Y and A/B shapes in the mesh avoids an atlas lookup at headset scale.
    fn legend(&mut self, center: V3, strokes: &[[f32; 4]], part: u32) {
        for &[x, y, u, v] in strokes {
            let dx = u - x;
            let dy = v - y;
            let inv = 0.00037 / dx.hypot(dy);
            let vtx = |x: f32, y: f32, side: f32| HandVertex {
                position: deck(
                    center,
                    x * 0.0030 - dy * inv * side,
                    y * 0.0030 + dx * inv * side,
                    0.00372,
                ),
                normal: deck([0.; 3], 0., 0., 1.),
                color: INK,
                part,
            };
            self.quad([vtx(x, y, -1.), vtx(u, v, -1.), vtx(u, v, 1.), vtx(x, y, 1.)]);
        }
    }

    fn sprite(&mut self, part: u32, y0: f32) {
        self.quad(
            [[-1., y0, 0.], [1., y0, 0.], [1., 1., 0.], [-1., 1., 0.]].map(|position| HandVertex {
                position,
                normal: [0.; 3],
                color: LIGHT,
                part,
            }),
        );
    }
}

pub(super) fn mesh() -> Vec<HandVertex> {
    let mut m = Mesh(Vec::new());
    // The split shell tapers toward the wrist and broadens into a pear-shaped
    // thumb rest. Coordinates span a 15 cm consumer controller.
    let waist = section(0., -0.018, 0.009, 0.024, 0.026);
    let collar = section(0., 0.031, -0.011, 0.040, 0.042);
    let shell = [
        section(0.002, -0.086, 0.032, 0.010, 0.012),
        section(0.002, -0.083, 0.032, 0.015, 0.017),
        section(0.002, -0.075, 0.030, 0.017, 0.019),
        section(0.001, -0.051, 0.023, 0.019, 0.022),
        waist,
        section(0., -0.001, -0.001, 0.031, 0.033),
        section(0., 0.013, -0.009, 0.038, 0.040),
        collar,
        section(0., 0.035, -0.011, 0.038, 0.040),
    ];
    m.loft(&shell, 32, CERAMIC, FIXED);
    m.palm_pad(&shell);
    m.loft(
        &[
            section(0.002, -0.087, 0.032, 0.009, 0.011),
            section(0.002, -0.084, 0.032, 0.0146, 0.0169),
        ],
        24,
        GRAPHITE,
        FIXED,
    );
    let top = [0., 0.035, -0.011];
    m.disc(
        top,
        &[
            (-0.001, 0.0388, 0.0406),
            (0.001, 0.0384, 0.0402),
            (0.0026, 0.0364, 0.0386),
        ],
        SEAM,
        FIXED,
    );
    m.disc(
        top,
        &[(0.0024, 0.0362, 0.0384), (0.0034, 0.0348, 0.0369)],
        GRAPHITE,
        FIXED,
    );
    m.guard(false);
    m.guard(true);
    m.optical_window();
    m.loft(
        &[
            section(0., -0.011, -0.032, 0.009, 0.005),
            section(0., -0.005, -0.037, 0.013, 0.007),
            section(0., 0.008, -0.044, 0.016, 0.009),
            section(0., 0.018, -0.046, 0.015, 0.006),
        ],
        20,
        GRAPHITE,
        TRIGGER,
    );
    m.loft(
        &[
            section(-0.020, -0.034, 0.009, 0.006, 0.008),
            section(-0.022, -0.022, 0.007, 0.006, 0.012),
            section(-0.025, -0.005, 0.001, 0.007, 0.014),
            section(-0.027, 0.003, -0.003, 0.006, 0.010),
        ],
        18,
        RUBBER,
        SQUEEZE,
    );
    let stick = deck(top, -0.014, -0.009, 0.0037);
    m.disc(
        stick,
        &[(0., 0.014, 0.014), (0.0018, 0.0131, 0.0131)],
        SEAM,
        FIXED,
    );
    m.disc(
        stick,
        &[
            (0.0009, 0.0075, 0.0075),
            (0.0070, 0.0050, 0.0050),
            (0.0096, 0.0092, 0.0092),
        ],
        RUBBER,
        STICK,
    );
    m.disc(
        stick,
        &[
            (0.0080, 0.0085, 0.0085),
            (0.0097, 0.0120, 0.0120),
            (0.0127, 0.0123, 0.0123),
            (0.0136, 0.0110, 0.0110),
            (0.0127, 0.0088, 0.0088),
        ],
        GRAPHITE,
        STICK,
    );
    let primary = deck(top, 0.012, 0.013, 0.0035);
    let secondary = deck(top, 0.020, -0.006, 0.0035);
    for (center, part) in [(primary, PRIMARY), (secondary, SECONDARY)] {
        m.disc(
            center,
            &[(-0.0002, 0.0088, 0.0088), (0.0009, 0.0086, 0.0086)],
            SEAM,
            FIXED,
        );
        m.disc(
            center,
            &[
                (0.0006, 0.0073, 0.0073),
                (0.0027, 0.0074, 0.0074),
                (0.0036, 0.0065, 0.0065),
            ],
            BUTTON,
            part,
        );
    }
    m.legend(
        primary,
        &[
            [-0.68, 0.9, 0., -0.9],
            [0., -0.9, 0.68, 0.9],
            [-0.43, 0.3, 0.43, 0.3],
        ],
        A,
    );
    m.legend(
        primary,
        &[[-0.62, -0.9, 0.62, 0.9], [0.62, -0.9, -0.62, 0.9]],
        X,
    );
    m.legend(
        secondary,
        &[
            [-0.55, 0.9, -0.55, -0.9],
            [-0.55, -0.9, 0.28, -0.9],
            [0.28, -0.9, 0.63, -0.56],
            [0.63, -0.56, 0.55, -0.21],
            [0.55, -0.21, 0.23, 0.],
            [-0.55, 0., 0.23, 0.],
            [0.23, 0., 0.65, 0.3],
            [0.65, 0.3, 0.65, 0.62],
            [0.65, 0.62, 0.25, 0.9],
            [0.25, 0.9, -0.55, 0.9],
        ],
        B,
    );
    m.legend(
        secondary,
        &[
            [-0.62, -0.9, 0., -0.07],
            [0.62, -0.9, 0., -0.07],
            [0., -0.07, 0., 0.9],
        ],
        Y,
    );
    m.sprite(BEAM, 0.);
    m.sprite(RETICLE, -1.);
    m.0
}
