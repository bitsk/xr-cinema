use crate::{
    command::{Projection, Scene, Status, Stereo},
    library::{Library, PAGE_SIZE},
};
use anyhow::Result;
use cairo::{Context, Format, ImageSurface, LineCap, LineJoin, LinearGradient, RadialGradient};
use pangocairo::{functions, pango};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

pub const W: u32 = 1024;
pub const H: u32 = 512;
pub const ART_W: u32 = 80;
pub const ART_H: u32 = 50;

pub struct Artwork {
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Player,
    Library,
    Picture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Tab(Tab),
    Play,
    Previous,
    Next,
    Timeline,
    Volume,
    Mute,
    Select(usize),
    Parent,
    Page(i32),
    Refresh,
    Demo,
    Projection(Projection),
    Stereo(Stereo),
    Squeeze,
    Swap,
    Scene(Scene),
    Recenter,
    Hide,
    Quit,
}

#[derive(Clone, Copy)]
struct Rect(f64, f64, f64, f64);

type Color = (u32, f64);
const WHITE: Color = (0xf2f7ff, 1.);
const ICE: Color = (0xb7e6ff, 1.);
const MUTED: Color = (0xa8bbcf, 1.);
const DIM: Color = (0x839ab3, 1.);
const PALE: Color = (0xeffaff, 1.);
const INK: Color = (0x16364d, 1.);
const BAR: Rect = Rect(16., 364., 992., 128.);
const SEEK: Rect = Rect(48., 367., 928., 34.);
const SOUND: Rect = Rect(715., 414., 82., 55.);

#[derive(Clone, Copy)]
enum Icon {
    Play,
    Pause,
    Previous,
    Next,
    Speaker,
    Silent,
    Folder,
    Tune,
    Hide,
    Close,
    Up,
    Refresh,
    Film,
    Planet,
    Right,
    Left,
    Recenter,
    Exit,
}

fn channels((rgb, a): Color) -> (f64, f64, f64, f64) {
    (
        ((rgb >> 16) & 255) as f64 / 255.,
        ((rgb >> 8) & 255) as f64 / 255.,
        (rgb & 255) as f64 / 255.,
        a,
    )
}
fn ink(c: &Context, color: Color) {
    let (r, g, b, a) = channels(color);
    c.set_source_rgba(r, g, b, a);
}
fn round(c: &Context, Rect(x, y, w, h): Rect, radius: f64) {
    let r = radius.min(w / 2.).min(h / 2.);
    c.new_sub_path();
    c.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.);
    c.arc(x + w - r, y + h - r, r, 0., FRAC_PI_2);
    c.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    c.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    c.close_path();
}
fn fill(c: &Context) {
    c.fill().expect("Cairo image fill");
}
fn stroke(c: &Context, width: f64) {
    c.set_line_width(width);
    c.stroke().expect("Cairo image stroke");
}
fn shape(c: &Context, r: Rect, radius: f64, color: Color) {
    if r.2 > 0. && r.3 > 0. {
        round(c, r, radius);
        ink(c, color);
        fill(c);
    }
}
fn circle(c: &Context, x: f64, y: f64, r: f64, color: Color) {
    c.new_sub_path();
    c.arc(x, y, r, 0., TAU);
    ink(c, color);
    fill(c);
}
fn line(c: &Context, points: &[(f64, f64)], width: f64, color: Color) {
    c.move_to(points[0].0, points[0].1);
    for &(x, y) in &points[1..] {
        c.line_to(x, y);
    }
    ink(c, color);
    stroke(c, width);
}
fn gradient(c: &Context, from: (f64, f64), to: (f64, f64), stops: &[(f64, Color)]) {
    let paint = LinearGradient::new(from.0, from.1, to.0, to.1);
    for &(pos, color) in stops {
        let (r, g, b, a) = channels(color);
        paint.add_color_stop_rgba(pos, r, g, b, a);
    }
    c.set_source(&paint).expect("linear gradient");
}
fn glow(c: &Context, x: f64, y: f64, radius: f64, color: Color) {
    let (r, g, b, a) = channels(color);
    let paint = RadialGradient::new(x, y, 0., x, y, radius);
    paint.add_color_stop_rgba(0., r, g, b, a);
    paint.add_color_stop_rgba(1., r, g, b, 0.);
    c.set_source(&paint).expect("radial gradient");
    c.new_sub_path();
    c.arc(x, y, radius, 0., TAU);
    fill(c);
}

fn glyph(c: &Context, icon: Icon, x: f64, y: f64, size: f64, color: Color) {
    c.save().expect("Cairo state");
    c.translate(x, y);
    c.scale(size / 24., size / 24.);
    c.set_line_cap(LineCap::Round);
    c.set_line_join(LineJoin::Round);
    ink(c, color);
    match icon {
        Icon::Pause => {
            shape(c, Rect(-7., -8.5, 4.8, 17.), 1.5, color);
            shape(c, Rect(2.2, -8.5, 4.8, 17.), 1.5, color);
        }
        Icon::Play | Icon::Previous | Icon::Next => {
            let skip = matches!(icon, Icon::Previous | Icon::Next);
            if matches!(icon, Icon::Previous) {
                c.scale(-1., 1.);
            }
            if skip {
                c.translate(-2., 0.);
                c.scale(0.84, 0.84);
            }
            // A rounded leading edge reads clearly on both the light disc and dark glass.
            c.move_to(-5.8, -8.7);
            c.curve_to(-6.9, -9.35, -7.8, -8.7, -7.8, -7.4);
            c.line_to(-7.8, 7.4);
            c.curve_to(-7.8, 8.7, -6.9, 9.35, -5.8, 8.7);
            c.line_to(8.5, 1.3);
            c.curve_to(9.8, 0.5, 9.8, -0.5, 8.5, -1.3);
            c.close_path();
            fill(c);
            if skip {
                shape(c, Rect(10., -9.2, 3.6, 18.4), 1.6, color);
            }
        }
        Icon::Speaker | Icon::Silent => {
            c.move_to(-10., -3.4);
            c.line_to(-6.1, -3.4);
            c.line_to(-0.5, -8.1);
            c.curve_to(0.3, -8.7, 1., -8.4, 1., -7.4);
            c.line_to(1., 7.4);
            c.curve_to(1., 8.4, 0.3, 8.7, -0.5, 8.1);
            c.line_to(-6.1, 3.4);
            c.line_to(-10., 3.4);
            c.close_path();
            stroke(c, 1.8);
            if matches!(icon, Icon::Silent) {
                line(c, &[(5., -3.), (11., 3.)], 1.8, color);
                line(c, &[(11., -3.), (5., 3.)], 1.8, color);
            } else {
                c.arc(-0.5, 0., 7., -0.67, 0.67);
                stroke(c, 1.8);
                c.arc(-0.5, 0., 11., -0.72, 0.72);
                stroke(c, 1.8);
            }
        }
        Icon::Folder => {
            c.move_to(-10., -4.3);
            c.line_to(-10., -6.);
            c.curve_to(-10., -7.5, -9., -8., -7.6, -8.);
            c.line_to(-3.1, -8.);
            c.line_to(-0.6, -5.);
            c.line_to(7.7, -5.);
            c.curve_to(9.4, -5., 10., -4.2, 10., -2.8);
            c.line_to(10., 5.4);
            c.curve_to(10., 6.9, 9.1, 8., 7.6, 8.);
            c.line_to(-7.7, 8.);
            c.curve_to(-9.1, 8., -10., 7., -10., 5.4);
            c.close_path();
            stroke(c, 1.85);
            line(c, &[(-9.8, -2.9), (-2.2, -2.9), (0.4, -4.8)], 1.55, color);
        }
        Icon::Tune => {
            line(c, &[(-9., -5.), (-3., -5.)], 1.85, color);
            line(c, &[(3., -5.), (9., -5.)], 1.85, color);
            line(c, &[(-9., 5.), (1., 5.)], 1.85, color);
            line(c, &[(7., 5.), (9., 5.)], 1.85, color);
            for (a, b) in [(0., -5.), (4., 5.)] {
                c.new_sub_path();
                c.arc(a, b, 2.8, 0., TAU);
                stroke(c, 1.85);
            }
        }
        Icon::Hide => line(c, &[(-7., -3.), (0., 3.5), (7., -3.)], 2., color),
        Icon::Close => {
            line(c, &[(-5.5, -5.5), (5.5, 5.5)], 1.8, color);
            line(c, &[(5.5, -5.5), (-5.5, 5.5)], 1.8, color);
        }
        Icon::Up => {
            line(c, &[(7., 6.), (-2., 6.), (-2., -7.)], 1.8, color);
            line(c, &[(-7., -2.), (-2., -7.), (3., -2.)], 1.8, color);
        }
        Icon::Refresh => {
            c.new_sub_path();
            c.arc(0., 0., 7.6, -1.06, 3.85);
            stroke(c, 1.8);
            line(c, &[(1., -8.5), (6.5, -7.), (5., -1.5)], 1.8, color);
        }
        Icon::Film => {
            round(c, Rect(-9.5, -7.5, 19., 15.), 3.);
            stroke(c, 1.6);
            for a in [-5.8, 0., 5.8] {
                line(c, &[(a, -7.1), (a, -4.6)], 1.4, color);
                line(c, &[(a, 7.1), (a, 4.6)], 1.4, color);
            }
            line(c, &[(-9., -4.3), (9., -4.3)], 1.25, color);
            line(c, &[(-9., 4.3), (9., 4.3)], 1.25, color);
        }
        Icon::Planet => {
            c.new_sub_path();
            c.arc(0., 0., 6.9, 0., TAU);
            stroke(c, 1.5);
            c.rotate(-0.38);
            c.scale(1., 0.32);
            c.new_sub_path();
            c.arc(0., 0., 11., 0., TAU);
            c.scale(1., 1. / 0.32);
            stroke(c, 1.4);
        }
        Icon::Left | Icon::Right => {
            if matches!(icon, Icon::Left) {
                c.scale(-1., 1.);
            }
            line(c, &[(-2.8, -6.), (3.1, 0.), (-2.8, 6.)], 1.9, color);
        }
        Icon::Recenter => {
            for (dx, dy) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
                line(
                    c,
                    &[(dx * 9., dy * 4.), (dx * 9., dy * 8.), (dx * 5., dy * 8.)],
                    1.7,
                    color,
                );
            }
            circle(c, 0., 0., 2.3, color);
        }
        Icon::Exit => {
            line(
                c,
                &[(-1., -8.), (-8., -8.), (-8., 8.), (-1., 8.)],
                1.7,
                color,
            );
            line(c, &[(-2., 0.), (9., 0.)], 1.7, color);
            line(c, &[(4.8, -4.3), (9., 0.), (4.8, 4.3)], 1.7, color);
        }
    }
    c.restore().expect("Cairo state");
}

#[derive(Clone, Copy)]
enum Face {
    Normal,
    Strong,
    Digits,
}
fn text(
    c: &Context,
    value: &str,
    Rect(x, y, w, h): Rect,
    px: f64,
    face: Face,
    center: bool,
    color: Color,
) -> f64 {
    let layout = functions::create_layout(c);
    let family = if matches!(face, Face::Digits) {
        "DejaVu Sans Mono, Noto Sans Mono, monospace"
    } else {
        "Noto Sans CJK SC, Noto Sans, sans-serif"
    };
    let mut font = pango::FontDescription::from_string(family);
    font.set_absolute_size(px * pango::SCALE as f64);
    if matches!(face, Face::Strong) {
        font.set_weight(pango::Weight::Semibold);
    }
    layout.set_font_description(Some(&font));
    layout.set_text(&value.replace(['\r', '\n', '\0'], " "));
    layout.set_single_paragraph_mode(true);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_width((w * pango::SCALE as f64) as i32);
    if center {
        layout.set_alignment(pango::Alignment::Center);
    }
    let (width, height) = layout.pixel_size();
    ink(c, color);
    c.move_to(x, (y + (h - height as f64) * 0.5).round());
    functions::show_layout(c, &layout);
    width as f64
}

struct Canvas {
    c: Context,
    hits: Vec<(Rect, Action)>,
    hover: Option<Action>,
    drag: Option<Action>,
}
impl Canvas {
    fn hot(&self, action: Action) -> bool {
        self.hover == Some(action) || self.drag == Some(action)
    }
    fn register(&mut self, rect: Rect, action: Action) {
        self.hits.push((rect, action));
    }

    fn glass(&self, rect: Rect, radius: f64) {
        let Rect(x, y, w, h) = rect;
        let c = &self.c;
        round(c, rect, radius);
        gradient(
            c,
            (x, y),
            (x + w * 0.23, y + h),
            &[
                (0., (0x344b60, 0.97)),
                (0.32, (0x243444, 0.97)),
                (1., (0x101b2a, 0.97)),
            ],
        );
        fill(c);
        c.save().expect("glass clip");
        round(c, rect, radius);
        c.clip();
        glow(c, x + w * 0.44, y - h * 0.42, w * 0.38, (0x91caff, 0.12));
        c.restore().expect("glass clip");
        round(c, Rect(x + 0.65, y + 0.65, w - 1.3, h - 1.3), radius - 0.5);
        gradient(
            c,
            (x, y),
            (x + w * 0.20, y + h),
            &[
                (0., (0xc1e5ff, 0.63)),
                (0.44, (0xabcfea, 0.32)),
                (1., (0x7894ae, 0.48)),
            ],
        );
        stroke(c, 1.3);
        round(c, Rect(x + 2.5, y + 2.5, w - 5., h - 5.), radius - 2.3);
        gradient(
            c,
            (x, y),
            (x, y + h),
            &[
                (0., (0xdeeefe, 0.09)),
                (0.5, (0x192a3d, 0.02)),
                (1., (0x06101d, 0.7)),
            ],
        );
        stroke(c, 1.);
    }
    fn icon_button(&mut self, rect: Rect, icon: Icon, action: Action, selected: bool, sz: f64) {
        let Rect(x, y, w, h) = rect;
        let hot = self.hot(action);
        if hot || selected {
            let radius = 17.;
            shape(
                &self.c,
                Rect(x + (w - 40.) / 2., y + (h - 40.) / 2., 40., 40.),
                radius,
                if hot {
                    (0x94c8ff, 0.20)
                } else {
                    (0x85c2ff, 0.13)
                },
            );
        }
        glyph(
            &self.c,
            icon,
            x + w / 2.,
            y + h / 2.,
            sz,
            if hot || selected { PALE } else { MUTED },
        );
        self.register(rect, action);
    }
    fn row_action(&mut self, r: Rect, action: Action) {
        if self.hot(action) {
            shape(&self.c, r, 11., (0x8cc6ff, 0.16));
        }
        self.register(r, action);
    }
}
pub fn timestamp(seconds: f64) -> String {
    let s = seconds.max(0.) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}
pub struct Panel {
    pub rgba: Vec<u8>,
    hits: Vec<(Rect, Action)>,
}
impl Panel {
    pub fn contains(&self, uv: [f32; 2]) -> bool {
        if !(0.0..1.0).contains(&uv[0]) || !(0.0..1.0).contains(&uv[1]) {
            return false;
        }
        let (x, y) = ((uv[0] * W as f32) as usize, (uv[1] * H as f32) as usize);
        self.rgba[(y * W as usize + x) * 4 + 3] >= 112
    }
    pub fn slider_fraction(&self, action: Action, u: f32) -> Option<f64> {
        if !matches!(action, Action::Volume | Action::Timeline) {
            return None;
        }
        self.hits.iter().find_map(|(Rect(x, _, w, _), a)| {
            (*a == action).then(|| ((u as f64 * W as f64 - x) / w).clamp(0., 1.))
        })
    }
    pub fn at(&self, uv: [f32; 2]) -> Option<(Action, f64)> {
        if !self.contains(uv) {
            return None;
        }
        let (px, py) = (uv[0] as f64 * W as f64, uv[1] as f64 * H as f64);
        self.hits.iter().rev().find_map(|(Rect(x, y, w, h), a)| {
            (px >= *x && px < x + w && py >= *y && py < y + h)
                .then(|| (*a, ((px - x) / w).clamp(0., 1.)))
        })
    }
    #[cfg(test)]
    pub fn point_for(&self, action: Action, fraction: f64) -> Option<[f32; 2]> {
        self.hits.iter().find_map(|(Rect(x, y, w, h), a)| {
            (*a == action).then(|| {
                [
                    ((x + w * fraction.clamp(0.0001, 0.9999)) / W as f64) as f32,
                    ((y + h * 0.5) / H as f64) as f32,
                ]
            })
        })
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn draw(tab: Tab, s: &Status, lib: &Library, notice: Option<&str>) -> Result<Self> {
        Self::draw_interactive(tab, s, lib, notice, None, None, None)
    }
    pub fn draw_interactive(
        tab: Tab,
        s: &Status,
        lib: &Library,
        notice: Option<&str>,
        hover: Option<Action>,
        dragging: Option<Action>,
        artwork: Option<&Artwork>,
    ) -> Result<Self> {
        let mut surface = ImageSurface::create(Format::ARgb32, W as i32, H as i32)?;
        let mut g = Canvas {
            c: Context::new(&surface)?,
            hits: Vec::new(),
            hover,
            drag: dragging,
        };
        g.c.set_line_cap(LineCap::Round);
        g.c.set_line_join(LineJoin::Round);
        transport(&mut g, tab, s, notice.or(s.error.as_deref()));
        thumbnail(&g.c, artwork)?;
        match tab {
            Tab::Player => (),
            Tab::Library => library(&mut g, lib),
            Tab::Picture => picture(&mut g, s),
        }
        tooltip(&g, tab, s);
        let hits = g.hits;
        drop(g.c);
        let mut out = surface.data()?.to_vec();
        for p in out.chunks_exact_mut(4) {
            let argb = u32::from_ne_bytes([p[0], p[1], p[2], p[3]]);
            let a = argb >> 24;
            if a == 0 {
                continue;
            }
            let ch = |v: u32| ((v * 255 + a / 2) / a).min(255) as u8;
            p.copy_from_slice(&[
                ch((argb >> 16) & 255),
                ch((argb >> 8) & 255),
                ch(argb & 255),
                a as u8,
            ]);
        }
        Ok(Self { rgba: out, hits })
    }
}

fn thumbnail(c: &Context, artwork: Option<&Artwork>) -> Result<()> {
    let frame = Rect(44., 418., 64., 40.);
    let Rect(x, y, w, h) = frame;
    shape(c, Rect(x - 1., y, w + 2., h + 2.), 8.5, (0x07121c, 0.86));
    shape(c, frame, 7.5, (0x1c3041, 1.));
    if let Some(a) = artwork.filter(|a| a.rgba.len() == (ART_W * ART_H * 4) as usize) {
        let mut pixels = Vec::with_capacity(a.rgba.len());
        for p in a.rgba.chunks_exact(4) {
            let alpha = u32::from(p[3]);
            let premul = |v: u8| (u32::from(v) * alpha + 127) / 255;
            let argb = alpha << 24 | premul(p[0]) << 16 | premul(p[1]) << 8 | premul(p[2]);
            pixels.extend_from_slice(&argb.to_ne_bytes());
        }
        let surface = ImageSurface::create_for_data(
            pixels,
            Format::ARgb32,
            ART_W as i32,
            ART_H as i32,
            (ART_W * 4) as i32,
        )?;
        c.save()?;
        round(c, frame, 7.5);
        c.clip();
        c.translate(x, y);
        c.scale(w / ART_W as f64, h / ART_H as f64);
        c.set_source_surface(&surface, 0., 0.)?;
        c.source().set_extend(cairo::Extend::Pad);
        c.source().set_filter(cairo::Filter::Bilinear);
        c.paint()?;
        c.restore()?;
    } else {
        glyph(c, Icon::Film, x + w / 2., y + h / 2., 24., (0xb4d3e8, 0.91));
    }
    round(c, Rect(x + 0.5, y + 0.5, w - 1., h - 1.), 7.);
    gradient(
        c,
        (x, y),
        (x + w * 0.3, y + h),
        &[(0., (0xdcf2ff, 0.44)), (1., (0x7697b2, 0.24))],
    );
    stroke(c, 1.);
    Ok(())
}

fn transport(g: &mut Canvas, tab: Tab, s: &Status, notice: Option<&str>) {
    g.glass(BAR, 31.);
    let c = &g.c;
    let Rect(x, y, w, h) = SEEK;
    let track_y = y + h / 2.;
    let total = s.duration.filter(|d| *d > 0.);
    let frac = total.map_or(0., |d| (s.position / d).clamp(0., 1.));
    let grip_x = x + w * frac;
    let held = g.hot(Action::Timeline);
    let thick = if held { 5.5 } else { 4. };
    shape(
        c,
        Rect(x, track_y - thick / 2. - 1., w, thick + 2.),
        4.,
        (0x091321, 0.62),
    );
    shape(
        c,
        Rect(x, track_y - thick / 2., w, thick),
        3.,
        (0x6f849a, 0.53),
    );
    if total.is_some() {
        if frac > 0. {
            round(c, Rect(x, track_y - thick / 2., w * frac, thick), 3.);
            gradient(
                c,
                (x, 0.),
                (grip_x.max(x + 1.), 0.),
                &[(0., (0x5887b2, 1.)), (0.72, (0x83c9ef, 1.)), (1., PALE)],
            );
            fill(c);
        }
        let rad = if held { 7. } else { 5. };
        glow(c, grip_x, track_y, rad + 5., (0x9fddff, 0.35));
        circle(c, grip_x, track_y, rad + 0.9, (0x132b40, 0.65));
        circle(c, grip_x, track_y, rad, PALE);
    }

    let stem = s
        .path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|p| p.to_string_lossy());
    let demo_title = s
        .demo_id
        .as_deref()
        .and_then(|id| crate::demos::by_id(id).ok())
        .map(|d| d.title.as_str());
    let title = notice
        .or(demo_title)
        .unwrap_or_else(|| stem.as_deref().unwrap_or("尚未选择影片"));
    text(
        c,
        title,
        Rect(122., 405., 265., 35.),
        22.,
        Face::Strong,
        false,
        if notice.is_some() {
            (0xffb9ad, 1.)
        } else {
            WHITE
        },
    );
    let elapsed = timestamp(s.position);
    let duration = total.map(timestamp).unwrap_or_else(|| "--:--".into());
    let stamp = format!("{elapsed} / {duration}");
    let width = text(
        c,
        &stamp,
        Rect(123., 441., 211., 29.),
        17.,
        Face::Digits,
        false,
        MUTED,
    );
    let meta_x = 123. + width + 15.;
    if s.path.is_some() {
        circle(c, meta_x, 456., 1.5, (0x90abc3, 0.80));
        let projection = match s.projection {
            Projection::Flat => "平面",
            Projection::Equirect180 => "180°",
            Projection::Equirect360 => "360°",
        };
        let layout = match s.stereo {
            Stereo::Mono => "2D",
            Stereo::Sbs => "SBS",
            Stereo::TopBottom => "TB",
        };
        text(
            c,
            &format!("{projection} · {layout}"),
            Rect(meta_x + 10., 441., (387. - meta_x).max(1.), 29.),
            16.,
            Face::Normal,
            false,
            DIM,
        );
    }

    // The central transport is the only raised surface on the bar.
    let (cx, cy) = (512., 440.);
    let hot = g.hot(Action::Play);
    glow(c, cx, cy, 47., (0x3f86bc, if hot { 0.31 } else { 0.17 }));
    circle(c, cx, cy + 1.7, 34., (0x071522, 0.88));
    circle(c, cx, cy, 33.6, (0xa5cde9, 0.39));
    circle(c, cx, cy, 32.5, (0x263e50, 1.));
    c.new_sub_path();
    c.arc(cx, cy, 29.3, 0., TAU);
    gradient(
        c,
        (cx - 10., cy - 31.),
        (cx + 9., cy + 30.),
        &[
            (0., (0xf0faff, 1.)),
            (0.38, (0xc5e8f9, 1.)),
            (1., if hot { (0x98d5ff, 1.) } else { (0x80b9df, 1.) }),
        ],
    );
    fill(c);
    c.new_sub_path();
    c.arc(cx, cy, 28.6, PI + 0.12, TAU - 0.2);
    ink(c, (0xffffff, 0.68));
    stroke(c, 1.05);
    glyph(
        c,
        if s.playing { Icon::Pause } else { Icon::Play },
        cx + if s.playing { 0. } else { 1. },
        cy,
        22.8,
        INK,
    );

    line(c, &[(823.5, 425.), (823.5, 456.)], 1., (0xbcd7f1, 0.21));
    let Rect(vx, vy, vw, vh) = SOUND;
    let middle = vy + vh / 2.;
    let level = s.volume.clamp(0., 1.);
    let thumb = vx + vw * level;
    shape(c, Rect(vx, middle - 2., vw, 4.), 2., (0x597186, 0.8));
    shape(c, Rect(vx, middle - 2., vw * level, 4.), 2., (0xb3d5ea, 1.));
    circle(
        c,
        thumb,
        middle,
        if g.hot(Action::Volume) { 5.8 } else { 4.4 },
        PALE,
    );

    if total.is_some() {
        g.register(SEEK, Action::Timeline);
    }
    g.register(Rect(471., 400., 82., 80.), Action::Play);
    g.icon_button(
        Rect(401., 407., 64., 66.),
        Icon::Previous,
        Action::Previous,
        false,
        23.,
    );
    g.icon_button(
        Rect(559., 407., 64., 66.),
        Icon::Next,
        Action::Next,
        false,
        23.,
    );
    g.icon_button(
        Rect(655., 411., 55., 61.),
        if level < 0.001 {
            Icon::Silent
        } else {
            Icon::Speaker
        },
        Action::Mute,
        false,
        26.,
    );
    g.register(SOUND, Action::Volume);
    g.icon_button(
        Rect(835., 411., 54., 61.),
        Icon::Folder,
        Action::Tab(Tab::Library),
        tab == Tab::Library,
        25.,
    );
    g.icon_button(
        Rect(892., 411., 55., 61.),
        Icon::Tune,
        Action::Tab(Tab::Picture),
        tab == Tab::Picture,
        25.,
    );
    g.icon_button(
        Rect(950., 411., 43., 61.),
        Icon::Hide,
        Action::Hide,
        false,
        21.,
    );
}

const LIBRARY: Rect = Rect(347., 22., 647., 323.);
const PICTURE: Rect = Rect(402., 22., 592., 323.);

fn popover(g: &Canvas, r: Rect, anchor: f64) {
    let Rect(x, y, w, h) = r;
    shape(&g.c, Rect(x, y + 4., w, h), 23., (0x050b13, 0.45));
    g.glass(r, 23.);
    // A short spine aligns the floating sheet with its transport glyph.
    line(
        &g.c,
        &[(anchor - 11., y + h - 1.), (anchor + 11., y + h - 1.)],
        2.,
        (0xb0def6, 0.87),
    );
}
fn library(g: &mut Canvas, lib: &Library) {
    popover(g, LIBRARY, 862.);
    let c = &g.c;
    glyph(c, Icon::Folder, 378., 55., 22., ICE);
    text(
        c,
        if lib.demos {
            "演示片库"
        } else {
            "本地片库"
        },
        Rect(400., 33., 105., 43.),
        21.,
        Face::Strong,
        false,
        WHITE,
    );
    text(
        c,
        &if lib.demos {
            format!("{} 部影片", lib.entries.len()).into()
        } else {
            lib.directory.to_string_lossy()
        },
        Rect(505., 36., 260., 39.),
        16.,
        Face::Normal,
        false,
        MUTED,
    );
    line(c, &[(367., 82.), (974., 82.)], 1., (0xc8dafa, 0.16));
    g.icon_button(
        Rect(804., 33., 45., 44.),
        Icon::Up,
        Action::Parent,
        false,
        20.,
    );
    g.icon_button(
        Rect(855., 33., 45., 44.),
        Icon::Refresh,
        Action::Refresh,
        false,
        19.,
    );
    g.icon_button(
        Rect(932., 33., 45., 44.),
        Icon::Close,
        Action::Tab(Tab::Player),
        false,
        21.,
    );
    let first = lib.page * PAGE_SIZE;
    for (i, entry) in lib.entries.iter().skip(first).take(PAGE_SIZE).enumerate() {
        let y = 91. + 47. * i as f64;
        let action = Action::Select(first + i);
        g.row_action(Rect(360., y, 619., 46.), action);
        glyph(
            &g.c,
            if entry.directory {
                Icon::Folder
            } else {
                Icon::Film
            },
            386.,
            y + 23.,
            20.,
            if entry.directory { ICE } else { MUTED },
        );
        let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
        let demo = crate::demos::for_path(&entry.path);
        text(
            &g.c,
            demo.map(|d| d.title.as_str()).unwrap_or(&name),
            Rect(409., y, if demo.is_some() { 362. } else { 491. }, 46.),
            19.,
            Face::Normal,
            false,
            WHITE,
        );
        if entry.directory {
            glyph(&g.c, Icon::Right, 952., y + 23., 15., DIM);
        } else if let Some(d) = demo {
            text(
                &g.c,
                d.format(),
                Rect(778., y, 188., 46.),
                16.5,
                Face::Digits,
                true,
                ICE,
            );
        }
    }
    if lib.entries.is_empty() {
        text(
            &g.c,
            "此目录中没有影片或子目录",
            Rect(375., 132., 592., 81.),
            20.,
            Face::Normal,
            true,
            MUTED,
        );
    }
    line(&g.c, &[(367., 285.), (974., 285.)], 1., (0xc8dafa, 0.16));
    g.row_action(Rect(361., 291., 206., 44.), Action::Demo);
    glyph(&g.c, Icon::Planet, 386., 313., 25., ICE);
    text(
        &g.c,
        if lib.demos {
            "返回本地片库"
        } else {
            "演示片库"
        },
        Rect(410., 292., 153., 42.),
        16.5,
        Face::Strong,
        false,
        ICE,
    );
    text(
        &g.c,
        &format!("{} / {}", lib.page + 1, lib.pages()),
        Rect(846., 291., 65., 44.),
        16.,
        Face::Digits,
        true,
        MUTED,
    );
    if lib.page > 0 {
        g.icon_button(
            Rect(803., 292., 41., 42.),
            Icon::Left,
            Action::Page(-1),
            false,
            18.,
        );
    }
    if lib.page + 1 < lib.pages() {
        g.icon_button(
            Rect(923., 292., 41., 42.),
            Icon::Right,
            Action::Page(1),
            false,
            18.,
        );
    }
}

fn segments(g: &mut Canvas, y: f64, label: &str, options: &[(&str, Action, bool)]) {
    text(
        &g.c,
        label,
        Rect(430., y, 104., 37.),
        18.,
        Face::Normal,
        false,
        MUTED,
    );
    let (x, w) = (549., 417.);
    shape(&g.c, Rect(x, y, w, 38.), 11., (0x091421, 0.63));
    let step = w / options.len() as f64;
    for (i, &(name, action, selected)) in options.iter().enumerate() {
        let r = Rect(x + step * i as f64, y, step, 38.);
        if selected || g.hot(action) {
            let slot = Rect(r.0 + 2., y + 2., step - 4., 34.);
            shape(
                &g.c,
                slot,
                9.,
                if selected {
                    (0x527fa0, 0.82)
                } else {
                    (0x6989a6, 0.34)
                },
            );
            if selected {
                round(&g.c, slot, 9.);
                ink(&g.c, (0xbee6ff, 0.27));
                stroke(&g.c, 0.8);
            }
        }
        text(
            &g.c,
            name,
            r,
            17.5,
            if selected { Face::Strong } else { Face::Normal },
            true,
            if selected { PALE } else { MUTED },
        );
        g.register(r, action);
    }
}
fn picture(g: &mut Canvas, s: &Status) {
    popover(g, PICTURE, 919.);
    glyph(&g.c, Icon::Tune, 433., 55., 22., ICE);
    text(
        &g.c,
        "画面与影院",
        Rect(456., 33., 270., 44.),
        21.,
        Face::Strong,
        false,
        WHITE,
    );
    g.icon_button(
        Rect(932., 33., 45., 44.),
        Icon::Close,
        Action::Tab(Tab::Player),
        false,
        21.,
    );
    line(&g.c, &[(424., 82.), (972., 82.)], 1., (0xc8dafa, 0.16));
    segments(
        g,
        93.,
        "投影",
        &[
            (
                "平面",
                Action::Projection(Projection::Flat),
                s.projection == Projection::Flat,
            ),
            (
                "180°",
                Action::Projection(Projection::Equirect180),
                s.projection == Projection::Equirect180,
            ),
            (
                "360°",
                Action::Projection(Projection::Equirect360),
                s.projection == Projection::Equirect360,
            ),
        ],
    );
    segments(
        g,
        140.,
        "立体排列",
        &[
            (
                "单眼 2D",
                Action::Stereo(Stereo::Mono),
                s.stereo == Stereo::Mono,
            ),
            (
                "左右 SBS",
                Action::Stereo(Stereo::Sbs),
                s.stereo == Stereo::Sbs,
            ),
            (
                "上下 TB",
                Action::Stereo(Stereo::TopBottom),
                s.stereo == Stereo::TopBottom,
            ),
        ],
    );
    segments(
        g,
        187.,
        "观看环境",
        &[
            (
                "影院",
                Action::Scene(Scene::Cinema),
                s.scene == Scene::Cinema,
            ),
            ("暗场", Action::Scene(Scene::Void), s.scene == Scene::Void),
        ],
    );
    text(
        &g.c,
        "片源校正",
        Rect(430., 234., 104., 37.),
        18.,
        Face::Normal,
        false,
        MUTED,
    );
    for (x, name, action, on) in [
        (549., "交换左右眼", Action::Swap, s.swap_eyes),
        (764., "半宽／半高", Action::Squeeze, s.squeezed),
    ] {
        let r = Rect(x, 232., 199., 41.);
        g.row_action(r, action);
        let color = if on { (0x87caee, 1.) } else { (0x63798b, 0.55) };
        shape(&g.c, Rect(x + 9., 245., 29., 16.), 8., color);
        circle(
            &g.c,
            x + if on { 29. } else { 17. },
            253.,
            5.9,
            if on { INK } else { ICE },
        );
        text(
            &g.c,
            name,
            Rect(x + 48., 232., 147., 41.),
            17.,
            Face::Normal,
            false,
            if on { WHITE } else { MUTED },
        );
    }
    line(&g.c, &[(424., 285.), (972., 285.)], 1., (0xc8dafa, 0.16));
    for (x, w, icon, name, action, col) in [
        (
            419.,
            198.,
            Icon::Recenter,
            "重设正前方",
            Action::Recenter,
            ICE,
        ),
        (
            829.,
            149.,
            Icon::Exit,
            "退出影院",
            Action::Quit,
            (0xebc9c9, 1.),
        ),
    ] {
        g.row_action(Rect(x, 291., w, 44.), action);
        glyph(&g.c, icon, x + 22., 313., 20., col);
        text(
            &g.c,
            name,
            Rect(x + 43., 291., w - 49., 44.),
            17.,
            Face::Normal,
            false,
            col,
        );
    }
}

fn tooltip(g: &Canvas, tab: Tab, s: &Status) {
    let Some(action) = g.drag.or(g.hover) else {
        return;
    };
    let Some(&(Rect(x, y, w, _), _)) = g.hits.iter().find(|(_, a)| *a == action) else {
        return;
    };
    let label = match action {
        Action::Play => {
            if s.playing {
                "暂停".into()
            } else if s.ended {
                "重新播放".into()
            } else {
                "播放".into()
            }
        }
        Action::Timeline if g.drag == Some(action) => timestamp(s.position),
        Action::Timeline => "跳转进度".into(),
        Action::Volume => format!("音量  {:.0}%", s.volume * 100.),
        Action::Mute => {
            if s.volume > 0. {
                "静音".into()
            } else {
                "取消静音".into()
            }
        }
        Action::Previous => "上一部".into(),
        Action::Next => "下一部".into(),
        Action::Tab(Tab::Library) => {
            if tab == Tab::Library {
                "收起片库".into()
            } else {
                "片库".into()
            }
        }
        Action::Tab(Tab::Picture) => {
            if tab == Tab::Picture {
                "收起设置".into()
            } else {
                "画面设置".into()
            }
        }
        Action::Tab(Tab::Player) => "关闭面板".into(),
        Action::Hide => "收起控制栏".into(),
        Action::Parent => "上级目录".into(),
        Action::Refresh => "刷新".into(),
        Action::Page(-1) => "上一页".into(),
        Action::Page(1) => "下一页".into(),
        _ => return,
    };
    let anchor = if action == Action::Timeline {
        x + w * s
            .duration
            .filter(|d| *d > 0.)
            .map_or(0., |d| (s.position / d).clamp(0., 1.))
    } else {
        x + w / 2.
    };
    let tw = (label
        .chars()
        .map(|ch| if ch.is_ascii() { 9.5 } else { 17.5 })
        .sum::<f64>()
        + 26.)
        .max(58.);
    let tx = (anchor - tw / 2.).clamp(16., W as f64 - 16. - tw);
    let ty = if y < BAR.1 {
        (y + 47.).min(290.)
    } else if tab != Tab::Player && action != Action::Timeline {
        401.
    } else {
        323.
    };
    shape(&g.c, Rect(tx, ty, tw, 32.), 10., (0x0b1724, 0.98));
    round(&g.c, Rect(tx + 0.5, ty + 0.5, tw - 1., 31.), 10.);
    ink(&g.c, (0x89b6d3, 0.52));
    stroke(&g.c, 1.);
    text(
        &g.c,
        &label,
        Rect(tx + 5., ty, tw - 10., 32.),
        16.,
        Face::Normal,
        true,
        PALE,
    );
}

#[cfg(test)]
mod pixels {
    use super::*;

    #[test]
    fn artwork_and_seek_tooltip_respect_transparent_panel_regions() -> Result<()> {
        let library = Library::demo_gallery();
        let mut status = Status::initial();
        status.duration = Some(60.);
        status.position = 30.;
        let art = Artwork {
            rgba: [120, 90, 200, 128].repeat((ART_W * ART_H) as usize),
        };
        let idle =
            Panel::draw_interactive(Tab::Player, &status, &library, None, None, None, Some(&art))?;
        assert!(idle.rgba[..364 * W as usize * 4]
            .chunks_exact(4)
            .all(|p| p[3] == 0));
        let sample = (438 * W as usize + 76) * 4;
        for (got, want) in idle.rgba[sample..sample + 4].iter().zip([74, 69, 133, 255]) {
            assert!(
                got.abs_diff(want) <= 1,
                "thumbnail pixel {:?}",
                &idle.rgba[sample..sample + 4]
            );
        }
        let tooltip_center = [0.5, 339. / H as f32];
        assert!(!idle.contains(tooltip_center));
        let dragged = Panel::draw_interactive(
            Tab::Player,
            &status,
            &library,
            None,
            Some(Action::Timeline),
            Some(Action::Timeline),
            Some(&art),
        )?;
        assert!(dragged.contains(tooltip_center));
        let uv = dragged.point_for(Action::Timeline, 0.5).unwrap();
        assert_eq!(dragged.at(uv), Some((Action::Timeline, 0.5)));
        Ok(())
    }
}
