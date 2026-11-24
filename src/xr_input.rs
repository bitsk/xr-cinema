use super::Input;
use crate::{
    hands::{
        panel_point, ControllerFrame, HandVisual, Pose, Ray, RayState, EMITTER, SECONDARY_REACH,
    },
    renderer::panel_hit,
    App,
};
use anyhow::{ensure, Context, Result};
use openxr as xr;
use std::time::{Duration, Instant};

const HANDS: [&str; 2] = ["left", "right"];
const SELECT: usize = 0;
const MENU: usize = 1;
const PLAY: usize = 2;
const CENTER: usize = 3;
const TRACK: usize = 4;

#[derive(Clone, Copy)]
enum Profile {
    Simple,
    Touch,
    Index,
    Vive,
    Pico4,
    Neo3,
}

impl Profile {
    fn path(self) -> &'static str {
        match self {
            Self::Simple => "/interaction_profiles/khr/simple_controller",
            Self::Touch => "/interaction_profiles/oculus/touch_controller",
            Self::Index => "/interaction_profiles/valve/index_controller",
            Self::Vive => "/interaction_profiles/htc/vive_controller",
            Self::Pico4 => "/interaction_profiles/bytedance/pico4_controller",
            Self::Neo3 => "/interaction_profiles/bytedance/pico_neo3_controller",
        }
    }
}

// The first active sample only arms the control after release. A button held
// through a focus change or controller reconnect must not replay its click.
#[derive(Default, Clone, Copy)]
struct Button {
    armed: bool,
}

impl Button {
    fn press(&mut self, active: bool, down: bool) -> bool {
        let fire = active && down && self.armed;
        self.armed = active && !down;
        fire
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    fn input(self) -> Input {
        match self {
            Self::Left => Input::Seek(-10.0),
            Self::Right => Input::Seek(10.0),
            Self::Up => Input::AdjustVolume(0.05),
            Self::Down => Input::AdjustVolume(-0.05),
        }
    }
}

#[derive(Default)]
struct Stick {
    armed: bool,
    held: Option<(Direction, Instant)>,
}

impl Stick {
    fn step(&mut self, active: bool, [x, y]: [f32; 2], now: Instant) -> Option<Input> {
        if !active || !x.is_finite() || !y.is_finite() {
            *self = Self::default();
            return None;
        }
        let magnitude = x.abs().max(y.abs());
        if magnitude < 0.35 {
            self.armed = true;
            self.held = None;
        }
        if !self.armed || magnitude < 0.65 {
            return None;
        }
        let direction = if x.abs() >= y.abs() {
            if x < 0.0 {
                Direction::Left
            } else {
                Direction::Right
            }
        } else if y < 0.0 {
            Direction::Down
        } else {
            Direction::Up
        };
        let delay = match self.held {
            Some((prior, until)) if prior == direction && now < until => return None,
            Some((prior, _)) if prior == direction => Duration::from_millis(300),
            _ => Duration::from_millis(600),
        };
        self.held = Some((direction, now + delay));
        Some(direction.input())
    }
}

#[derive(Default)]
struct Hand {
    buttons: [Button; 5],
    stick: Stick,
    beam: Beam,
}

#[derive(Default, Clone, Copy)]
struct Aim {
    tracked: bool,
    // A captured slider follows the ray across transparent parts of the canvas.
    hit: Option<[f32; 2]>,
    on_surface: bool,
}

impl Aim {
    fn new(pose: Option<Pose>, app: &App) -> Self {
        let Some(pose) = pose else {
            return Self::default();
        };
        let hit = panel_hit(pose.position, pose.forward());
        Self {
            tracked: true,
            on_surface: hit.is_some_and(|uv| app.pointer_surface(uv)),
            hit,
        }
    }

    fn surface(self) -> Option<[f32; 2]> {
        self.hit.filter(|_| self.on_surface)
    }
}

fn pointer_hand(
    preferred: usize,
    owner: Option<usize>,
    aims: [Aim; 2],
    controls: [bool; 2],
) -> usize {
    if let Some(owner) = owner {
        return owner;
    }
    // Prefer an actionable hit over glass, glass over open space, and any
    // tracked aim over a missing one. The current hand wins equal candidates.
    let priority = |i: usize| (controls[i], aims[i].surface().is_some(), aims[i].tracked);
    let other = 1 - preferred;
    if priority(other) > priority(preferred) {
        other
    } else {
        preferred
    }
}

fn tracked_pose(location: xr::SpaceLocation) -> Option<Pose> {
    use xr::SpaceLocationFlags as F;
    // A disconnected controller may keep reporting its last position as VALID.
    // Only project a device while the runtime is tracking its six-DoF pose.
    if !location.location_flags.contains(
        F::POSITION_VALID | F::ORIENTATION_VALID | F::POSITION_TRACKED | F::ORIENTATION_TRACKED,
    ) {
        return None;
    }
    let p = location.pose.position;
    let q = location.pose.orientation;
    let position = [p.x, p.y, p.z];
    let orientation = [q.x, q.y, q.z, q.w];
    if !position.iter().chain(&orientation).all(|x| x.is_finite()) {
        return None;
    }
    Some(Pose {
        position,
        orientation,
    })
}

fn locate(
    action: &xr::Action<xr::Posef>,
    space: &xr::Space,
    hand: xr::Path,
    session: &xr::Session<xr::Vulkan>,
    base: &xr::Space,
    time: xr::Time,
) -> Result<Option<Pose>> {
    if action.is_active(session, hand)? {
        Ok(tracked_pose(space.locate(base, time)?))
    } else {
        Ok(None)
    }
}

const IDLE_REACH: f32 = 0.75;

#[derive(Default)]
struct Beam {
    // Axial distance along the OpenXR aim ray, and the remaining alignment
    // between its native origin and the rendered controller's optical port.
    retract: Option<(Instant, f32, f32)>,
}

impl Beam {
    fn sample(
        &mut self,
        aim: Option<Pose>,
        grip: Option<Pose>,
        hit: Option<[f32; 2]>,
        state: RayState,
        active: bool,
        now: Instant,
    ) -> Option<Ray> {
        let Some(aim) = aim else {
            self.retract = None;
            return None;
        };
        let origin = grip.map_or(aim.position, |pose| pose.transform_point(EMITTER));
        let forward = aim.forward();
        let along = |length, alignment| {
            std::array::from_fn(|i| {
                origin[i] + forward[i] * length + (aim.position[i] - origin[i]) * alignment
            })
        };
        let end = if !active {
            self.retract = None;
            along(SECONDARY_REACH, 0.)
        } else if let Some(uv) = hit {
            // Native aim determines the clicked point even when grip and aim
            // have different translations and orientations on this device.
            let end = panel_point(uv);
            let reach = (0..3)
                .map(|i| (end[i] - aim.position[i]) * forward[i])
                .sum();
            self.retract = Some((now, reach, 1.));
            end
        } else if let Some((last, previous, weight)) = self.retract {
            // A 65 ms time constant is independent of headset frame rate.
            let retain = (-now.saturating_duration_since(last).as_secs_f32() / 0.065).exp();
            let length = IDLE_REACH + (previous - IDLE_REACH) * retain;
            let alignment = weight * retain;
            self.retract = ((length - IDLE_REACH).abs() > 0.001 || alignment > 0.001)
                .then_some((now, length, alignment));
            along(length, alignment)
        } else {
            along(IDLE_REACH, 0.)
        };
        Some(Ray {
            origin,
            end,
            state: if active { state } else { RayState::Idle },
            active,
        })
    }
}

#[derive(Default)]
struct Capture {
    owner: Option<usize>,
    last: [f32; 2],
}

impl Capture {
    fn cancel(&mut self) -> Option<Input> {
        self.owner.take().map(|_| Input::PointerCancel)
    }

    fn update(&mut self, hand: usize, start: bool, held: Option<bool>, aim: Aim) -> Option<Input> {
        if self.owner.is_none() && start {
            let uv = aim.surface()?;
            self.owner = Some(hand);
            self.last = uv;
            return Some(Input::PointerDown(uv));
        }
        if self.owner != Some(hand) {
            return None;
        }
        if !aim.tracked || held.is_none() {
            return self.cancel();
        }
        if held == Some(false) {
            self.owner = None;
            return Some(Input::PointerUp(aim.surface()));
        }
        let uv = aim.hit?;
        // Skip sub-texel jitter while the held ray is stationary.
        if (uv[0] - self.last[0])
            .abs()
            .max((uv[1] - self.last[1]).abs())
            < 0.001
        {
            return None;
        }
        self.last = uv;
        Some(Input::PointerMove(uv))
    }
}

pub(super) struct Controllers {
    // Drop spaces before either of their parent pose actions.
    aim_spaces: [xr::Space; 2],
    grip_spaces: [xr::Space; 2],
    aim: xr::Action<xr::Posef>,
    grip: xr::Action<xr::Posef>,
    trigger: xr::Action<f32>,
    buttons: [xr::Action<bool>; 5],
    axis: xr::Action<xr::Vector2f>,
    haptic: xr::Action<xr::Haptic>,
    set: xr::ActionSet,
    hands: [xr::Path; 2],
    gates: [Hand; 2],
    pointer_hand: usize,
    capture: Capture,
}

impl Controllers {
    pub(super) fn new(
        instance: &xr::Instance,
        session: &xr::Session<xr::Vulkan>,
        pico: bool,
    ) -> Result<Self> {
        let hands = [
            instance.string_to_path("/user/hand/left")?,
            instance.string_to_path("/user/hand/right")?,
        ];
        let set = instance.create_action_set("cinema", "Cinema controls", 0)?;
        let aim = set.create_action::<xr::Posef>("aim", "Point at controls", &hands)?;
        let grip = set.create_action::<xr::Posef>("grip_pose", "Hold the controller", &hands)?;
        let trigger = set.create_action::<f32>("trigger_travel", "Trigger travel", &hands)?;
        let buttons = [
            set.create_action::<bool>("select", "Select control", &hands)?,
            set.create_action::<bool>("menu", "Show or hide controls", &hands)?,
            set.create_action::<bool>("play", "Play or pause", &hands)?,
            set.create_action::<bool>("center", "Recenter seat", &hands)?,
            set.create_action::<bool>("track", "Previous or next movie", &hands)?,
        ];
        let axis = set.create_action::<xr::Vector2f>("axis", "Seek and volume", &hands)?;
        let haptic = set.create_action::<xr::Haptic>("feedback", "Selection feedback", &hands)?;
        let mut accepted = 0;
        for profile in [
            Profile::Simple,
            Profile::Touch,
            Profile::Vive,
            Profile::Index,
            Profile::Pico4,
            Profile::Neo3,
        ] {
            if matches!(profile, Profile::Pico4 | Profile::Neo3) && !pico {
                continue;
            }
            let mut bindings = Vec::new();
            for (index, hand) in HANDS.into_iter().enumerate() {
                let path = |component: &str| {
                    instance.string_to_path(&format!("/user/hand/{hand}/{component}"))
                };
                bindings.push(xr::Binding::new(&aim, path("input/aim/pose")?));
                bindings.push(xr::Binding::new(&grip, path("input/grip/pose")?));
                bindings.push(xr::Binding::new(&haptic, path("output/haptic")?));
                let select_path = match profile {
                    Profile::Simple => "input/select/click",
                    Profile::Vive => "input/trigger/click",
                    _ => "input/trigger/value",
                };
                bindings.push(xr::Binding::new(
                    &trigger,
                    path(match profile {
                        Profile::Vive => "input/trigger/value",
                        _ => select_path,
                    })?,
                ));
                let mut push = |button: usize, component: &str| -> Result<()> {
                    bindings.push(xr::Binding::new(&buttons[button], path(component)?));
                    Ok(())
                };
                // Keep the runtime's own click threshold and separately sample the
                // physical travel for the model (Simple Controller supplies 0/1).
                push(SELECT, select_path)?;
                match profile {
                    Profile::Simple => {
                        push(MENU, "input/menu/click")?;
                    }
                    Profile::Vive => {
                        push(MENU, "input/menu/click")?;
                        push(CENTER, "input/squeeze/click")?;
                        push(TRACK, "input/trackpad/click")?;
                        bindings.push(xr::Binding::new(&axis, path("input/trackpad")?));
                    }
                    Profile::Touch | Profile::Pico4 | Profile::Neo3 | Profile::Index => {
                        // OpenXR's float-to-boolean input binding conversion supplies
                        // the runtime's device-specific trigger/grip threshold.
                        push(CENTER, "input/squeeze/value")?;
                        push(TRACK, "input/thumbstick/click")?;
                        let left_face = index == 0 && !matches!(profile, Profile::Index);
                        push(
                            PLAY,
                            if left_face {
                                "input/x/click"
                            } else {
                                "input/a/click"
                            },
                        )?;
                        push(
                            MENU,
                            if left_face {
                                "input/y/click"
                            } else {
                                "input/b/click"
                            },
                        )?;
                        if left_face || matches!(profile, Profile::Neo3) {
                            push(MENU, "input/menu/click")?;
                        }
                        bindings.push(xr::Binding::new(&axis, path("input/thumbstick")?));
                    }
                }
            }
            let name = profile.path();
            match instance
                .suggest_interaction_profile_bindings(instance.string_to_path(name)?, &bindings)
            {
                Ok(_) => accepted += 1,
                Err(xr::sys::Result::ERROR_PATH_UNSUPPORTED) => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("bind OpenXR controller profile {name}"))
                }
            }
        }
        ensure!(
            accepted > 0,
            "runtime accepted no supported controller profile"
        );
        session.attach_action_sets(&[&set])?;
        let spaces = |pose: &xr::Action<xr::Posef>| -> Result<[xr::Space; 2]> {
            Ok([
                pose.create_space(session, hands[0], xr::Posef::IDENTITY)?,
                pose.create_space(session, hands[1], xr::Posef::IDENTITY)?,
            ])
        };
        Ok(Self {
            aim_spaces: spaces(&aim)?,
            grip_spaces: spaces(&grip)?,
            aim,
            grip,
            trigger,
            buttons,
            axis,
            haptic,
            set,
            hands,
            gates: Default::default(),
            pointer_hand: 1,
            capture: Capture::default(),
        })
    }

    pub(super) fn reset(&mut self, app: &mut App) {
        if let Some(cancel) = self.capture.cancel() {
            app.input(cancel);
        }
        self.gates = Default::default();
        app.hover(None);
        app.clear_controllers();
    }

    pub(super) fn poll(
        &mut self,
        session: &xr::Session<xr::Vulkan>,
        space: &xr::Space,
        time: xr::Time,
        app: &mut App,
    ) -> Result<ControllerFrame> {
        session.sync_actions(&[xr::ActiveActionSet::new(&self.set)])?;
        let now = Instant::now();
        let mut frame = ControllerFrame::default();
        let mut poses = [None; 2];
        let mut aims = [Aim::default(); 2];
        let mut claimed = self.capture.owner.is_some();
        let mut menu_used = false;
        for i in [self.pointer_hand, 1 - self.pointer_hand] {
            let auto_hidden = app.auto_hidden();
            let visible = app.panel_visible() && !auto_hidden;
            let hand = self.hands[i];
            poses[i] = locate(&self.aim, &self.aim_spaces[i], hand, session, space, time)?;
            let grip = locate(&self.grip, &self.grip_spaces[i], hand, session, space, time)?;
            let aim = if visible || auto_hidden {
                Aim::new(poses[i], app)
            } else {
                Aim::default()
            };
            aims[i] = aim;
            let select = self.buttons[SELECT].state(session, hand)?;
            let travel = self.trigger.state(session, hand)?;
            let trigger = if travel.is_active && travel.current_state.is_finite() {
                travel.current_state.clamp(0.0, 1.0)
            } else {
                u8::from(select.is_active && select.current_state) as f32
            };
            let mut visual = HandVisual {
                grip,
                trigger,
                ..Default::default()
            };
            let pressed = self.gates[i].buttons[SELECT].press(
                select.is_active && (!visible || aim.tracked),
                select.current_state,
            );
            if auto_hidden && (aim.surface().is_some() || pressed) {
                app.wake_controls();
                // The wake gesture only reveals the controls.
                claimed = true;
            }
            let held = select.is_active.then_some(select.current_state);
            let control = aim.surface().is_some_and(|uv| app.pointer_control(uv));
            let pointer =
                self.capture
                    .update(i, pressed && !claimed && visible && control, held, aim);
            let mut any = false;
            if let Some(event) = pointer {
                claimed = true;
                self.pointer_hand = i;
                any = matches!(event, Input::PointerUp(Some(uv)) if app.released_control(uv).is_some());
                app.input(event);
            } else if pressed && !claimed && visible && aim.tracked && aim.surface().is_none() {
                // A ray through transparent space can dismiss an open drawer.
                claimed = app.dismiss_popover();
            } else if pressed && !visible && !auto_hidden && !claimed {
                app.input(Input::TogglePlayback);
                claimed = true;
                any = true;
            }
            for (b, action) in self.buttons.iter().enumerate().skip(1) {
                let state = action.state(session, hand)?;
                let down = state.is_active && state.current_state;
                match b {
                    PLAY => visual.primary_button = down,
                    MENU => visual.secondary_button = down,
                    CENTER => visual.squeeze = down,
                    _ => {}
                }
                if !self.gates[i].buttons[b].press(state.is_active, state.current_state) {
                    continue;
                }
                if (claimed && b != MENU) || (b == MENU && menu_used) {
                    continue;
                }
                if let Some(cancel) = self.capture.cancel() {
                    app.input(cancel);
                }
                let command = match b {
                    PLAY => Input::TogglePlayback,
                    MENU => Input::TogglePanel,
                    CENTER => Input::Recenter,
                    TRACK if i == 0 => Input::Previous,
                    TRACK => Input::Next,
                    _ => unreachable!(),
                };
                self.pointer_hand = i;
                app.input(command);
                any = true;
                claimed = true;
                menu_used |= b == MENU;
            }
            let axis = self.axis.state(session, hand)?;
            let v = axis.current_state;
            if axis.is_active && v.x.is_finite() && v.y.is_finite() {
                visual.stick = [v.x.clamp(-1., 1.), v.y.clamp(-1., 1.)];
            }
            if let Some(command) = self.gates[i].stick.step(axis.is_active, [v.x, v.y], now) {
                if !claimed {
                    app.input(command);
                    any = true;
                    claimed = true;
                }
            }
            frame.hands[i] = visual;
            if any {
                let pulse = xr::HapticVibration::new()
                    .duration(xr::Duration::from_nanos(15_000_000))
                    .frequency(xr::FREQUENCY_UNSPECIFIED)
                    .amplitude(0.28);
                // A remapped controller may not expose vibration.
                let _ = self.haptic.apply_feedback(session, self.hands[i], &pulse);
            }
        }
        if !app.panel_visible() || app.auto_hidden() {
            for hand in &mut self.gates {
                hand.beam.retract = None;
            }
            return Ok(frame);
        }
        for aim in &mut aims {
            aim.on_surface = aim.hit.is_some_and(|uv| app.pointer_surface(uv));
        }
        let hits = aims.map(Aim::surface);
        let controls = hits.map(|hit| hit.is_some_and(|uv| app.pointer_control(uv)));
        self.pointer_hand = pointer_hand(self.pointer_hand, self.capture.owner, aims, controls);
        frame.pointer = hits[self.pointer_hand];
        for i in 0..2 {
            let active = i == self.pointer_hand;
            let state = if self.capture.owner == Some(i) {
                if hits[i].is_some_and(|uv| app.released_control(uv).is_some()) {
                    RayState::Pressed
                } else {
                    RayState::Idle
                }
            } else if controls[i] {
                RayState::Hover
            } else {
                RayState::Idle
            };
            frame.hands[i].ray = self.gates[i].beam.sample(
                poses[i],
                frame.hands[i].grip,
                hits[i],
                state,
                active,
                now,
            );
        }
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_pins_one_hand_and_an_untracked_idle_hand_cannot_win() {
        let missing = Aim::default();
        let in_air = Aim {
            tracked: true,
            ..missing
        };
        let on_panel = Aim {
            hit: Some([0.3, 0.8]),
            on_surface: true,
            ..in_air
        };
        assert_eq!(pointer_hand(1, None, [in_air, missing], [false; 2]), 0);
        assert_eq!(pointer_hand(0, None, [in_air, on_panel], [false; 2]), 1);
        assert_eq!(pointer_hand(0, None, [on_panel; 2], [false, true]), 1);
        assert_eq!(pointer_hand(0, None, [on_panel; 2], [true; 2]), 0);
        assert_eq!(
            pointer_hand(0, Some(1), [on_panel, in_air], [true, false]),
            1
        );
    }

    #[test]
    fn stale_pose_drops_model_or_ray_independently() {
        use xr::SpaceLocationFlags as F;
        let valid = F::POSITION_VALID | F::ORIENTATION_VALID;
        for flags in [
            valid,
            valid | F::ORIENTATION_TRACKED,
            valid | F::POSITION_TRACKED,
        ] {
            assert!(tracked_pose(xr::SpaceLocation {
                location_flags: flags,
                pose: xr::Posef::IDENTITY
            })
            .is_none());
        }
        let tracked = valid | F::POSITION_TRACKED | F::ORIENTATION_TRACKED;
        let location = xr::SpaceLocation {
            location_flags: tracked,
            pose: xr::Posef::IDENTITY,
        };
        let pose = tracked_pose(location).unwrap();
        assert_eq!(pose.forward(), [0., 0., -1.]);
    }

    #[test]
    fn optical_port_and_native_hit_can_have_different_origins_and_rotations() {
        let (sine, cosine) = 0.12_f32.sin_cos();
        let p = Pose {
            position: [0.25, -0.4, -0.55],
            orientation: [0., sine, 0., cosine],
        };
        let direction = p.forward();
        let hit = panel_hit(p.position, direction).unwrap();
        let grip = Pose {
            position: [0.18, -0.46, -0.42],
            orientation: [0., 1., 0., 0.],
        };
        let mut beam = Beam::default();
        let t = Instant::now();
        let targeting = beam
            .sample(Some(p), Some(grip), Some(hit), RayState::Hover, true, t)
            .unwrap();
        assert_eq!(targeting.origin, grip.transform_point(EMITTER));
        assert_ne!(targeting.origin, p.position);
        assert_eq!(targeting.end, panel_point(hit));
        let distance = (targeting.end[2] - p.position[2]) / direction[2];
        for (i, component) in direction.into_iter().enumerate() {
            assert!((targeting.end[i] - (p.position[i] + distance * component)).abs() < 1e-5);
        }
        let other = beam
            .sample(Some(p), Some(grip), Some(hit), RayState::Hover, false, t)
            .unwrap();
        assert_eq!(other.state, RayState::Idle);
        assert_eq!(
            other.end,
            std::array::from_fn(|i| other.origin[i] + SECONDARY_REACH * direction[i])
        );
        let aim_only = beam
            .sample(Some(p), None, Some(hit), RayState::Hover, true, t)
            .unwrap();
        assert_eq!(aim_only.origin, p.position);
        assert_eq!(aim_only.end, targeting.end);
    }

    #[test]
    fn leaving_a_target_retracts_without_a_jump_and_lost_tracking_clears_history() {
        let t = Instant::now();
        let aim = Some(Pose::default());
        let grip = Some(Pose {
            position: [0.08, -0.06, -0.02],
            ..Default::default()
        });
        let hit = panel_hit([0.; 3], [0., 0., -1.]);
        let mut beam = Beam::default();
        let contact = beam
            .sample(aim, grip, hit, RayState::Hover, true, t)
            .unwrap();
        let release = beam
            .sample(aim, grip, None, RayState::Idle, true, t)
            .unwrap();
        for i in 0..3 {
            assert!((release.end[i] - contact.end[i]).abs() < 0.0001);
        }
        let a = beam
            .sample(
                aim,
                grip,
                None,
                RayState::Idle,
                true,
                t + Duration::from_millis(12),
            )
            .unwrap();
        let b = beam
            .sample(
                aim,
                grip,
                None,
                RayState::Idle,
                true,
                t + Duration::from_millis(40),
            )
            .unwrap();
        assert!(release.end[2] < a.end[2] && a.end[2] < b.end[2] && b.end[2] < -IDLE_REACH);
        let mut slower = Beam::default();
        slower.sample(aim, grip, hit, RayState::Hover, true, t);
        let slow = slower
            .sample(
                aim,
                grip,
                None,
                RayState::Idle,
                true,
                t + Duration::from_millis(40),
            )
            .unwrap();
        assert!((slow.end[2] - b.end[2]).abs() < 0.0001);
        assert!(beam
            .sample(None, grip, hit, RayState::Idle, true, t)
            .is_none());
        let fresh = beam
            .sample(aim, grip, None, RayState::Idle, true, t)
            .unwrap();
        assert_eq!(
            fresh.end,
            [
                fresh.origin[0],
                fresh.origin[1],
                fresh.origin[2] - IDLE_REACH
            ]
        );
        let renewed = beam
            .sample(aim, grip, hit, RayState::Pressed, true, t)
            .unwrap();
        assert_eq!(renewed.end, contact.end);
        assert_eq!(renewed.state, RayState::Pressed);
    }

    #[test]
    fn capture_moves_one_hand_and_release_outside_cancels_activation() {
        let mut capture = Capture::default();
        let aim = |uv| Aim {
            tracked: true,
            hit: uv,
            on_surface: true,
        };
        let start = [0.3, 0.5];
        let end = [0.75, 0.5];
        assert_eq!(
            capture.update(1, true, Some(true), aim(Some(start))),
            Some(Input::PointerDown(start))
        );
        assert_eq!(capture.update(0, true, Some(true), aim(Some(end))), None);
        assert_eq!(
            capture.update(1, false, Some(true), aim(Some([0.3001, 0.5]))),
            None
        );
        assert_eq!(
            capture.update(1, false, Some(true), aim(Some(end))),
            Some(Input::PointerMove(end))
        );
        assert_eq!(capture.update(1, false, Some(true), aim(None)), None);
        assert_eq!(
            capture.update(1, false, Some(false), aim(None)),
            Some(Input::PointerUp(None))
        );
        assert_eq!(capture.cancel(), None);
    }

    #[test]
    fn invalid_tracking_cancels_without_synthesizing_release() {
        let mut capture = Capture::default();
        let uv = [0.5, 0.5];
        let aim = Aim {
            tracked: true,
            hit: Some(uv),
            on_surface: true,
        };
        for (held, lost) in [(None, aim), (Some(false), Aim::default())] {
            assert_eq!(
                capture.update(0, true, Some(true), aim),
                Some(Input::PointerDown(uv))
            );
            assert_eq!(
                capture.update(0, false, held, lost),
                Some(Input::PointerCancel)
            );
            assert_eq!(capture.update(0, false, Some(false), aim), None);
        }
        assert_eq!(
            capture.update(0, true, Some(true), aim),
            Some(Input::PointerDown(uv))
        );
        assert_eq!(
            capture.update(0, false, Some(false), aim),
            Some(Input::PointerUp(Some(uv)))
        );
    }

    #[test]
    fn transparent_pixels_only_receive_motion_from_an_existing_capture() {
        let mut capture = Capture::default();
        let glass = Aim {
            tracked: true,
            hit: Some([0.5, 0.85]),
            on_surface: true,
        };
        let clear = Aim {
            tracked: true,
            hit: Some([0.6, 0.4]),
            on_surface: false,
        };
        assert_eq!(clear.surface(), None);
        assert_eq!(capture.update(0, true, Some(true), clear), None);
        assert_eq!(
            capture.update(0, true, Some(true), glass),
            Some(Input::PointerDown([0.5, 0.85]))
        );
        assert_eq!(
            capture.update(0, false, Some(true), clear),
            Some(Input::PointerMove([0.6, 0.4]))
        );
        assert_eq!(
            capture.update(0, false, Some(false), clear),
            Some(Input::PointerUp(None))
        );
    }

    #[test]
    fn disconnect_discards_held_button_and_rearms_on_release() {
        let mut button = Button::default();
        assert!(!button.press(true, false));
        assert!(button.press(true, true));
        assert!(!button.press(true, true));
        assert!(!button.press(false, false));
        assert!(!button.press(true, true));
        assert!(!button.press(true, false));
        assert!(button.press(true, true));
    }

    #[test]
    fn seek_repeat_uses_time_and_dead_zone() {
        let mut stick = Stick::default();
        let t = Instant::now();
        assert_eq!(stick.step(true, [0.9, 0.0], t), None);
        assert_eq!(stick.step(true, [0.1, -0.2], t), None);
        assert_eq!(stick.step(true, [-0.9, 0.2], t), Some(Input::Seek(-10.0)));
        assert_eq!(
            stick.step(true, [-0.9, 0.2], t + Duration::from_millis(30)),
            None
        );
        assert_eq!(
            stick.step(true, [-0.9, 0.2], t + Duration::from_millis(610)),
            Some(Input::Seek(-10.0))
        );
        assert_eq!(
            stick.step(true, [0.0, 0.9], t + Duration::from_secs(1)),
            Some(Input::AdjustVolume(0.05))
        );
    }
}
