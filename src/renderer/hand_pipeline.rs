use super::hands::{mesh, HandConstants, HandVertex};
use super::*;
use std::mem::{offset_of, size_of_val};

pub(super) fn attributes() -> [vk::VertexInputAttributeDescription; 4] {
    [
        (
            0,
            vk::Format::R32G32B32_SFLOAT,
            offset_of!(HandVertex, position),
        ),
        (
            1,
            vk::Format::R32G32B32_SFLOAT,
            offset_of!(HandVertex, normal),
        ),
        (
            2,
            vk::Format::R32G32B32A32_SFLOAT,
            offset_of!(HandVertex, color),
        ),
        (3, vk::Format::R32_UINT, offset_of!(HandVertex, part)),
    ]
    .map(|(location, format, offset)| {
        vk::VertexInputAttributeDescription::default()
            .location(location)
            .binding(0)
            .format(format)
            .offset(offset as u32)
    })
}

impl Renderer {
    pub(super) fn create_hand_depth(&mut self) -> Result<()> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::D16_UNORM)
            .extent(vk::Extent3D {
                width: self.extent.width,
                height: self.extent.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                    | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        unsafe {
            self.depth_image = self.device.create_image(&info, None)?;
            let req = self.device.get_image_memory_requirements(self.depth_image);
            let (index, _) = memory_type(
                &self.memory,
                req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                vk::MemoryPropertyFlags::LAZILY_ALLOCATED,
            )?;
            self.depth_memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(index),
                None,
            )?;
            self.device
                .bind_image_memory(self.depth_image, self.depth_memory, 0)?;
            self.depth_view = self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(self.depth_image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(info.format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::DEPTH)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )?;
        }
        Ok(())
    }

    pub(super) fn create_hand_geometry(&mut self) -> Result<()> {
        let vertices = mesh();
        let bytes = size_of_val(vertices.as_slice());
        self.hand_vertex_count = u32::try_from(vertices.len())?;
        unsafe {
            // The topology, normals and materials never change. Only small pose
            // constants are sent while the controllers move.
            self.hand_vertices = self.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(bytes as u64)
                    .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?;
            let req = self
                .device
                .get_buffer_memory_requirements(self.hand_vertices);
            let (index, flags) = memory_type(
                &self.memory,
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )?;
            self.hand_memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(index),
                None,
            )?;
            self.device
                .bind_buffer_memory(self.hand_vertices, self.hand_memory, 0)?;
            let mapped =
                self.device
                    .map_memory(self.hand_memory, 0, vk::WHOLE_SIZE, Default::default())?;
            ptr::copy_nonoverlapping(vertices.as_ptr().cast::<u8>(), mapped.cast::<u8>(), bytes);
            let flushed = if flags.contains(vk::MemoryPropertyFlags::HOST_COHERENT) {
                Ok(())
            } else {
                self.device
                    .flush_mapped_memory_ranges(&[vk::MappedMemoryRange::default()
                        .memory(self.hand_memory)
                        .size(vk::WHOLE_SIZE)])
            };
            self.device.unmap_memory(self.hand_memory);
            flushed?;
            self.hand_layout = self.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&[
                    vk::PushConstantRange::default()
                        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                        .size(size_of::<HandConstants>() as u32),
                ]),
                None,
            )?;
        }
        Ok(())
    }

    pub(super) unsafe fn draw_hands(&self, eye: Eye, hands: &[HandVisual; 2]) {
        if hands.iter().all(|h| h.grip.is_none() && h.ray.is_none()) {
            return;
        }
        self.device.cmd_bind_pipeline(
            self.cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.hand_pipeline,
        );
        self.device
            .cmd_bind_vertex_buffers(self.cmd, 0, &[self.hand_vertices], &[0]);
        for (side, hand) in hands.iter().enumerate() {
            if hand.grip.is_none() && hand.ray.is_none() {
                continue;
            }
            let constants = HandConstants::new(eye, side, hand, self.srgb_target);
            let bytes = slice::from_raw_parts(
                ptr::from_ref(&constants).cast::<u8>(),
                size_of::<HandConstants>(),
            );
            self.device.cmd_push_constants(
                self.cmd,
                self.hand_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytes,
            );
            self.device
                .cmd_draw(self.cmd, self.hand_vertex_count, 1, 0, 0);
        }
    }
}
