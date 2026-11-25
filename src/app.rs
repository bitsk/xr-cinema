use crate::{
    command::{Command, Scene, Status},
    control::Server,
    library::Library,
    media::{Media, VideoFrame},
    panel::{self, Action, Artwork, Panel, Tab},
    renderer::{Pixels, Settings},
    xr::Input,
};
use anyhow::{ensure, Context, Result};
use std::{
    mem,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};

struct Layout {
    epoch: u64,
    tab: Tab,
    status: Status,
    library: Arc<Library>,
    notice: Option<String>,
    hover: Option<Action>,
    dragging: Option<Action>,
    artwork: Option<Arc<Artwork>>,
}

const CONTROL_IDLE: Duration = Duration::from_secs(4);
const FADE_OUT: Duration = Duration::from_millis(240);

pub struct App {
    media: Media,
    server: Server,
    status: Status,
    library: Arc<Library>,
    queue: Vec<PathBuf>,
    local_library: Option<Arc<Library>>,
    tab: Tab,
    notice: Option<String>,
    video: Option<VideoFrame>,
    artwork: Option<Arc<Artwork>>,
    artwork_at: Option<f64>,
    new_video: bool,
    aspect: f32,
    panel: Option<Panel>,
    new_panel: bool,
    generation: u64,
    epoch: u64,
    painting: bool,
    clickable: bool,
    capture: Option<(Action, f64)>,
    preview_time: Option<Instant>,
    hover_action: Option<Action>,
    last_activity: Instant,
    appeared: Instant,
    idle_hidden: bool,
    audible_level: f64,
    requested: Option<u64>,
    layout: mpsc::SyncSender<Layout>,
    painted: mpsc::Receiver<(u64, Result<Panel, String>)>,
    headless: bool,
    recenter: bool,
    quit: bool,
}

impl App {
    pub fn new(server: Server, dir: &Path, headless: bool) -> Result<Self> {
        let (layout, jobs) = mpsc::sync_channel::<Layout>(1);
        let (complete, painted) = mpsc::channel();
        // Text shaping, glass rasterization and alpha conversion stay off the XR frame thread.
        thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let panel = Panel::draw_interactive(
                    job.tab,
                    &job.status,
                    &job.library,
                    job.notice.as_deref(),
                    job.hover,
                    job.dragging,
                    job.artwork.as_deref(),
                )
                .map_err(|e| e.to_string());
                if complete.send((job.epoch, panel)).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            media: Media::new().map_err(anyhow::Error::msg)?,
            server,
            status: Status::initial(),
            library: Arc::new(Library::read(dir)?),
            queue: Vec::new(),
            local_library: None,
            tab: Tab::Library,
            notice: None,
            video: None,
            artwork: None,
            artwork_at: None,
            new_video: false,
            aspect: 16. / 9.,
            panel: None,
            new_panel: false,
            generation: 1,
            epoch: 0,
            painting: false,
            clickable: false,
            capture: None,
            preview_time: None,
            hover_action: None,
            last_activity: Instant::now(),
            appeared: Instant::now(),
            idle_hidden: false,
            audible_level: 1.,
            requested: None,
            layout,
            painted,
            headless,
            recenter: false,
            quit: false,
        })
    }
    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    fn controls_changed(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.capture = None;
        self.hover_action = None;
        self.clickable = false;
        self.last_activity = Instant::now();
        self.invalidate();
    }
    fn open(&mut self, path: &Path) -> Result<()> {
        let path = path
            .canonicalize()
            .with_context(|| format!("打开 {}", path.display()))?;
        self.media.open(&path).map_err(anyhow::Error::msg)?;
        let demo = crate::demos::for_path(&path);
        self.status.demo_id = demo.map(|d| d.id.clone());
        if let Some(d) = demo {
            self.status.projection = d.projection;
            self.status.stereo = d.stereo;
            self.status.squeezed = d.squeezed;
            self.status.swap_eyes = false;
        }
        self.status.path = Some(path);
        self.status.position = 0.;
        self.status.duration = None;
        self.status.playing = true;
        self.status.ended = false;
        self.status.error = None;
        self.status.video_size = None;
        self.status.decoded_frames = 0;
        self.status.decoder = None;
        self.video = None;
        self.artwork = None;
        self.artwork_at = None;
        self.new_video = true;
        self.aspect = 16. / 9.;
        self.tab = Tab::Player;
        Ok(())
    }
    pub fn playlist(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        let files = paths
            .into_iter()
            .map(|p| p.canonicalize().with_context(|| p.display().to_string()))
            .collect::<Result<Vec<_>>>()?;
        if let Some(path) = files.first() {
            self.open(path)?;
        }
        self.queue = files;
        self.controls_changed();
        Ok(())
    }
    fn step(&mut self, delta: isize) -> Result<()> {
        ensure!(!self.queue.is_empty(), "片库中尚未选择影片");
        let i = self
            .queue
            .iter()
            .position(|p| Some(p) == self.status.path.as_ref())
            .unwrap_or(0);
        let next = (i as isize + delta).rem_euclid(self.queue.len() as isize) as usize;
        self.open(&self.queue[next].clone())
    }
    pub fn apply(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Status => return Ok(()),
            Command::OpenDemo { id } => {
                self.open(&crate::demos::by_id(id.as_deref().unwrap_or("rings"))?.path())?;
                self.queue = crate::demos::catalog().iter().map(|d| d.path()).collect();
            }
            Command::Open { path } => {
                self.open(&path)?;
                self.queue = self.status.path.clone().into_iter().collect();
            }
            Command::Play | Command::Pause => {
                let playing = matches!(cmd, Command::Play);
                self.media
                    .set_playing(playing)
                    .map_err(anyhow::Error::msg)?;
                self.status.playing = playing;
                if playing {
                    self.status.ended = false;
                }
            }
            Command::Seek { seconds } => {
                ensure!(
                    seconds.is_finite() && seconds >= 0.,
                    "seek seconds must be finite and nonnegative"
                );
                let pos = self.status.duration.map_or(seconds, |end| seconds.min(end));
                self.media.seek(pos).map_err(anyhow::Error::msg)?;
                self.status.position = pos;
                self.status.ended = false;
            }
            Command::Volume { level } => {
                self.media.set_volume(level).map_err(anyhow::Error::msg)?;
                self.status.volume = level;
                if level > 0. {
                    self.audible_level = level;
                }
            }
            Command::SetProjection { projection } => self.status.projection = projection,
            Command::SetStereo { stereo } => self.status.stereo = stereo,
            Command::SetSqueezed { enabled } => self.status.squeezed = enabled,
            Command::SwapEyes { enabled } => self.status.swap_eyes = enabled,
            Command::SetScene { scene } => self.status.scene = scene,
            Command::ShowPanel { visible } => {
                self.status.panel_visible = visible;
                self.idle_hidden = false;
                if visible {
                    self.appeared = Instant::now();
                }
            }
            Command::Recenter => self.recenter = true,
            Command::Next => self.step(1)?,
            Command::Previous => self.step(-1)?,
            Command::Quit => self.quit = true,
        }
        self.notice = None;
        self.wake_controls();
        self.controls_changed();
        Ok(())
    }
    fn toggle_play(&self) -> Command {
        if self.status.playing {
            Command::Pause
        } else {
            Command::Play
        }
    }
    fn select(&mut self, act: Action, frac: f64) -> Result<()> {
        let cmd = match act {
            Action::Tab(t) => {
                self.tab = if self.tab == t { Tab::Player } else { t };
                None
            }
            Action::Play => Some(self.toggle_play()),
            Action::Demo => {
                if let Some(local) = self.local_library.take() {
                    self.library = local;
                } else {
                    self.local_library = Some(self.library.clone());
                    self.library = Arc::new(Library::demo_gallery());
                }
                None
            }
            Action::Previous => Some(Command::Previous),
            Action::Next => Some(Command::Next),
            Action::Timeline => self
                .status
                .duration
                .map(|s| Command::Seek { seconds: s * frac }),
            Action::Volume => Some(Command::Volume { level: frac }),
            Action::Mute => Some(Command::Volume {
                level: if self.status.volume > 0. {
                    0.
                } else {
                    self.audible_level
                },
            }),
            Action::Parent | Action::Refresh => {
                if self.library.demos {
                    if matches!(act, Action::Parent) {
                        if let Some(local) = self.local_library.take() {
                            self.library = local;
                        }
                    }
                    self.controls_changed();
                    return Ok(());
                }
                let path = &self.library.directory;
                let dir = if matches!(act, Action::Parent) {
                    path.parent().unwrap_or(path)
                } else {
                    path
                };
                self.library = Arc::new(Library::read(dir)?);
                None
            }
            Action::Select(id) => {
                if let Some(e) = self.library.entries.get(id) {
                    let path = e.path.clone();
                    if e.directory {
                        self.library = Arc::new(Library::read(&path)?);
                    } else {
                        self.open(&path)?;
                        self.queue = self
                            .library
                            .entries
                            .iter()
                            .filter(|e| !e.directory)
                            .map(|e| e.path.canonicalize().unwrap_or_else(|_| e.path.clone()))
                            .collect();
                    }
                }
                None
            }
            Action::Page(delta) => {
                let last = self.library.pages() - 1;
                let page = self
                    .library
                    .page
                    .saturating_add_signed(delta as isize)
                    .min(last);
                Arc::make_mut(&mut self.library).page = page;
                None
            }
            Action::Projection(projection) => Some(Command::SetProjection { projection }),
            Action::Stereo(stereo) => Some(Command::SetStereo { stereo }),
            Action::Squeeze => Some(Command::SetSqueezed {
                enabled: !self.status.squeezed,
            }),
            Action::Swap => Some(Command::SwapEyes {
                enabled: !self.status.swap_eyes,
            }),
            Action::Scene(scene) => Some(Command::SetScene { scene }),
            Action::Recenter => Some(Command::Recenter),
            Action::Hide => Some(Command::ShowPanel { visible: false }),
            Action::Quit => Some(Command::Quit),
        };
        if let Some(command) = cmd {
            return self.apply(command);
        }
        self.notice = None;
        self.controls_changed();
        Ok(())
    }
    pub fn input(&mut self, input: Input) {
        self.last_activity = Instant::now();
        let result = match input {
            Input::PointerDown(uv) => {
                if self.status.panel_visible && self.clickable {
                    self.capture = self.panel.as_ref().and_then(|p| p.at(uv));
                    self.preview_time = None;
                    self.invalidate();
                }
                Ok(())
            }
            Input::PointerMove(uv) => {
                if let Some((act, old)) = self.capture {
                    if let Some(frac) = self
                        .panel
                        .as_ref()
                        .and_then(|p| p.slider_fraction(act, uv[0]))
                    {
                        self.capture = Some((act, frac));
                        if (frac - old).abs() > f64::EPSILON
                            && self
                                .preview_time
                                .is_none_or(|t| t.elapsed() >= Duration::from_millis(70))
                        {
                            self.preview_time = Some(Instant::now());
                            self.invalidate();
                        }
                    }
                }
                Ok(())
            }
            Input::PointerUp(uv) => {
                let release = uv.and_then(|uv| self.released_control(uv));
                if self.capture.take().is_none() {
                    return;
                }
                self.invalidate();
                if let Some((act, frac)) = release {
                    self.select(act, frac)
                } else {
                    Ok(())
                }
            }
            Input::PointerCancel => {
                if self.capture.take().is_some() {
                    self.invalidate();
                }
                Ok(())
            }
            Input::TogglePlayback => self.apply(self.toggle_play()),
            Input::TogglePanel => self.apply(Command::ShowPanel {
                visible: !self.status.panel_visible,
            }),
            Input::Seek(d) => self.apply(Command::Seek {
                seconds: (self.status.position + d).max(0.),
            }),
            Input::AdjustVolume(d) => self.apply(Command::Volume {
                level: (self.status.volume + d).clamp(0., 1.),
            }),
            Input::Recenter => self.apply(Command::Recenter),
            Input::Next => self.apply(Command::Next),
            Input::Previous => self.apply(Command::Previous),
        };
        if let Err(e) = result {
            self.notice = Some(e.to_string());
            self.invalidate();
        }
    }
    pub fn tick(&mut self) -> Result<()> {
        let update = self.media.poll();
        // Only repaint the transport when its clock or visible thumb actually
        // advances. Short clips need finer updates than a feature-length film.
        let steps = (960. / update.duration.unwrap_or(960.).max(1.)).clamp(1., 8.);
        let time_changed = self.status.position as u64 != update.position as u64
            || (self.status.position * steps) as u64 != (update.position * steps) as u64;
        if time_changed
            || self.status.playing != update.playing
            || self.status.ended != update.ended
            || self.status.duration != update.duration
            || (update.error.is_some() && self.status.error != update.error)
        {
            self.invalidate();
        }
        self.status.playing = update.playing;
        self.status.ended = update.ended;
        self.status.position = update.position;
        self.status.duration = update.duration;
        self.status.decoder = update.decoder;
        if let Some(error) = update.error {
            self.status.error = Some(error);
        }
        if let Some(frame) = update.frame {
            self.status.video_size = Some([frame.width, frame.height]);
            self.status.decoded_frames += 1;
            self.aspect = frame.display_aspect;
            if !self.headless
                && self.status.panel_visible
                && self
                    .artwork_at
                    .is_none_or(|t| (t - update.position).abs() >= 3.)
            {
                self.artwork = Some(Arc::new(thumbnail(&frame, &self.status)));
                self.artwork_at = Some(update.position);
                self.invalidate();
            }
            self.video = Some(frame);
            self.new_video = true;
        }
        self.status.ready = true;
        if !self.headless {
            if self.idle_hidden && (!self.status.playing || self.status.error.is_some()) {
                self.wake_controls();
            } else if self.can_fade() && self.last_activity.elapsed() >= CONTROL_IDLE + FADE_OUT {
                self.status.panel_visible = false;
                self.idle_hidden = true;
            }
        }
        self.server.publish(&self.status);
        for _ in 0..16 {
            let Ok(req) = self.server.requests.try_recv() else {
                break;
            };
            if Instant::now() >= req.expires {
                let _ = req
                    .reply
                    .send(Err("request expired before execution".into()));
                continue;
            }
            let response = self
                .apply(req.command)
                .and_then(|_| {
                    self.server.publish(&self.status);
                    Ok(serde_json::to_value(&self.status)?)
                })
                .map_err(|e| e.to_string());
            let _ = req.reply.send(response);
        }
        if self.headless || !self.status.panel_visible {
            return Ok(());
        }
        while let Ok((epoch, panel)) = self.painted.try_recv() {
            self.painting = false;
            if epoch == self.epoch {
                if self.panel.is_none() {
                    self.last_activity = Instant::now();
                    self.appeared = Instant::now();
                }
                self.panel = Some(panel.map_err(anyhow::Error::msg)?);
                self.new_panel = true;
                self.clickable = true;
            }
        }
        if !self.painting && self.requested != Some(self.generation) {
            let mut status = self.status.clone();
            if let Some((act, fraction)) = self.capture {
                match act {
                    Action::Volume => status.volume = fraction,
                    Action::Timeline => status.position = status.duration.unwrap_or(0.) * fraction,
                    _ => (),
                }
            }
            let layout = Layout {
                epoch: self.epoch,
                tab: self.tab,
                status,
                library: self.library.clone(),
                notice: self.notice.clone(),
                hover: self.hover_action,
                dragging: self.capture.map(|(action, _)| action),
                artwork: self.artwork.clone(),
            };
            match self.layout.try_send(layout) {
                Ok(()) => {
                    self.requested = Some(self.generation);
                    self.painting = true;
                }
                Err(mpsc::TrySendError::Full(_)) => (),
                Err(mpsc::TrySendError::Disconnected(_)) => anyhow::bail!("panel renderer stopped"),
            }
        }
        Ok(())
    }
    pub fn on_presented(&mut self, hands: [crate::hands::HandVisual; 2]) {
        self.status.submitted_frames += 1;
        self.status.controllers = hands.map(Into::into);
    }
    pub fn clear_controllers(&mut self) {
        self.status.controllers = Default::default();
    }
    pub fn shutdown_requested(&self) -> bool {
        self.quit
    }
    pub fn take_recenter(&mut self) -> bool {
        mem::take(&mut self.recenter)
    }
    pub fn panel_visible(&self) -> bool {
        self.status.panel_visible
    }
    pub fn pointer_surface(&self, uv: [f32; 2]) -> bool {
        self.panel.as_ref().is_some_and(|p| p.contains(uv))
    }
    pub fn pointer_control(&self, uv: [f32; 2]) -> bool {
        self.clickable && self.panel.as_ref().is_some_and(|p| p.at(uv).is_some())
    }
    pub fn released_control(&self, uv: [f32; 2]) -> Option<(Action, f64)> {
        let (action, _) = self.capture?;
        let panel = self.panel.as_ref()?;
        if let Some(fraction) = panel.slider_fraction(action, uv[0]) {
            return Some((action, fraction));
        }
        panel
            .at(uv)
            .filter(|(hit, _)| *hit == action)
            .map(|(hit, _)| (hit, 0.))
    }
    pub fn auto_hidden(&self) -> bool {
        self.idle_hidden
    }
    pub fn wake_controls(&mut self) {
        if self.idle_hidden {
            self.idle_hidden = false;
            self.status.panel_visible = true;
            self.appeared = Instant::now();
            self.last_activity = self.appeared;
            self.invalidate();
        }
    }
    pub fn dismiss_popover(&mut self) -> bool {
        if self.tab == Tab::Player {
            return false;
        }
        self.tab = Tab::Player;
        self.controls_changed();
        true
    }
    pub fn hover(&mut self, uv: Option<[f32; 2]>) {
        if uv.is_some() {
            self.last_activity = Instant::now();
        }
        let action = uv
            .and_then(|uv| self.panel.as_ref()?.at(uv))
            .map(|(a, _)| a);
        if action != self.hover_action {
            self.hover_action = action;
            self.invalidate();
        }
    }
    fn can_fade(&self) -> bool {
        self.status.playing
            && self.tab == Tab::Player
            && self.panel.is_some()
            && self.status.panel_visible
            && self.capture.is_none()
            && self.status.error.is_none()
            && self.notice.is_none()
    }
    fn opacity(&self) -> f32 {
        let fade_in = (self.appeared.elapsed().as_secs_f32() / 0.16).min(1.);
        if !self.can_fade() {
            return fade_in;
        }
        let since_idle = self.last_activity.elapsed().saturating_sub(CONTROL_IDLE);
        let fade_out = (1. - since_idle.as_secs_f32() / FADE_OUT.as_secs_f32()).clamp(0., 1.);
        fade_in.min(fade_out)
    }
    pub fn settings(&self, hands: [crate::hands::HandVisual; 2]) -> Settings {
        Settings {
            projection: self.status.projection as u32,
            stereo: self.status.stereo as u32,
            swap_eyes: self.status.swap_eyes,
            cinema: self.status.scene == Scene::Cinema,
            panel_visible: self.status.panel_visible && self.panel.is_some(),
            panel_opacity: self.opacity(),
            hands,
            aspect: self.status.eye_aspect(self.aspect),
        }
    }
    pub fn textures(&mut self) -> (Option<Pixels<'_>>, Option<Pixels<'_>>) {
        let video = mem::take(&mut self.new_video).then(|| {
            self.video.as_ref().map_or(
                Pixels {
                    width: 1,
                    height: 1,
                    rgba: &[0, 0, 0, 255],
                },
                |f| Pixels {
                    width: f.width,
                    height: f.height,
                    rgba: &f.rgba,
                },
            )
        });
        let panel = if mem::take(&mut self.new_panel) {
            self.panel.as_ref().map(|p| Pixels {
                width: panel::W,
                height: panel::H,
                rgba: &p.rgba,
            })
        } else {
            None
        };
        (video, panel)
    }
}

// Sample a small, correctly proportioned crop of a single eye from the current
// frame. The card can follow the film without a second decoder or a GPU readback.
fn thumbnail(frame: &VideoFrame, state: &Status) -> Artwork {
    use crate::command::Stereo;
    let (w, h) = (frame.width, frame.height);
    let ew = if state.stereo == Stereo::Sbs {
        (w / 2).max(1)
    } else {
        w
    };
    let eh = if state.stereo == Stereo::TopBottom {
        (h / 2).max(1)
    } else {
        h
    };
    let left = if state.swap_eyes { w - ew } else { 0 };
    let top = if state.swap_eyes { h - eh } else { 0 };
    let fit = state.eye_aspect(frame.display_aspect) / (panel::ART_W as f32 / panel::ART_H as f32);
    let span_x = ew as f32 / fit.max(1.);
    let span_y = eh as f32 * fit.min(1.);
    let mut rgba = Vec::with_capacity((panel::ART_W * panel::ART_H * 4) as usize);
    for y in 0..panel::ART_H {
        let sy =
            ((eh as f32 - span_y) * 0.5 + (y as f32 + 0.5) * span_y / panel::ART_H as f32) as u32;
        for x in 0..panel::ART_W {
            let sx = ((ew as f32 - span_x) * 0.5 + (x as f32 + 0.5) * span_x / panel::ART_W as f32)
                as u32;
            let offset = (((top + sy.min(eh - 1)) as usize * w as usize)
                + (left + sx.min(ew - 1)) as usize)
                * 4;
            rgba.extend_from_slice(&frame.rgba[offset..offset + 4]);
        }
    }
    Artwork { rgba }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn dragging_previews_then_commits_once_and_cancel_preserves_volume() -> Result<()> {
        let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = std::env::temp_dir().join(format!("cinema-input-{}-{id}", std::process::id()));
        let socket = dir.join("control.sock");
        let server = Server::start(&socket)?;
        // A language client can poll while the OpenXR runtime is still starting.
        assert_eq!(
            crate::control::send(&socket, Command::Status)?["state"]["ready"],
            false
        );
        assert!(crate::control::send(&socket, Command::Quit).is_err());
        let mut app = App::new(server, &dir, true)?;
        app.tick()?;
        assert_eq!(
            crate::control::send(&socket, Command::Status)?["state"]["ready"],
            true
        );
        app.tab = Tab::Player;
        app.panel = Some(Panel::draw(app.tab, &app.status, &app.library, None)?);
        app.clickable = true;
        let panel = app.panel.as_ref().unwrap();
        let start = panel.point_for(Action::Volume, 0.1).unwrap();
        let half = panel.point_for(Action::Volume, 0.5).unwrap();
        app.input(Input::PointerDown(start));
        app.input(Input::PointerMove(half));
        assert!((app.capture.unwrap().1 - 0.5).abs() < 0.001);
        assert_eq!(app.status.volume, 1.);
        app.input(Input::PointerCancel);
        app.input(Input::PointerUp(Some(half)));
        assert_eq!(app.status.volume, 1.);
        app.input(Input::PointerDown(start));
        app.input(Input::PointerUp(None));
        assert_eq!(app.status.volume, 1.);
        app.input(Input::PointerDown(start));
        app.input(Input::PointerUp(Some(half)));
        assert!((app.status.volume - 0.5).abs() < 0.001);
        app.select(Action::Mute, 0.)?;
        assert_eq!(app.status.volume, 0.);
        app.select(Action::Mute, 0.)?;
        assert!((app.status.volume - 0.5).abs() < 0.001);
        app.apply(Command::ShowPanel { visible: false })?;
        app.wake_controls();
        assert!(
            !app.panel_visible(),
            "explicitly hidden controls should stay hidden"
        );
        app.idle_hidden = true;
        app.wake_controls();
        assert!(app.panel_visible());
        app.select(Action::Tab(Tab::Picture), 0.)?;
        assert!(app.dismiss_popover());
        assert!(!app.dismiss_popover());
        drop(app);
        std::fs::remove_file(dir.join("control.lock"))?;
        std::fs::remove_dir(dir)?;
        Ok(())
    }
}
