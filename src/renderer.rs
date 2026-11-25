use crate::hands::HandVisual;
use anyhow::{anyhow, ensure, Context, Result};
use ash::{util::read_spv, vk, Device, Instance};
use std::{io::Cursor, mem::size_of, ptr, slice};

mod hand_pipeline;
mod hands;

#[derive(Clone, Copy)]
pub struct Pixels<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

#[derive(Clone, Copy, Default)]
pub struct Eye {
    pub orientation: [f32; 4],
    pub position: [f32; 3],
    pub fov: [f32; 4],
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub projection: u32,
    pub stereo: u32,
    pub swap_eyes: bool,
    pub cinema: bool,
    pub panel_visible: bool,
    pub panel_opacity: f32,
    pub hands: [HandVisual; 2],
    pub aspect: f32,
}

// The visible controls occupy the bottom quarter of a transparent 2:1 canvas.
// At the seat, their top edge sits just below the 16:9 screen's lower frame.
// Keeping both surfaces at nearly the same depth also preserves their
// alignment as the viewer leans sideways.
pub(crate) const PANEL: [f32; 4] = [6.50, 3.25, -0.575, -6.12];

pub fn panel_hit(origin: [f32; 3], direction: [f32; 3]) -> Option<[f32; 2]> {
    if direction[2] >= -0.001 {
        return None;
    }
    let [width, height, y, z] = PANEL;
    let t = (z - origin[2]) / direction[2];
    let u = 0.5 + (origin[0] + direction[0] * t) / width;
    let v = 0.5 - (origin[1] + direction[1] * t - y) / height;
    (t > 0.0 && (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some([u, v])
}

#[repr(C)]
struct Constants {
    orientation: [f32; 4],
    origin: [f32; 4],
    tangents: [f32; 4],
    mode: [i32; 4],
    display: [f32; 4],
    panel: [f32; 4],
}

fn color_range(layer: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_array_layer(layer)
        .layer_count(1)
        .level_count(1)
}

fn memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    allowed: u32,
    required: vk::MemoryPropertyFlags,
    preferred: vk::MemoryPropertyFlags,
) -> Result<(u32, vk::MemoryPropertyFlags)> {
    properties.memory_types[..properties.memory_type_count as usize]
        .iter()
        .enumerate()
        .filter(|(i, ty)| allowed & (1u32 << i) != 0 && ty.property_flags.contains(required))
        .max_by_key(|(_, ty)| ty.property_flags.contains(preferred))
        .map(|(i, ty)| (i as u32, ty.property_flags))
        .context("no compatible Vulkan memory type")
}

// One persistently mapped upload allocation per texture. The next render waits
// for the previous submission before reusing staging storage or descriptors.
struct Texture {
    device: Device,
    size: [u32; 2],
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    mapped: *mut u8,
    coherent: bool,
    initialized: bool,
}

impl Texture {
    fn new(
        device: &Device,
        memory: &vk::PhysicalDeviceMemoryProperties,
        format: vk::Format,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let mut texture = Self {
            device: device.clone(),
            size: [width, height],
            image: Default::default(),
            memory: Default::default(),
            view: Default::default(),
            staging: Default::default(),
            staging_memory: Default::default(),
            mapped: ptr::null_mut(),
            coherent: false,
            initialized: false,
        };
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        unsafe {
            texture.image = device.create_image(&image_info, None)?;
            let req = device.get_image_memory_requirements(texture.image);
            let (i, _) = memory_type(
                memory,
                req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                Default::default(),
            )?;
            texture.memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(i),
                None,
            )?;
            device.bind_image_memory(texture.image, texture.memory, 0)?;
            texture.view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(texture.image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(color_range(0)),
                None,
            )?;

            texture.staging = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(u64::from(width) * u64::from(height) * 4)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = device.get_buffer_memory_requirements(texture.staging);
            let (i, flags) = memory_type(
                memory,
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;
            texture.coherent = flags.contains(vk::MemoryPropertyFlags::HOST_COHERENT);
            texture.staging_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(i),
                None,
            )?;
            device.bind_buffer_memory(texture.staging, texture.staging_memory, 0)?;
            texture.mapped = device
                .map_memory(
                    texture.staging_memory,
                    0,
                    vk::WHOLE_SIZE,
                    Default::default(),
                )?
                .cast();
        }
        Ok(texture)
    }

    fn upload(&mut self, cmd: vk::CommandBuffer, bytes: &[u8]) -> Result<()> {
        let [width, height] = self.size;
        ensure!(
            bytes.len() as u64 == u64::from(width) * u64::from(height) * 4,
            "incorrect RGBA byte count"
        );
        let (old_layout, access, stage) = if self.initialized {
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_READ,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
        } else {
            (
                vk::ImageLayout::UNDEFINED,
                vk::AccessFlags::empty(),
                vk::PipelineStageFlags::TOP_OF_PIPE,
            )
        };
        let barrier = vk::ImageMemoryBarrier::default()
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.image)
            .subresource_range(color_range(0));
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), self.mapped, bytes.len());
            if !self.coherent {
                self.device
                    .flush_mapped_memory_ranges(&[vk::MappedMemoryRange::default()
                        .memory(self.staging_memory)
                        .size(vk::WHOLE_SIZE)])?;
            }
            self.device.cmd_pipeline_barrier(
                cmd,
                stage,
                vk::PipelineStageFlags::TRANSFER,
                Default::default(),
                &[],
                &[],
                &[barrier
                    .old_layout(old_layout)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_access_mask(access)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)],
            );
            let copy = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            self.device.cmd_copy_buffer_to_image(
                cmd,
                self.staging,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            );
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                Default::default(),
                &[],
                &[],
                &[barrier
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)],
            );
        }
        self.initialized = true;
        Ok(())
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        unsafe {
            if !self.mapped.is_null() {
                self.device.unmap_memory(self.staging_memory);
            }
            self.device.destroy_buffer(self.staging, None);
            self.device.free_memory(self.staging_memory, None);
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

pub struct Renderer {
    device: Device,
    memory: vk::PhysicalDeviceMemoryProperties,
    texture_limit: u32,
    queue: vk::Queue,
    extent: vk::Extent2D,
    srgb_target: bool,
    sampler: vk::Sampler,
    set_layout: vk::DescriptorSetLayout,
    set_pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    layout: vk::PipelineLayout,
    pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    hand_pipeline: vk::Pipeline,
    hand_layout: vk::PipelineLayout,
    hand_vertices: vk::Buffer,
    hand_memory: vk::DeviceMemory,
    hand_vertex_count: u32,
    depth_image: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    textures: [Option<Texture>; 2],
}

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical: vk::PhysicalDevice,
        family: u32,
        queue: vk::Queue,
        format: vk::Format,
        extent: vk::Extent2D,
        images: &[(vk::Image, u32)],
    ) -> Result<Self> {
        ensure!(!images.is_empty(), "OpenXR returned an empty swapchain");
        let mut r = Self {
            device: device.clone(),
            memory: unsafe { instance.get_physical_device_memory_properties(physical) },
            texture_limit: unsafe { instance.get_physical_device_properties(physical) }
                .limits
                .max_image_dimension2_d,
            queue,
            extent,
            srgb_target: matches!(
                format,
                vk::Format::R8G8B8A8_SRGB | vk::Format::B8G8R8A8_SRGB
            ),
            sampler: Default::default(),
            set_layout: Default::default(),
            set_pool: Default::default(),
            set: Default::default(),
            layout: Default::default(),
            pass: Default::default(),
            pipeline: Default::default(),
            hand_pipeline: Default::default(),
            hand_layout: Default::default(),
            hand_vertices: Default::default(),
            hand_memory: Default::default(),
            hand_vertex_count: 0,
            depth_image: Default::default(),
            depth_memory: Default::default(),
            depth_view: Default::default(),
            views: vec![],
            framebuffers: vec![],
            pool: Default::default(),
            cmd: Default::default(),
            fence: Default::default(),
            textures: [None, None],
        };
        r.create_descriptors()?;
        r.create_hand_depth()?;
        r.create_targets(format, images)?;
        r.pipeline = r.create_pipeline(false)?;
        r.create_hand_geometry()?;
        r.hand_pipeline = r.create_pipeline(true)?;
        unsafe {
            r.pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(family)
                    .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                None,
            )?;
            r.cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(r.pool)
                    .command_buffer_count(1)
                    .level(vk::CommandBufferLevel::PRIMARY),
            )?[0];
            r.fence = device.create_fence(
                &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )?;
        }
        Ok(r)
    }

    fn create_descriptors(&mut self) -> Result<()> {
        let bindings = [0, 1].map(|i| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(i)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        });
        let pool_size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(2);
        unsafe {
            self.sampler = self.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .min_filter(vk::Filter::LINEAR)
                    .mag_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            self.set_layout = self.device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            self.set_pool = self.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .pool_sizes(&[pool_size])
                    .max_sets(1),
                None,
            )?;
            self.set = self.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.set_pool)
                    .set_layouts(&[self.set_layout]),
            )?[0];
            self.layout = self.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[self.set_layout])
                    .push_constant_ranges(&[vk::PushConstantRange::default()
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                        .size(size_of::<Constants>() as u32)]),
                None,
            )?;
        }
        Ok(())
    }

    fn create_targets(&mut self, format: vk::Format, images: &[(vk::Image, u32)]) -> Result<()> {
        let color = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth_reference = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color)
            .depth_stencil_attachment(&depth_reference);
        let attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let depth = vk::AttachmentDescription::default()
            .format(vk::Format::D16_UNORM)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        // Each eye reuses the same transient depth surface. Order the preceding
        // eye's writes before clearing it for the next view.
        let stages = vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(stages | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(stages | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
            .dst_access_mask(
                vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )
            .dependency_flags(vk::DependencyFlags::BY_REGION);
        unsafe {
            self.pass = self.device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&[attachment, depth])
                    .dependencies(&[dependency])
                    .subpasses(&[subpass]),
                None,
            )?;
            for &(image, layers) in images {
                ensure!(
                    layers == 1 || layers == 2,
                    "expected one or two image layers"
                );
                for eye in 0..2 {
                    let view = self.device.create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(image)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(format)
                            .subresource_range(color_range(eye.min(layers - 1))),
                        None,
                    )?;
                    self.views.push(view);
                    self.framebuffers.push(
                        self.device.create_framebuffer(
                            &vk::FramebufferCreateInfo::default()
                                .render_pass(self.pass)
                                .attachments(&[view, self.depth_view])
                                .width(self.extent.width)
                                .height(self.extent.height)
                                .layers(1),
                            None,
                        )?,
                    );
                }
            }
        }
        Ok(())
    }

    fn create_pipeline(&self, controllers: bool) -> Result<vk::Pipeline> {
        let (vs_code, fs_code): (&[u8], &[u8]) = if controllers {
            (
                include_bytes!(concat!(env!("OUT_DIR"), "/hands.vert.spv")),
                include_bytes!(concat!(env!("OUT_DIR"), "/hands.frag.spv")),
            )
        } else {
            (
                include_bytes!(concat!(env!("OUT_DIR"), "/cinema.vert.spv")),
                include_bytes!(concat!(env!("OUT_DIR"), "/cinema.frag.spv")),
            )
        };
        let vertex = read_spv(&mut Cursor::new(vs_code))?;
        let fragment = read_spv(&mut Cursor::new(fs_code))?;
        unsafe {
            let vs = self
                .device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vertex), None)?;
            let fs_result = self
                .device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&fragment), None);
            let fs = match fs_result {
                Ok(module) => module,
                Err(e) => {
                    self.device.destroy_shader_module(vs, None);
                    return Err(e.into());
                }
            };
            let stages = [
                (vk::ShaderStageFlags::VERTEX, vs),
                (vk::ShaderStageFlags::FRAGMENT, fs),
            ]
            .map(|(stage, module)| {
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(stage)
                    .module(module)
                    .name(c"main")
            });
            let binding = [vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride(size_of::<hands::HandVertex>() as u32)
                .input_rate(vk::VertexInputRate::VERTEX)];
            let attribs = hand_pipeline::attributes();
            let mut vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
            if controllers {
                vertex_input = vertex_input
                    .vertex_binding_descriptions(&binding)
                    .vertex_attribute_descriptions(&attribs);
            }
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = [vk::Viewport::default()
                .width(self.extent.width as f32)
                .height(self.extent.height as f32)
                .max_depth(1.0)];
            let scissor = [vk::Rect2D::default().extent(self.extent)];
            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewports(&viewport)
                .scissors(&scissor);
            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .line_width(1.0);
            let samples = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(controllers)
                .depth_write_enable(controllers)
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
            let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(controllers)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend =
                vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&raster)
                .multisample_state(&samples)
                .color_blend_state(&blend)
                .depth_stencil_state(&depth)
                .layout(if controllers {
                    self.hand_layout
                } else {
                    self.layout
                })
                .render_pass(self.pass);
            let pipeline =
                self.device
                    .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None);
            self.device.destroy_shader_module(fs, None);
            self.device.destroy_shader_module(vs, None);
            Ok(pipeline.map_err(|(partial, e)| {
                for pipeline in partial {
                    self.device.destroy_pipeline(pipeline, None);
                }
                anyhow!("creating cinema graphics pipeline: {e:?}")
            })?[0])
        }
    }

    fn update_texture(&mut self, binding: usize, input: Option<Pixels<'_>>) -> Result<()> {
        // Bind valid black images even before the first frame or panel arrives.
        let pixels = match input {
            Some(pixels) => pixels,
            None if self.textures[binding].is_some() => return Ok(()),
            None => Pixels {
                width: 1,
                height: 1,
                rgba: &[0, 0, 0, 0],
            },
        };
        ensure!(
            (1..=self.texture_limit).contains(&pixels.width)
                && (1..=self.texture_limit).contains(&pixels.height),
            "texture {}x{} exceeds GPU limits",
            pixels.width,
            pixels.height
        );
        let size = [pixels.width, pixels.height];
        if self.textures[binding].as_ref().map(|t| t.size) != Some(size) {
            // Match the transfer function of the target. sRGB textures linearize in the
            // sampler and sRGB attachments encode on store, including during HUD blending.
            let format = if self.srgb_target {
                vk::Format::R8G8B8A8_SRGB
            } else {
                vk::Format::R8G8B8A8_UNORM
            };
            let texture = Texture::new(
                &self.device,
                &self.memory,
                format,
                pixels.width,
                pixels.height,
            )?;
            let descriptor = [vk::DescriptorImageInfo::default()
                .sampler(self.sampler)
                .image_view(texture.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            unsafe {
                self.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(self.set)
                        .dst_binding(binding as u32)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&descriptor)],
                    &[],
                );
            }
            self.textures[binding] = Some(texture);
        }
        self.textures[binding]
            .as_mut()
            .unwrap()
            .upload(self.cmd, pixels.rgba)
    }

    pub fn render(
        &mut self,
        image: [usize; 2],
        eyes: [Eye; 2],
        settings: Settings,
        video: Option<Pixels<'_>>,
        panel: Option<Pixels<'_>>,
    ) -> Result<()> {
        ensure!(
            image
                .iter()
                .all(|&index| index < self.framebuffers.len() / 2),
            "unknown swapchain image index"
        );
        unsafe {
            // enable2 allows outstanding work on the session's graphics queue
            // at xrReleaseSwapchainImage. Wait here, only when reusing resources.
            self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;
            self.device
                .reset_command_pool(self.pool, Default::default())?;
            self.device.begin_command_buffer(
                self.cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
        }
        self.update_texture(0, video)?;
        self.update_texture(1, panel)?;
        let mut panel_geometry = PANEL;
        if !settings.panel_visible {
            panel_geometry[0] = 0.0;
        }
        for (index, eye) in eyes.into_iter().enumerate() {
            let [x, y, z] = eye.position;
            let constants = Constants {
                orientation: eye.orientation,
                origin: [x, y, z, settings.panel_opacity.clamp(0.0, 1.0)],
                tangents: eye.fov.map(f32::tan),
                mode: [
                    settings.projection as i32,
                    settings.stereo as i32,
                    (index ^ usize::from(settings.swap_eyes)) as i32,
                    i32::from(settings.cinema),
                ],
                display: [
                    settings.aspect.clamp(0.01, 100.0),
                    0.0,
                    0.0,
                    u8::from(self.srgb_target) as f32,
                ],
                panel: panel_geometry,
            };
            unsafe {
                let clears = [
                    vk::ClearValue::default(),
                    vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: 1.0,
                            stencil: 0,
                        },
                    },
                ];
                let pass = vk::RenderPassBeginInfo::default()
                    .render_pass(self.pass)
                    .framebuffer(self.framebuffers[image[index] * 2 + index])
                    .clear_values(&clears)
                    .render_area(vk::Rect2D::default().extent(self.extent));
                self.device
                    .cmd_begin_render_pass(self.cmd, &pass, vk::SubpassContents::INLINE);
                self.device.cmd_bind_pipeline(
                    self.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline,
                );
                self.device.cmd_bind_descriptor_sets(
                    self.cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[self.set],
                    &[],
                );
                // All fields are arrays of four 32-bit scalars; repr(C) matches the shader's vec4 slots.
                let bytes = slice::from_raw_parts(
                    ptr::from_ref(&constants).cast::<u8>(),
                    size_of::<Constants>(),
                );
                self.device.cmd_push_constants(
                    self.cmd,
                    self.layout,
                    vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytes,
                );
                self.device.cmd_draw(self.cmd, 3, 1, 0, 0);
                self.draw_hands(eye, &settings.hands);
                self.device.cmd_end_render_pass(self.cmd);
            }
        }
        unsafe {
            self.device.end_command_buffer(self.cmd)?;
            self.device.reset_fences(&[self.fence])?;
            self.device.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&[self.cmd])],
                self.fence,
            )?;
        }
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.queue_wait_idle(self.queue);
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_descriptor_pool(self.set_pool, None);
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_pipeline(self.hand_pipeline, None);
            self.device.destroy_pipeline_layout(self.hand_layout, None);
            self.device.destroy_buffer(self.hand_vertices, None);
            self.device.free_memory(self.hand_memory, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            for &buffer in &self.framebuffers {
                self.device.destroy_framebuffer(buffer, None);
            }
            for &view in &self.views {
                self.device.destroy_image_view(view, None);
            }
            self.device.destroy_render_pass(self.pass, None);
            self.device.destroy_image_view(self.depth_view, None);
            self.device.destroy_image(self.depth_image, None);
            self.device.free_memory(self.depth_memory, None);
        }
    }
}

#[cfg(test)]
mod readback;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_ray_uses_the_rendered_top_left_coordinates() {
        let [w, h, y, z] = PANEL;
        for (x, y_offset, expected) in [(0.0, 0.0, [0.5, 0.5]), (-w * 0.48, h * 0.45, [0.02, 0.05])]
        {
            let uv = panel_hit([0.0; 3], [x, y + y_offset, z]).unwrap();
            assert!((uv[0] - expected[0]).abs() < 1e-5 && (uv[1] - expected[1]).abs() < 1e-5);
        }
        assert!(panel_hit([0.0; 3], [0.0, 0.0, 1.0]).is_none());
        assert!(panel_hit([0.0; 3], [w * 0.51, y, z]).is_none());
    }

    #[test]
    fn bar_aligns_below_widescreen_across_the_seat() {
        let [width, height, y_center, z] = PANEL;
        let screen_z = -6.45;
        let screen_width = 2.0 * 2.05 * 16.0 / 9.0;
        let projected_bar_width = width * (992.0 / 1024.0) * screen_z / z;
        assert!((0.85..0.95).contains(&(projected_bar_width / screen_width)));
        let bar_top = (y_center + height * (0.5 - 364.0 / 512.0)) * screen_z / z;
        let gap = (0.82 - 2.05) - bar_top;
        assert!((0.065..0.16).contains(&gap));
        for head_x in [-0.30, 0.30] {
            let shifted_center = head_x * (1.0 - screen_z / z);
            assert!(shifted_center.abs() < 0.025);
        }
    }
}
