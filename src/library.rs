use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub const PAGE_SIZE: usize = 4;

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub directory: bool,
}

#[derive(Clone)]
pub struct Library {
    pub directory: PathBuf,
    pub entries: Vec<Entry>,
    pub page: usize,
    pub demos: bool,
}

pub fn default_directory() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    if let Ok(out) = Command::new("xdg-user-dir").arg("VIDEOS").output() {
        let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if out.status.success() && path.is_absolute() && path.is_dir() {
            return path;
        }
    }
    ["Videos", "视频", "影片"]
        .iter()
        .map(|n| home.join(n))
        .find(|p| p.is_dir())
        .unwrap_or(home)
}

impl Library {
    pub fn demo_gallery() -> Self {
        let entries = crate::demos::catalog()
            .iter()
            .map(|d| Entry {
                path: d.path(),
                directory: false,
            })
            .collect();
        Self {
            directory: PathBuf::from("演示片库"),
            entries,
            page: 0,
            demos: true,
        }
    }
    pub fn read(dir: &Path) -> Result<Self> {
        let directory = dir.canonicalize()?;
        let mut entries = Vec::new();
        for item in fs::read_dir(&directory)?.flatten() {
            if item.file_name().as_encoded_bytes().first() == Some(&b'.') {
                continue;
            }
            let path = item.path();
            let directory = path.is_dir();
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            if directory
                || (path.is_file()
                    && matches!(
                        ext.as_str(),
                        "mp4"
                            | "m4v"
                            | "mkv"
                            | "webm"
                            | "avi"
                            | "mov"
                            | "mpg"
                            | "mpeg"
                            | "ts"
                            | "m2ts"
                            | "ogv"
                            | "wmv"
                            | "flv"
                            | "vob"
                            | "mxf"
                    ))
            {
                entries.push(Entry { path, directory });
            }
        }
        entries.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.path.cmp(&b.path))
        });
        Ok(Self {
            directory,
            entries,
            page: 0,
            demos: false,
        })
    }

    pub fn pages(&self) -> usize {
        self.entries.len().div_ceil(PAGE_SIZE).max(1)
    }
}
