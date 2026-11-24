use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    #[default]
    Flat,
    Equirect180,
    Equirect360,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stereo {
    #[default]
    Mono,
    Sbs,
    TopBottom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scene {
    #[default]
    Cinema,
    Void,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    OpenDemo {
        #[serde(default)]
        id: Option<String>,
    },
    Open {
        path: PathBuf,
    },
    Play,
    Pause,
    Seek {
        seconds: f64,
    },
    Volume {
        level: f64,
    },
    SetProjection {
        projection: Projection,
    },
    SetStereo {
        stereo: Stereo,
    },
    SetSqueezed {
        enabled: bool,
    },
    SwapEyes {
        enabled: bool,
    },
    SetScene {
        scene: Scene,
    },
    ShowPanel {
        visible: bool,
    },
    Recenter,
    Next,
    Previous,
    Quit,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    pub ready: bool,
    pub path: Option<PathBuf>,
    pub demo_id: Option<String>,
    pub playing: bool,
    pub ended: bool,
    pub position: f64,
    pub duration: Option<f64>,
    pub volume: f64,
    pub projection: Projection,
    pub stereo: Stereo,
    pub squeezed: bool,
    pub swap_eyes: bool,
    pub scene: Scene,
    pub panel_visible: bool,
    pub decoded_frames: u64,
    pub submitted_frames: u64,
    pub controllers: [crate::hands::ControllerStatus; 2],
    pub video_size: Option<[u32; 2]>,
    pub error: Option<String>,
}

impl Status {
    pub fn initial() -> Self {
        Self {
            volume: 1.,
            panel_visible: true,
            ..Default::default()
        }
    }
    pub fn eye_aspect(&self, source_aspect: f32) -> f32 {
        match (self.stereo, self.squeezed) {
            (Stereo::Sbs, false) => source_aspect * 0.5,
            (Stereo::TopBottom, false) => source_aspect * 2.0,
            _ => source_aspect,
        }
    }
}
