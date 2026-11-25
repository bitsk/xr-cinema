use crate::renderer::{Eye, Renderer};
use crate::App;
use anyhow::{anyhow, bail, ensure, Context, Result};
use ash::{vk, vk::Handle};
use openxr as xr;
use std::{ffi::CStr, thread, time::Duration};

#[path = "xr_input.rs"]
mod controls;

const STEREO: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    PointerDown([f32; 2]),
    PointerMove([f32; 2]),
    PointerUp(Option<[f32; 2]>),
    PointerCancel,
    TogglePlayback,
    TogglePanel,
    Seek(f64),
    AdjustVolume(f64),
    Recenter,
    Next,
    Previous,
}

// Keep the loader alive through the last Vulkan destructor, including early
// returns during graphics device creation.
struct VkInstance {
    api: ash::Instance,
    _loader: ash::Entry,
}

impl Drop for VkInstance {
    fn drop(&mut self) {
        unsafe { self.api.destroy_instance(None) }
    }
}

struct Graphics {
    instance: VkInstance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    family: u32,
}

impl Drop for Graphics {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
        }
    }
}

impl Graphics {
    fn new(instance: &xr::Instance, system: xr::SystemId) -> Result<Self> {
        let requirements = instance.graphics_requirements::<xr::Vulkan>(system)?;
        // enable2 lets the runtime select its physical device and required
        // instance/device extensions. No desktop surface or WSI is involved.
        unsafe {
            let loader = ash::Entry::load().context("load Vulkan graphics loader")?;
            let loader_version = loader
                .try_enumerate_instance_version()?
                .unwrap_or(vk::API_VERSION_1_0);
            let loader_version = xr::Version::new(
                vk::api_version_major(loader_version) as u16,
                vk::api_version_minor(loader_version) as u16,
                0,
            );
            let required = requirements.min_api_version_supported;
            let version = xr::Version::new(required.major(), required.minor(), 0)
                .max(xr::Version::new(1, 0, 0));
            ensure!(
                version <= loader_version,
                "OpenXR requires Vulkan {version}, loader offers {loader_version}"
            );
            let info = vk::ApplicationInfo::default()
                .application_name(c"openKylin XR Cinema")
                .api_version(vk::make_api_version(
                    0,
                    version.major() as _,
                    version.minor() as _,
                    0,
                ));
            let create = vk::InstanceCreateInfo::default().application_info(&info);
            // Both bindings describe vkGetInstanceProcAddr with the Vulkan ABI.
            // Their Rust representations of the dispatchable handles differ.
            let get_proc = std::mem::transmute::<
                vk::PFN_vkGetInstanceProcAddr,
                xr::sys::platform::VkGetInstanceProcAddr,
            >(loader.static_fn().get_instance_proc_addr);
            let handle = instance
                .create_vulkan_instance(system, get_proc, &create as *const _ as *const _)?
                .map_err(vk::Result::from_raw)?;
            let host = VkInstance {
                api: ash::Instance::load(loader.static_fn(), vk::Instance::from_raw(handle as _)),
                _loader: loader,
            };
            let physical = vk::PhysicalDevice::from_raw(
                instance.vulkan_graphics_device(system, host.api.handle().as_raw() as _)? as _,
            );
            let family = host
                .api
                .get_physical_device_queue_family_properties(physical)
                .iter()
                .position(|p| p.queue_count > 0 && p.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .context("OpenXR Vulkan device has no graphics queue")?
                as u32;
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(&priorities)];
            let create = vk::DeviceCreateInfo::default().queue_create_infos(&queues);
            let handle = instance
                .create_vulkan_device(
                    system,
                    get_proc,
                    physical.as_raw() as _,
                    &create as *const _ as *const _,
                )?
                .map_err(vk::Result::from_raw)?;
            let device = ash::Device::load(host.api.fp_v1_0(), vk::Device::from_raw(handle as _));
            let queue = device.get_device_queue(family, 0);
            let props = host.api.get_physical_device_properties(physical);
            eprintln!(
                "Vulkan GPU: {}",
                CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy()
            );
            Ok(Self {
                instance: host,
                device,
                physical,
                queue,
                family,
            })
        }
    }
}

// Anchor the cinema to the user's seat. Keeping only yaw keeps the screen
// upright if the viewer is looking up or tilting their head during recenter.
fn seat_pose(view: xr::SpaceLocation) -> Option<xr::Posef> {
    if !view.location_flags.contains(
        xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID,
    ) {
        return None;
    }
    let pose = view.pose;
    let q = pose.orientation;
    let x = 2.0 * (q.x * q.z + q.w * q.y);
    let z = 1.0 - 2.0 * (q.x * q.x + q.y * q.y);
    if ![x, z, pose.position.x, pose.position.y, pose.position.z]
        .iter()
        .all(|v| v.is_finite())
        || x * x + z * z < 0.0001
    {
        return None;
    }
    let (y, w) = (x.atan2(z) * 0.5).sin_cos();
    Some(xr::Posef {
        orientation: xr::Quaternionf {
            x: 0.0,
            y,
            z: 0.0,
            w,
        },
        position: pose.position,
    })
}

fn eye(view: &xr::View) -> Eye {
    let p = view.pose.position;
    let q = view.pose.orientation;
    let f = view.fov;
    Eye {
        position: [p.x, p.y, p.z],
        orientation: [q.x, q.y, q.z, q.w],
        fov: [f.angle_left, f.angle_right, f.angle_up, f.angle_down],
    }
}

pub fn run(app: &mut App) -> Result<()> {
    let entry = unsafe { xr::Entry::load(&()).context("load libopenxr_loader")? };
    let available = entry.enumerate_extensions()?;
    ensure!(
        available.khr_vulkan_enable2,
        "runtime lacks XR_KHR_vulkan_enable2"
    );
    let mut extensions = xr::ExtensionSet::default();
    extensions.khr_vulkan_enable2 = true;
    extensions.bd_controller_interaction = available.bd_controller_interaction;
    let instance = entry.create_instance(
        &xr::ApplicationInfo {
            application_name: "openKylin XR Cinema",
            application_version: 1,
            engine_name: "XR Cinema",
            engine_version: 1,
            api_version: xr::Version::new(1, 0, 0),
        },
        &extensions,
        &[],
        &(),
    )?;
    let properties = instance.properties()?;
    eprintln!(
        "OpenXR runtime: {} {}",
        properties.runtime_name, properties.runtime_version
    );
    let system = instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .context("locate OpenXR HMD; check the runtime and headset connection")?;
    let modes = instance.enumerate_environment_blend_modes(system, STEREO)?;
    let blend = [
        xr::EnvironmentBlendMode::OPAQUE,
        xr::EnvironmentBlendMode::ALPHA_BLEND,
        xr::EnvironmentBlendMode::ADDITIVE,
    ]
    .into_iter()
    .find(|mode| modes.contains(mode))
    .context("runtime has no stereo environment blend mode")?;
    let views = instance.enumerate_view_configuration_views(system, STEREO)?;
    ensure!(views.len() == 2, "PRIMARY_STEREO must supply two views");
    let extent = vk::Extent2D {
        width: views[0]
            .recommended_image_rect_width
            .max(views[1].recommended_image_rect_width)
            .min(views[0].max_image_rect_width)
            .min(views[1].max_image_rect_width),
        height: views[0]
            .recommended_image_rect_height
            .max(views[1].recommended_image_rect_height)
            .min(views[0].max_image_rect_height)
            .min(views[1].max_image_rect_height),
    };
    let rect = xr::Rect2Di {
        offset: xr::Offset2Di { x: 0, y: 0 },
        extent: xr::Extent2Di {
            width: extent.width.try_into()?,
            height: extent.height.try_into()?,
        },
    };
    let gpu = Graphics::new(&instance, system)?;
    // Local bindings are declared in dependency order so the renderer,
    // swapchain, spaces and XR session are destroyed before the graphics device.
    let (session, mut waiter, mut stream) = unsafe {
        instance.create_session::<xr::Vulkan>(
            system,
            &xr::vulkan::SessionCreateInfo {
                instance: gpu.instance.api.handle().as_raw() as _,
                physical_device: gpu.physical.as_raw() as _,
                device: gpu.device.handle().as_raw() as _,
                queue_family_index: gpu.family,
                queue_index: 0,
            },
        )?
    };
    let base =
        session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
    let view = session.create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)?;
    let mut seat = None;
    let mut input =
        controls::Controllers::new(&instance, &session, extensions.bd_controller_interaction)?;
    let formats = session.enumerate_swapchain_formats()?;
    let format = [
        vk::Format::R8G8B8A8_SRGB,
        vk::Format::B8G8R8A8_SRGB,
        vk::Format::R8G8B8A8_UNORM,
        vk::Format::B8G8R8A8_UNORM,
    ]
    .into_iter()
    .find(|format| formats.contains(&(format.as_raw() as _)))
    .ok_or_else(|| anyhow!("runtime offers no supported 8-bit RGBA swapchain ({formats:?})"))?;
    // Separate single-layer swapchains avoid cross-process array-layer layout
    // assumptions in native display runtimes while preserving each eye's pose.
    let mut chains = Vec::new();
    let mut images = Vec::new();
    let mut offsets = Vec::new();
    for _ in 0..2 {
        let chain = session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
            format: format.as_raw() as _,
            sample_count: 1,
            width: extent.width,
            height: extent.height,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        })?;
        offsets.push(images.len());
        images.extend(
            chain
                .enumerate_images()?
                .into_iter()
                .map(|image| (vk::Image::from_raw(image), 1)),
        );
        chains.push(chain);
    }
    let mut renderer = Renderer::new(
        &gpu.instance.api,
        &gpu.device,
        gpu.physical,
        gpu.family,
        gpu.queue,
        format,
        extent,
        &images,
    )?;
    let mut events = xr::EventDataBuffer::new();
    let (mut running, mut focused, mut exiting) = (false, false, false);
    let mut recenter = true;
    let mut space_changed_at = None;
    loop {
        while let Some(event) = instance.poll_event(&mut events)? {
            match event {
                xr::Event::SessionStateChanged(e) => {
                    focused = e.state() == xr::SessionState::FOCUSED;
                    if !focused {
                        input.reset(app);
                    }
                    match e.state() {
                        xr::SessionState::READY => {
                            session.begin(STEREO)?;
                            running = true;
                        }
                        xr::SessionState::STOPPING => {
                            session.end()?;
                            running = false;
                            if exiting {
                                return Ok(());
                            }
                        }
                        xr::SessionState::EXITING => return Ok(()),
                        xr::SessionState::LOSS_PENDING => bail!("OpenXR session lost"),
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => bail!("OpenXR runtime lost"),
                xr::Event::ReferenceSpaceChangePending(e)
                    if e.reference_space_type() == xr::ReferenceSpaceType::LOCAL =>
                {
                    space_changed_at = Some(e.change_time().as_nanos());
                }
                _ => {}
            }
        }
        app.tick()?;
        recenter |= app.take_recenter();
        if app.shutdown_requested() && !exiting {
            if !running {
                return Ok(());
            }
            session.request_exit()?;
            exiting = true;
        }
        if !running {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        let frame = waiter.wait()?;
        stream.begin()?;
        let time = frame.predicted_display_time;
        if !frame.should_render || exiting {
            input.reset(app);
            stream.end(time, blend, &[])?;
            continue;
        }
        if space_changed_at.is_some_and(|change| time.as_nanos() >= change) {
            space_changed_at = None;
            recenter = true;
            input.reset(app);
        }
        if recenter {
            if let Some(pose) = seat_pose(view.locate(&base, time)?) {
                seat = Some(session.create_reference_space(xr::ReferenceSpaceType::LOCAL, pose)?);
                recenter = false;
                input.reset(app);
            }
        }
        let space = seat.as_ref().unwrap_or(&base);
        let (flags, views) = session.locate_views(STEREO, time, space)?;
        if !flags
            .contains(xr::ViewStateFlags::ORIENTATION_VALID | xr::ViewStateFlags::POSITION_VALID)
        {
            input.reset(app);
            stream.end(time, blend, &[])?;
            continue;
        }
        ensure!(views.len() == 2, "runtime changed the stereo view count");
        let controls = if focused {
            input.poll(&session, space, time, app)?
        } else {
            Default::default()
        };
        app.hover(controls.pointer);
        let settings = app.settings(controls.hands);
        let (video, panel) = app.textures();
        let left = chains[0].acquire_image()?;
        chains[0].wait_image(xr::Duration::INFINITE)?;
        let right = match chains[1].acquire_image() {
            Ok(image) => image,
            Err(error) => {
                let _ = chains[0].release_image();
                return Err(error.into());
            }
        };
        chains[1].wait_image(xr::Duration::INFINITE)?;
        let painted = renderer.render(
            [offsets[0] + left as usize, offsets[1] + right as usize],
            [eye(&views[0]), eye(&views[1])],
            settings,
            video,
            panel,
        );
        let released_left = chains[0].release_image();
        let released_right = chains[1].release_image();
        painted?;
        released_left?;
        released_right?;
        let layers = [0, 1].map(|i| {
            xr::CompositionLayerProjectionView::new()
                .pose(views[i].pose)
                .fov(views[i].fov)
                .sub_image(
                    xr::SwapchainSubImage::new()
                        .swapchain(&chains[i])
                        .image_array_index(0)
                        .image_rect(rect),
                )
        });
        stream.end(
            time,
            blend,
            &[&xr::CompositionLayerProjection::new()
                .space(space)
                .views(&layers)],
        )?;
        app.on_presented(controls.hands);
    }
}
