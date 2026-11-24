use crate::command::{Projection, Stereo};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Deserialize, Serialize)]
pub struct Demo {
    pub id: String,
    pub title: String,
    file: PathBuf,
    pub projection: Projection,
    pub stereo: Stereo,
    #[serde(default)]
    pub squeezed: bool,
}

pub fn catalog() -> &'static [Demo] {
    static CATALOG: OnceLock<Vec<Demo>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/demos/catalog.json"))
            .expect("bundled demo catalog")
    })
}

fn asset_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let installed = std::env::current_exe()
            .ok()
            .and_then(|p| Some(p.parent()?.parent()?.join("share/xr-cinema")));
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let root = installed
            .filter(|p| p.join("demos/catalog.json").is_file())
            .unwrap_or(source);
        root.canonicalize().unwrap_or(root)
    })
}

impl Demo {
    pub fn path(&self) -> PathBuf {
        asset_root().join(&self.file)
    }
    pub fn format(&self) -> &'static str {
        match (self.projection, self.stereo) {
            (Projection::Flat, Stereo::Mono) => "2D",
            (Projection::Flat, Stereo::Sbs) => "3D · SBS",
            (Projection::Flat, Stereo::TopBottom) => "3D · TB",
            (Projection::Equirect180, Stereo::Mono) => "180°",
            (Projection::Equirect360, Stereo::Mono) => "360°",
            (Projection::Equirect180, Stereo::Sbs) => "180° · SBS",
            (Projection::Equirect180, Stereo::TopBottom) => "180° · TB",
            (Projection::Equirect360, Stereo::Sbs) => "360° · SBS",
            (Projection::Equirect360, Stereo::TopBottom) => "360° · TB",
        }
    }
}

pub fn by_id(id: &str) -> Result<&'static Demo> {
    catalog()
        .iter()
        .find(|d| d.id == id)
        .with_context(|| format!("未知演示片 {id}，请使用 --list-demos 查看"))
}
pub fn for_path(file: &Path) -> Option<&'static Demo> {
    let relative = file.strip_prefix(asset_root()).ok()?;
    catalog().iter().find(|d| d.file == relative)
}
