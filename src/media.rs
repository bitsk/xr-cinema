use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ops::Deref;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;
use std::slice;

#[repr(C)]
#[derive(Default)]
struct NativeUpdate {
    sample: *mut c_void,
    pixels: *const u8,
    width: u32,
    height: u32,
    stride: i32,
    display_aspect: f64,
    position: f64,
    duration: f64,
    playing: i32,
    ended: i32,
    error: *const c_char,
}

unsafe extern "C" {
    fn cinema_gst_init(message: *mut c_char, size: usize) -> c_int;
    fn cinema_media_open(
        path: *const c_char,
        volume: f64,
        message: *mut c_char,
        size: usize,
    ) -> *mut c_void;
    fn cinema_media_free(media: *mut c_void);
    fn cinema_media_set_playing(media: *mut c_void, playing: c_int) -> c_int;
    fn cinema_media_seek(media: *mut c_void, nanoseconds: i64) -> c_int;
    fn cinema_media_set_volume(media: *mut c_void, volume: f64);
    fn cinema_media_poll(media: *mut c_void, update: *mut NativeUpdate);
    fn cinema_sample_free(sample: *mut c_void);
}

struct Sample(NonNull<c_void>);

impl Drop for Sample {
    fn drop(&mut self) {
        unsafe { cinema_sample_free(self.0.as_ptr()) }
    }
}

enum Storage {
    // Own the mapped GstSample for precisely as long as the slice is available.
    Mapped {
        _sample: Sample,
        pixels: NonNull<u8>,
        length: usize,
    },
    Packed(Vec<u8>),
}

pub struct FramePixels(Storage);

impl Deref for FramePixels {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match &self.0 {
            Storage::Mapped { pixels, length, .. } => unsafe {
                slice::from_raw_parts(pixels.as_ptr(), *length)
            },
            Storage::Packed(data) => data,
        }
    }
}

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// Display aspect of the packed frame, including the pixel aspect ratio.
    pub display_aspect: f32,
    pub rgba: FramePixels,
}

impl VideoFrame {
    fn from_update(u: &NativeUpdate) -> Result<Option<Self>, String> {
        let Some(handle) = NonNull::new(u.sample) else {
            return Ok(None);
        };
        let sample = Sample(handle);
        let pixels = NonNull::new(u.pixels.cast_mut()).ok_or("Missing video pixels")?;
        let row = (u.width as usize).checked_mul(4).ok_or("Frame too wide")?;
        let height = u.height as usize;
        let pitch = u.stride.unsigned_abs() as usize;
        let length = row.checked_mul(height).ok_or("Frame too large")?;
        let span = pitch.checked_mul(height).ok_or("Frame pitch too large")?;
        if row == 0 || height == 0 || row > pitch || span > isize::MAX as usize {
            return Err("Invalid video frame layout".into());
        }
        let storage = if u.stride > 0 && pitch == row {
            Storage::Mapped {
                _sample: sample,
                pixels,
                length,
            }
        } else {
            let mut data = Vec::new();
            data.try_reserve_exact(length).map_err(|e| e.to_string())?;
            for y in 0..height {
                // gst_video_frame_map provides the address and signed stride of plane 0.
                let start = unsafe { u.pixels.offset((y as isize) * (u.stride as isize)) };
                data.extend_from_slice(unsafe { slice::from_raw_parts(start, row) });
            }
            Storage::Packed(data)
        };
        Ok(Some(Self {
            width: u.width,
            height: u.height,
            display_aspect: u.display_aspect as f32,
            rgba: FramePixels(storage),
        }))
    }
}

#[derive(Default)]
pub struct MediaUpdate {
    pub frame: Option<VideoFrame>,
    pub position: f64,
    pub duration: Option<f64>,
    pub playing: bool,
    pub ended: bool,
    pub error: Option<String>,
}

struct Pipeline(NonNull<c_void>);

impl Drop for Pipeline {
    fn drop(&mut self) {
        unsafe { cinema_media_free(self.0.as_ptr()) }
    }
}

pub struct Media {
    pipeline: Option<Pipeline>,
    volume: f64,
}

fn message(buffer: &[c_char]) -> String {
    // Every native error branch writes a NUL-terminated message into this buffer.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

impl Media {
    pub fn new() -> Result<Self, String> {
        let mut error = [0; 1024];
        if unsafe { cinema_gst_init(error.as_mut_ptr(), error.len()) } == 0 {
            return Err(message(&error));
        }
        Ok(Self {
            pipeline: None,
            volume: 1.0,
        })
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        if !path.is_file() {
            return Err("Please select a local video file".into());
        }
        let filename = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let mut error = [0; 1024];
        let raw = unsafe {
            cinema_media_open(
                filename.as_ptr(),
                self.volume,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let pipeline = NonNull::new(raw).ok_or_else(|| message(&error))?;
        self.pipeline = Some(Pipeline(pipeline));
        Ok(())
    }

    fn handle(&self) -> Result<*mut c_void, String> {
        self.pipeline
            .as_ref()
            .map(|p| p.0.as_ptr())
            .ok_or_else(|| "No video is open".into())
    }

    pub fn set_playing(&mut self, playing: bool) -> Result<(), String> {
        if unsafe { cinema_media_set_playing(self.handle()?, playing.into()) } == 0 {
            return Err("Cannot change playback state".into());
        }
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds < 0.0 || seconds >= i64::MAX as f64 * 1e-9 {
            return Err("Invalid playback position".into());
        }
        if unsafe { cinema_media_seek(self.handle()?, (seconds * 1e9).round() as i64) } == 0 {
            return Err("This video cannot seek at the requested time".into());
        }
        Ok(())
    }

    pub fn set_volume(&mut self, volume: f64) -> Result<(), String> {
        if !(0.0..=1.0).contains(&volume) {
            return Err("Volume must be between 0 and 1".into());
        }
        if let Some(p) = &self.pipeline {
            unsafe { cinema_media_set_volume(p.0.as_ptr(), volume) };
        }
        self.volume = volume;
        Ok(())
    }

    pub fn poll(&mut self) -> MediaUpdate {
        let Some(pipeline) = &self.pipeline else {
            return MediaUpdate::default();
        };
        let mut native = NativeUpdate::default();
        unsafe { cinema_media_poll(pipeline.0.as_ptr(), &mut native) };
        let mut update = MediaUpdate {
            position: native.position,
            duration: (native.duration >= 0.0).then_some(native.duration),
            playing: native.playing != 0,
            ended: native.ended != 0,
            ..MediaUpdate::default()
        };
        if !native.error.is_null() {
            update.error = Some(
                unsafe { CStr::from_ptr(native.error) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        match VideoFrame::from_update(&native) {
            Ok(frame) => update.frame = frame,
            Err(error) => {
                self.pipeline = None;
                update.playing = false;
                update.error = Some(error);
            }
        }
        update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for(media: &mut Media, predicate: impl Fn(&MediaUpdate) -> bool) -> MediaUpdate {
        let start = Instant::now();
        loop {
            let update = media.poll();
            assert!(update.error.is_none(), "{:?}", update.error);
            if predicate(&update) {
                return update;
            }
            assert!(
                start.elapsed() < Duration::from_secs(8),
                "playback timed out at {} seconds",
                update.position
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    // Run with XR_CINEMA_AUDIO_SINK=fakesink cargo test video_playback -- --ignored.
    #[test]
    #[ignore = "requires ffmpeg plus GStreamer VP8/Vorbis/Matroska plugins"]
    fn video_playback() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp =
            TempDir(std::env::temp_dir().join(format!("xr-cinema-{}-{nonce}", std::process::id())));
        fs::create_dir(&temp.0).unwrap();
        let clip = temp.0.join("影院 #1.webm");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=96x64:rate=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440",
                "-t",
                "3",
                "-vf",
                "setsar=4/3",
                "-c:v",
                "libvpx",
                "-c:a",
                "libvorbis",
            ])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(status.success());

        let mut media = Media::new().unwrap();
        assert!(media.set_playing(true).is_err());
        media.set_volume(0.5).unwrap();
        assert!(media.set_volume(f64::NAN).is_err());
        media.open(&clip).unwrap();
        let first = wait_for(&mut media, |u| u.frame.is_some() && u.duration.is_some());
        assert!(first.playing);
        assert!((first.duration.unwrap() - 3.0).abs() < 0.15);
        let first_frame = first.frame.unwrap();
        assert_eq!((first_frame.width, first_frame.height), (96, 64));
        assert!((first_frame.display_aspect - 2.0).abs() < 0.01);
        assert_eq!(first_frame.rgba.len(), 96 * 64 * 4);
        assert!(first_frame.rgba.chunks_exact(4).all(|p| p[3] == 255));
        let first_pixels = first_frame.rgba.to_vec();
        assert!(first_pixels.windows(2).any(|p| p[0] != p[1]));

        wait_for(&mut media, |u| u.position >= 0.2);
        media.set_playing(false).unwrap();
        thread::sleep(Duration::from_millis(170));
        let paused = media.poll();
        thread::sleep(Duration::from_millis(200));
        assert!(!paused.playing && (media.poll().position - paused.position).abs() < 0.03);
        media.seek(2.0).unwrap();
        let after_seek = wait_for(&mut media, |u| {
            u.frame.is_some() && (u.position - 2.0).abs() < 0.15
        });
        assert!(!after_seek.playing);
        assert_ne!(&*after_seek.frame.unwrap().rgba, first_pixels.as_slice());
        media.set_playing(true).unwrap();
        let ended = wait_for(&mut media, |u| u.ended);
        assert!(!ended.playing && ended.position > 2.8);
        media.set_playing(true).unwrap();
        wait_for(&mut media, |u| {
            u.playing && u.position < 1.0 && u.frame.is_some()
        });

        let broken = temp.0.join("broken.webm");
        fs::write(&broken, b"uncodable video").unwrap();
        if media.open(&broken).is_ok() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while media.poll().error.is_none() {
                assert!(Instant::now() < deadline, "missing decoder error");
                thread::sleep(Duration::from_millis(10));
            }
        }
        media.open(&clip).unwrap();
        wait_for(&mut media, |u| u.frame.is_some());
        // Mapped samples remain readable across seeking and replacement of their pipeline.
        assert_eq!(&*first_frame.rgba, first_pixels.as_slice());
    }
}
