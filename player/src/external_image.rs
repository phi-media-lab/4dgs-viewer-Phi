use std::os::fd::{FromRawFd, OwnedFd};

use anyhow::{Context, Result, bail, ensure};
use ash::vk;

pub const DRM_FORMAT_ARGB8888: u32 = fourcc(b'A', b'R', b'2', b'4');
const DRM_FORMAT_MOD_LINEAR: u64 = 0;
const VA_SAFE_MODIFIER_POLICY: &str = "linear-only; no fallback to tiled AR24";

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DmabufLayout {
    pub drm_fourcc: u32,
    pub drm_fourcc_name: &'static str,
    pub modifier: u64,
    pub modifier_hex: String,
    pub candidate_modifiers: Vec<String>,
    pub modifier_selection_policy: &'static str,
    pub plane_count: u32,
    pub offset: u64,
    pub stride: u64,
    pub allocation_bytes: u64,
}

pub struct ExternalImage {
    pub texture: wgpu::Texture,
    pub dmabuf: OwnedFd,
    pub layout: DmabufLayout,
}

struct RawExternalImage {
    hal_texture: wgpu::hal::vulkan::Texture,
    dmabuf: OwnedFd,
    layout: DmabufLayout,
}

impl ExternalImage {
    pub fn create(device: &wgpu::Device, width: u32, height: u32) -> Result<Self> {
        let raw = unsafe {
            let hal = device
                .as_hal::<wgpu::hal::api::Vulkan>()
                .context("wgpu device is not backed by Vulkan")?;
            create_raw(&hal, width, height)?
        };
        let descriptor = wgpu::TextureDescriptor {
            label: Some("DMA-BUF exportable BGRA output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu::hal::api::Vulkan>(
                raw.hal_texture,
                &descriptor,
                wgpu::TextureUses::UNINITIALIZED,
            )
        };
        Ok(Self {
            texture,
            dmabuf: raw.dmabuf,
            layout: raw.layout,
        })
    }
}

unsafe fn create_raw(
    hal: &wgpu::hal::vulkan::Device,
    width: u32,
    height: u32,
) -> Result<RawExternalImage> {
    let raw_instance = hal.shared_instance().raw_instance();
    let physical_device = hal.raw_physical_device();
    let format = vk::Format::B8G8R8A8_UNORM;
    let candidates = unsafe { format_modifiers(raw_instance, physical_device, format) }?;
    ensure!(
        !candidates.is_empty(),
        "no single-plane DRM modifier supports BGRA color attachment + transfer source"
    );
    let candidate_modifiers = candidates
        .iter()
        .map(|modifier| format!("0x{modifier:016x}"))
        .collect::<Vec<_>>();
    let selected = select_va_safe_modifier(&candidates).with_context(|| {
        format!("select VA-safe BGRA modifier from Vulkan candidates {candidate_modifiers:?}")
    })?;
    unsafe {
        try_create(
            hal,
            selected,
            width,
            height,
            format,
            candidate_modifiers,
        )
    }
    .with_context(|| {
        format!(
            "create exportable BGRA image with required linear modifier; policy={VA_SAFE_MODIFIER_POLICY}"
        )
    })
}

fn select_va_safe_modifier(candidates: &[u64]) -> Result<u64> {
    ensure!(
        candidates.contains(&DRM_FORMAT_MOD_LINEAR),
        "linear DRM modifier 0x0000000000000000 is unavailable; refusing tiled fallback"
    );
    Ok(DRM_FORMAT_MOD_LINEAR)
}

unsafe fn format_modifiers(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    format: vk::Format,
) -> Result<Vec<u64>> {
    let available = {
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
        {
            let mut properties = vk::FormatProperties2::default().push_next(&mut list);
            unsafe {
                instance.get_physical_device_format_properties2(
                    physical_device,
                    format,
                    &mut properties,
                )
            };
        }
        list.drm_format_modifier_count as usize
    };
    let mut entries = vec![vk::DrmFormatModifierPropertiesEXT::default(); available];
    let count = {
        let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut entries);
        {
            let mut properties = vk::FormatProperties2::default().push_next(&mut list);
            unsafe {
                instance.get_physical_device_format_properties2(
                    physical_device,
                    format,
                    &mut properties,
                )
            };
        }
        list.drm_format_modifier_count as usize
    };
    let required = vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::TRANSFER_SRC;
    entries.truncate(count);
    let mut modifiers: Vec<_> = entries
        .into_iter()
        .filter(|entry| {
            entry.drm_format_modifier_plane_count == 1
                && entry.drm_format_modifier_tiling_features.contains(required)
        })
        .map(|entry| entry.drm_format_modifier)
        .collect();
    modifiers.sort_unstable();
    Ok(modifiers)
}

unsafe fn try_create(
    hal: &wgpu::hal::vulkan::Device,
    modifier: u64,
    width: u32,
    height: u32,
    format: vk::Format,
    candidate_modifiers: Vec<String>,
) -> Result<RawExternalImage> {
    let device = hal.raw_device();
    let modifiers = [modifier];
    let mut modifier_info =
        vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
    let mut external_info = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let image_info = vk::ImageCreateInfo::default()
        .push_next(&mut modifier_info)
        .push_next(&mut external_info)
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
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe { device.create_image(&image_info, None) }
        .context("vkCreateImage with DRM modifier")?;
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let memory_type_index = find_memory_type(hal, requirements.memory_type_bits)
        .context("no compatible device-local Vulkan memory type")?;
    let mut export_info = vk::ExportMemoryAllocateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let allocate_info = vk::MemoryAllocateInfo::default()
        .push_next(&mut export_info)
        .push_next(&mut dedicated_info)
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index);
    let memory = match unsafe { device.allocate_memory(&allocate_info, None) } {
        Ok(memory) => memory,
        Err(error) => {
            unsafe { device.destroy_image(image, None) };
            return Err(error).context("vkAllocateMemory for exportable image");
        }
    };
    if let Err(error) = unsafe { device.bind_image_memory(image, memory, 0) } {
        unsafe {
            device.destroy_image(image, None);
            device.free_memory(memory, None);
        }
        return Err(error).context("vkBindImageMemory for exportable image");
    }

    let external_fd =
        ash::khr::external_memory_fd::Device::new(hal.shared_instance().raw_instance(), device);
    let fd_info = vk::MemoryGetFdInfoKHR::default()
        .memory(memory)
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let raw_fd = match unsafe { external_fd.get_memory_fd(&fd_info) } {
        Ok(fd) => fd,
        Err(error) => {
            unsafe {
                device.destroy_image(image, None);
                device.free_memory(memory, None);
            }
            return Err(error).context("vkGetMemoryFdKHR(DMA_BUF)");
        }
    };
    let dmabuf = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let drm_extension = ash::ext::image_drm_format_modifier::Device::new(
        hal.shared_instance().raw_instance(),
        device,
    );
    let mut actual = vk::ImageDrmFormatModifierPropertiesEXT::default();
    if let Err(error) =
        unsafe { drm_extension.get_image_drm_format_modifier_properties(image, &mut actual) }
    {
        unsafe {
            device.destroy_image(image, None);
            device.free_memory(memory, None);
        }
        return Err(error).context("vkGetImageDrmFormatModifierPropertiesEXT");
    }
    if actual.drm_format_modifier != modifier {
        unsafe {
            device.destroy_image(image, None);
            device.free_memory(memory, None);
        }
        bail!(
            "driver selected modifier 0x{:016x}, requested 0x{modifier:016x}",
            actual.drm_format_modifier
        );
    }
    let subresource = vk::ImageSubresource {
        aspect_mask: vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
        mip_level: 0,
        array_layer: 0,
    };
    let plane = unsafe { device.get_image_subresource_layout(image, subresource) };
    let cleanup_device = device.clone();
    let callback: wgpu::hal::DropCallback = Box::new(move || unsafe {
        cleanup_device.destroy_image(image, None);
        cleanup_device.free_memory(memory, None);
    });
    let hal_descriptor = wgpu::hal::TextureDescriptor {
        label: Some("DMA-BUF exportable BGRA output"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUses::COLOR_TARGET | wgpu::TextureUses::COPY_SRC,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    };
    let hal_texture = unsafe {
        hal.texture_from_raw(
            image,
            &hal_descriptor,
            Some(callback),
            wgpu::hal::vulkan::TextureMemory::External,
        )
    };
    Ok(RawExternalImage {
        hal_texture,
        dmabuf,
        layout: DmabufLayout {
            drm_fourcc: DRM_FORMAT_ARGB8888,
            drm_fourcc_name: "AR24",
            modifier,
            modifier_hex: format!("0x{modifier:016x}"),
            candidate_modifiers,
            modifier_selection_policy: VA_SAFE_MODIFIER_POLICY,
            plane_count: 1,
            offset: plane.offset,
            stride: plane.row_pitch,
            allocation_bytes: requirements.size,
        },
    })
}

fn find_memory_type(hal: &wgpu::hal::vulkan::Device, compatible: u32) -> Option<u32> {
    let properties = unsafe {
        hal.shared_instance()
            .raw_instance()
            .get_physical_device_memory_properties(hal.raw_physical_device())
    };
    let mut fallback = None;
    for index in 0..properties.memory_type_count {
        if compatible & (1 << index) == 0 {
            continue;
        }
        fallback.get_or_insert(index);
        if properties.memory_types[index as usize]
            .property_flags
            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        {
            return Some(index);
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn va_safe_modifier_selects_linear_from_mixed_candidates() {
        assert_eq!(
            select_va_safe_modifier(&[0x0200_0000_1040_1b04, 0]).unwrap(),
            DRM_FORMAT_MOD_LINEAR
        );
    }

    #[test]
    fn va_safe_modifier_refuses_tiled_only_candidates() {
        let error = select_va_safe_modifier(&[0x0200_0000_1040_1b04]).unwrap_err();
        assert!(error.to_string().contains("refusing tiled fallback"));
    }
}
