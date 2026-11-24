//! Pixel-level regression on a real Vulkan driver; Mesa lavapipe works without a headset.
use super::*;
use ash::Entry;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 100;
const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;
const EXTENT: vk::Extent2D = vk::Extent2D {
    width: WIDTH,
    height: HEIGHT,
};

struct Readback {
    extent: vk::Extent2D,
    _entry: Entry,
    instance: Instance,
    device: Device,
    physical: vk::PhysicalDevice,
    queue: vk::Queue,
    family: u32,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    buffer: vk::Buffer,
    host_memory: vk::DeviceMemory,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
}

impl Readback {
    unsafe fn new(extent: vk::Extent2D) -> Result<Self> {
        let entry = Entry::load()?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_0);
        let instance = entry.create_instance(
            &vk::InstanceCreateInfo::default().application_info(&app),
            None,
        )?;
        let (physical, family) = instance
            .enumerate_physical_devices()?
            .into_iter()
            .find_map(|physical| {
                instance
                    .get_physical_device_queue_family_properties(physical)
                    .iter()
                    .position(|p| p.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                    .map(|i| (physical, i as u32))
            })
            .context("a Vulkan graphics queue is required for the readback test")?;
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&[1.0])];
        let device = instance.create_device(
            physical,
            &vk::DeviceCreateInfo::default().queue_create_infos(&queue_info),
            None,
        )?;
        let queue = device.get_device_queue(family, 0);
        let props = instance.get_physical_device_memory_properties(physical);
        let mut r = Self {
            extent,
            _entry: entry,
            instance,
            device,
            queue,
            family,
            physical,
            image: Default::default(),
            image_memory: Default::default(),
            buffer: Default::default(),
            host_memory: Default::default(),
            pool: Default::default(),
            cmd: Default::default(),
        };
        r.image = r.device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(FORMAT)
                .extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(2)
                .samples(vk::SampleCountFlags::TYPE_1)
                .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC),
            None,
        )?;
        let alloc = |req: vk::MemoryRequirements, flags: vk::MemoryPropertyFlags| -> Result<_> {
            let (i, _) = memory_type(&props, req.memory_type_bits, flags, Default::default())?;
            Ok(r.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(i),
                None,
            )?)
        };
        r.image_memory = alloc(
            r.device.get_image_memory_requirements(r.image),
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        r.device.bind_image_memory(r.image, r.image_memory, 0)?;
        r.buffer = r.device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(u64::from(extent.width * extent.height * 8))
                .usage(vk::BufferUsageFlags::TRANSFER_DST),
            None,
        )?;
        r.host_memory = alloc(
            r.device.get_buffer_memory_requirements(r.buffer),
            vk::MemoryPropertyFlags::HOST_COHERENT | vk::MemoryPropertyFlags::HOST_VISIBLE,
        )?;
        r.device.bind_buffer_memory(r.buffer, r.host_memory, 0)?;
        r.pool = r.device.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(family),
            None,
        )?;
        r.cmd = r.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(r.pool)
                .command_buffer_count(1)
                .level(vk::CommandBufferLevel::PRIMARY),
        )?[0];
        Ok(r)
    }

    unsafe fn sample_eyes(&self, y: u32) -> Result<[[u8; 4]; 2]> {
        let pixels = self.pixels()?;
        let (w, h) = (self.extent.width, self.extent.height);
        Ok([0, 1].map(|eye| pixels[((eye * h + y) * w + w / 2) as usize]))
    }

    unsafe fn pixels(&self) -> Result<Vec<[u8; 4]>> {
        self.device
            .reset_command_pool(self.pool, Default::default())?;
        self.device
            .begin_command_buffer(self.cmd, &vk::CommandBufferBeginInfo::default())?;
        let to_copy = vk::ImageMemoryBarrier::default()
            .image(self.image)
            .subresource_range(color_range(0).layer_count(2))
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        self.device.cmd_pipeline_barrier(
            self.cmd,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::PipelineStageFlags::TRANSFER,
            Default::default(),
            &[],
            &[],
            &[to_copy],
        );
        let copy = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(2),
            )
            .image_extent(vk::Extent3D {
                width: self.extent.width,
                height: self.extent.height,
                depth: 1,
            });
        self.device.cmd_copy_image_to_buffer(
            self.cmd,
            self.image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            self.buffer,
            &[copy],
        );
        let to_cpu = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ);
        self.device.cmd_pipeline_barrier(
            self.cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::HOST,
            Default::default(),
            &[to_cpu],
            &[],
            &[],
        );
        self.device.end_command_buffer(self.cmd)?;
        self.device.queue_submit(
            self.queue,
            &[vk::SubmitInfo::default().command_buffers(&[self.cmd])],
            vk::Fence::null(),
        )?;
        self.device.queue_wait_idle(self.queue)?;
        let p = self
            .device
            .map_memory(self.host_memory, 0, vk::WHOLE_SIZE, Default::default())?
            .cast::<u8>();
        let colors = slice::from_raw_parts(
            p.cast(),
            (self.extent.width * self.extent.height * 2) as usize,
        )
        .to_vec();
        self.device.unmap_memory(self.host_memory);
        Ok(colors)
    }
}

impl Drop for Readback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.host_memory, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.image_memory, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[test]
#[ignore = "requires a Vulkan ICD, e.g. Mesa lavapipe"]
fn stereo_pixels() -> Result<()> {
    let target = unsafe { Readback::new(EXTENT)? };
    let mut renderer = Renderer::new(
        &target.instance,
        &target.device,
        target.physical,
        target.family,
        target.queue,
        FORMAT,
        EXTENT,
        &[target.image],
    )?;
    let front = Eye {
        orientation: [0., 0., 0., 1.],
        position: [0.; 3],
        fov: [-0.8, 0.8, 0.65, -0.65],
    };
    let rear = Eye {
        orientation: [0., 1., 0., 0.],
        ..front
    };
    let mut s = Settings {
        projection: 0,
        stereo: 1,
        swap_eyes: false,
        cinema: true,
        panel_visible: false,
        panel_opacity: 1.0,
        hands: Default::default(),
        aspect: 1.8,
    };
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let packed = [red, green].concat();
    let gray = [128, 128, 128, 255];
    let blank = [3, 4, 7, 255];
    let center = HEIGHT / 2;
    let mut check = |label: &str,
                     view: Eye,
                     settings: Settings,
                     frame: Option<Pixels<'_>>,
                     panel: Option<Pixels<'_>>,
                     row: u32,
                     expected: [[u8; 4]; 2]|
     -> Result<()> {
        renderer.render(0, [view; 2], settings, frame, panel)?;
        let colors = unsafe { target.sample_eyes(row)? };
        for (got, want) in colors.iter().zip(expected) {
            assert!(
                got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 1),
                "{label}: {colors:?}, expected {expected:?}"
            );
        }
        Ok(())
    };
    check(
        "side-by-side",
        front,
        s,
        Some(Pixels {
            width: 2,
            height: 1,
            rgba: &packed,
        }),
        None,
        center,
        [red, green],
    )?;
    check(
        "swapped",
        front,
        Settings {
            swap_eyes: true,
            ..s
        },
        None,
        None,
        center,
        [green, red],
    )?;
    s.stereo = 2;
    check(
        "top-bottom",
        front,
        s,
        Some(Pixels {
            width: 1,
            height: 2,
            rgba: &packed,
        }),
        None,
        center,
        [red, green],
    )?;
    check(
        "360 rear",
        rear,
        Settings { projection: 2, ..s },
        None,
        None,
        center,
        [red, green],
    )?;
    s.projection = 1;
    s.cinema = false;
    check("180 rear", rear, s, None, None, center, [blank; 2])?;
    check("180 front", front, s, None, None, center, [red, green])?;
    // A transparent canvas above an opaque bar, as drawn by the player.
    let bar = [[0; 4], [0; 4], [0; 4], [0; 4], [0; 4], [0; 4], blue, blue].concat();
    let hud = Settings {
        panel_visible: true,
        ..s
    };
    check(
        "HUD bar below screen",
        front,
        hud,
        None,
        Some(Pixels {
            width: 1,
            height: 8,
            rgba: &bar,
        }),
        69,
        [blue; 2],
    )?;
    check(
        "transparent HUD above bar",
        front,
        hud,
        None,
        None,
        center,
        [red, green],
    )?;
    check(
        "HUD fading",
        front,
        Settings {
            panel_opacity: 0.5,
            ..hud
        },
        None,
        None,
        69,
        [[188, 0, 188, 255], [0, 188, 188, 255]],
    )?;
    check(
        "fully faded HUD",
        front,
        Settings {
            panel_opacity: 0.0,
            ..hud
        },
        None,
        None,
        69,
        [red, green],
    )?;
    s.stereo = 0;
    s.projection = 0;
    check(
        "mono sRGB",
        front,
        s,
        Some(Pixels {
            width: 1,
            height: 1,
            rgba: &gray,
        }),
        None,
        center,
        [gray; 2],
    )?;
    // Four longitude sectors with the same landmark on both sides of the 360° seam.
    let mut compass = Vec::new();
    for x in 0..16 {
        compass.extend(if !(2..14).contains(&x) {
            gray
        } else if x < 6 {
            red
        } else if x < 10 {
            green
        } else {
            blue
        });
    }
    s.projection = 2;
    s.cinema = true;
    check(
        "front longitude",
        front,
        s,
        Some(Pixels {
            width: 16,
            height: 1,
            rgba: &compass,
        }),
        None,
        center,
        [green; 2],
    )?;
    for (yaw, color) in [
        (90.0_f32, red),
        (-90.0, blue),
        (179.0, gray),
        (-179.0, gray),
    ] {
        let angle = 0.5 * yaw.to_radians();
        let turned = Eye {
            orientation: [0., angle.sin(), 0., angle.cos()],
            ..front
        };
        check(
            "panorama longitude",
            turned,
            s,
            None,
            None,
            center,
            [color; 2],
        )?;
    }
    renderer.render(0, [rear; 2], Settings { projection: 1, ..s }, None, None)?;
    let cinema = unsafe { target.sample_eyes(center)? };
    assert!(cinema
        .iter()
        .all(|rgba| rgba[..3].iter().any(|value| *value > 8)));
    Ok(())
}

// Align local -Z with the target, then roll the wrist about that same axis.
// Place the lens, rather than the grip origin, at the requested position.
fn pointing_hand(lens: [f32; 3], end: [f32; 3], roll: f32, active: bool) -> HandVisual {
    use crate::hands::{Pose, Ray, RayState, EMITTER};
    let delta: [f32; 3] = std::array::from_fn(|i| end[i] - lens[i]);
    let length = delta.iter().map(|x| x * x).sum::<f32>().sqrt();
    let [dx, dy, dz] = delta.map(|x| x / length);
    let scale = (2. * (1. - dz)).sqrt();
    let (x, y, w) = (dy / scale, -dx / scale, (1. - dz) / scale);
    let (s, c) = (0.5 * roll).sin_cos();
    let mut grip = Pose {
        position: [0.; 3],
        orientation: [x * c + y * s, y * c - x * s, w * s, w * c],
    };
    let offset = grip.transform_point(EMITTER);
    grip.position = std::array::from_fn(|i| lens[i] - offset[i]);
    HandVisual {
        grip: Some(grip),
        ray: Some(Ray {
            origin: grip.transform_point(EMITTER),
            end: if active {
                end
            } else {
                std::array::from_fn(|i| lens[i] + delta[i] / length * crate::hands::SECONDARY_REACH)
            },
            state: if active {
                RayState::Hover
            } else {
                RayState::Idle
            },
            active,
        }),
        ..HandVisual::default()
    }
}

#[test]
#[ignore = "requires a Vulkan ICD"]
fn controller_pixels() -> Result<()> {
    use crate::hands::{Pose, Ray, RayState};
    let extent = vk::Extent2D {
        width: 480,
        height: 300,
    };
    let target = unsafe { Readback::new(extent)? };
    let mut renderer = Renderer::new(
        &target.instance,
        &target.device,
        target.physical,
        target.family,
        target.queue,
        FORMAT,
        extent,
        &[target.image],
    )?;
    let eyes = [-0.032, 0.032].map(|x| Eye {
        position: [x, 0., 0.],
        orientation: [0., 0., 0., 1.],
        fov: [-0.66, 0.66, 0.45, -0.45],
    });
    let mut settings = Settings {
        projection: 0,
        stereo: 0,
        swap_eyes: false,
        cinema: false,
        panel_visible: false,
        panel_opacity: 1.,
        hands: Default::default(),
        aspect: 16. / 9.,
    };
    let video_black = [0, 0, 0, 255];
    renderer.render(
        0,
        eyes,
        settings,
        Some(Pixels {
            width: 1,
            height: 1,
            rgba: &video_black,
        }),
        None,
    )?;
    let baseline = unsafe { target.pixels()? };
    let per_eye = (extent.width * extent.height) as usize;
    let left = HandVisual {
        grip: Some(Pose {
            position: [-0.195, -0.135, -0.49],
            ..Pose::default()
        }),
        ..HandVisual::default()
    };
    settings.hands[0] = left;
    renderer.render(0, eyes, settings, None, None)?;
    let model = unsafe { target.pixels()? };
    let centroids = [0, 1].map(|eye| {
        let mut count = 0;
        let mut sum = 0.;
        for i in 0..per_eye {
            let at = eye * per_eye + i;
            if model[at] != baseline[at] {
                count += 1;
                sum += (i % extent.width as usize) as f32;
            }
        }
        assert!(count > per_eye / 1500, "missing controller in eye {eye}");
        sum / count as f32 / extent.width as f32
    });
    assert!(
        centroids[0] < 0.5 && centroids[1] > 0.05 && centroids[0] - centroids[1] > 0.04,
        "incorrect world-space stereo disparity {centroids:?}"
    );
    // Track aim independently of grip. The tip should land at the projection
    // of one world-space point in each eye, including when there is no model.
    let ray = Ray {
        origin: [0.19, -0.11, -0.54],
        end: [0., 0., -2.],
        state: RayState::Hover,
        active: true,
    };
    settings.hands = [
        HandVisual::default(),
        HandVisual {
            ray: Some(ray),
            ..HandVisual::default()
        },
    ];
    renderer.render(0, eyes, settings, None, None)?;
    let laser = unsafe { target.pixels()? };
    for (index, eye) in eyes.iter().enumerate() {
        let u = 0.5 - eye.position[0] / (4. * eye.fov[1].tan());
        let x = (u * extent.width as f32) as i32;
        let y = extent.height as i32 / 2;
        let radius = (extent.height / 80) as i32;
        let change = (-radius..=radius).any(|dy| {
            (-radius..=radius).any(|dx| {
                let offset =
                    index * per_eye + (y + dy) as usize * extent.width as usize + (x + dx) as usize;
                laser[offset] != baseline[offset]
            })
        });
        assert!(
            change,
            "missing laser at predicted eye {index} endpoint ({x}, {y})"
        );
    }
    // Compare frames with identical geometry and eye poses, isolating only the light.
    let hand = pointing_hand([0.33, -0.11, -0.65], [0., -1.5, -6.12], 0.13, true);
    let ray = hand.ray.unwrap();
    settings.hands = [HandVisual::default(), HandVisual { ray: None, ..hand }];
    renderer.render(0, eyes, settings, None, None)?;
    let unlit = unsafe { target.pixels()? };
    settings.hands[1] = hand;
    renderer.render(0, eyes, settings, None, None)?;
    let hover = unsafe { target.pixels()? };
    // Removing the grip reveals the part of the beam that the real housing
    // can legitimately hide when seen from behind the optical window.
    settings.hands[1].grip = None;
    renderer.render(0, eyes, settings, None, None)?;
    let light_without_housing = unsafe { target.pixels()? };
    settings.hands[1] = hand;
    settings.hands[1].ray.as_mut().unwrap().state = RayState::Pressed;
    renderer.render(0, eyes, settings, None, None)?;
    let pressed = unsafe { target.pixels()? };
    let delta: [f32; 3] = std::array::from_fn(|i| ray.end[i] - ray.origin[i]);
    let length = delta.iter().map(|x| x * x).sum::<f32>().sqrt();
    for (side, eye) in eyes.iter().enumerate() {
        let change = |p: [f32; 3], a: &[[u8; 4]], b: &[[u8; 4]], radius: i32| {
            let [left, right, top, bottom] = eye.fov.map(f32::tan);
            let depth = eye.position[2] - p[2];
            let x = (((p[0] - eye.position[0]) / depth - left) / (right - left)
                * extent.width as f32) as i32;
            let y = (((p[1] - eye.position[1]) / depth - top) / (bottom - top)
                * extent.height as f32) as i32;
            let mut contrast = 0;
            for row in y - radius..=y + radius {
                for col in x - radius..=x + radius {
                    let at = side * per_eye + row as usize * extent.width as usize + col as usize;
                    for rgb in 0..3 {
                        contrast = contrast.max(a[at][rgb].abs_diff(b[at][rgb]));
                    }
                }
            }
            contrast
        };
        for distance in [0.002, 0.008, 0.025, 0.055, 0.12] {
            let point = std::array::from_fn(|i| ray.origin[i] + delta[i] * distance / length);
            assert!(
                change(point, &light_without_housing, &baseline, 1) > 65,
                "beam fades out {distance} metres from the emitting glass in eye {side}"
            );
            let beam_visible = change(point, &hover, &unlit, 1) > 25;
            if distance >= 0.055 {
                assert!(beam_visible, "missing exposed beam in eye {side}");
            } else {
                assert!(
                    beam_visible || change(point, &unlit, &baseline, 1) > 65,
                    "detached beam beyond emitter in eye {side}"
                );
            }
        }
        assert!(
            change(ray.end, &hover, &unlit, 2) > 30,
            "missing hit in eye {side}"
        );
        assert!(
            change(ray.end, &hover, &pressed, 4) > 12,
            "no press feedback in eye {side}"
        );
    }
    // Removing tracked poses must clear both layers, including the cursor.
    settings.hands = Default::default();
    renderer.render(0, eyes, settings, None, None)?;
    assert!(unsafe { target.pixels()? } == baseline);

    if let Some(preview) = std::env::var_os("XR_CINEMA_HAND_PREVIEW") {
        use crate::{
            command::Status,
            hands::panel_point,
            library::Library,
            panel::{self, Action, Panel, Tab},
        };
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let media = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/环行星.webm");
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-threads", "1", "-ss", "4", "-i"])
            .arg(&media)
            .args([
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .output()?;
        ensure!(
            decoded.status.success() && decoded.stdout.len() == 1280 * 720 * 4,
            "could not decode demo preview frame"
        );
        let status = Status {
            path: Some(media),
            position: 4.,
            duration: Some(12.),
            playing: false,
            ..Status::initial()
        };
        let mut artwork = panel::Artwork {
            rgba: Vec::with_capacity((panel::ART_W * panel::ART_H * 4) as usize),
        };
        for y in 0..panel::ART_H {
            for x in 0..panel::ART_W {
                let index =
                    ((y * 720 / panel::ART_H * 1280 + x * 1280 / panel::ART_W) * 4) as usize;
                artwork
                    .rgba
                    .extend_from_slice(&decoded.stdout[index..index + 4]);
            }
        }
        let panel = Panel::draw_interactive(
            Tab::Player,
            &status,
            &Library::demo_gallery(),
            None,
            Some(Action::Play),
            None,
            Some(&artwork),
        )?;
        let target_button =
            panel_point(panel.point_for(Action::Play, 0.5).context("play control")?);
        settings.cinema = true;
        settings.panel_visible = true;
        settings.hands = [
            pointing_hand([-0.37, -0.105, -0.71], [-0.19, 0.02, -1.95], -0.22, false),
            pointing_hand([0.38, -0.113, -0.72], target_button, 0.25, true),
        ];
        let extent = vk::Extent2D {
            width: 1920,
            height: 1200,
        };
        let target = unsafe { Readback::new(extent)? };
        let mut renderer = Renderer::new(
            &target.instance,
            &target.device,
            target.physical,
            target.family,
            target.queue,
            FORMAT,
            extent,
            &[target.image],
        )?;
        let eyes = eyes.map(|e| Eye {
            fov: [-0.73, 0.73, 0.51, -0.51],
            ..e
        });
        renderer.render(
            0,
            eyes,
            settings,
            Some(Pixels {
                width: 1280,
                height: 720,
                rgba: &decoded.stdout,
            }),
            Some(Pixels {
                width: panel::W,
                height: panel::H,
                rgba: &panel.rgba,
            }),
        )?;
        let image = unsafe { target.pixels()? };
        let mut encode = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgba",
                "-video_size",
            ])
            .arg(format!("{}x{}", extent.width, extent.height))
            .args(["-i", "pipe:0", "-frames:v", "1", "-threads", "1"])
            .arg(preview)
            .stdin(Stdio::piped())
            .spawn()?;
        let bytes =
            unsafe { slice::from_raw_parts(image.as_ptr().cast::<u8>(), image.len() / 2 * 4) };
        encode.stdin.take().unwrap().write_all(bytes)?;
        ensure!(
            encode.wait()?.success(),
            "could not encode controller preview"
        );
    }
    Ok(())
}
