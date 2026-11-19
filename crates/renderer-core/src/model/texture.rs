use std::path::{Path, PathBuf};

use anyhow::Result;
use wgpu::{Device, Queue, Texture, TextureView};

/// 已上传到 GPU 的材质贴图。
pub struct TextureSlot {
    pub label: String,
    pub texture: Texture,
    pub view: TextureView,
}

fn padded_rgba(width: u32, height: u32, rgba: &[u8]) -> (Vec<u8>, u32) {
    let unpadded_row = width * 4;
    let row = (unpadded_row + 255) & !255;
    if row == unpadded_row {
        return (rgba.to_vec(), row);
    }
    let mut padded = vec![0u8; (row * height) as usize];
    for y in 0..height as usize {
        let src = y * unpadded_row as usize;
        let dst = y * row as usize;
        padded[dst..dst + unpadded_row as usize]
            .copy_from_slice(&rgba[src..src + unpadded_row as usize]);
    }
    (padded, row)
}

fn create_texture_from_rgba(
    device: &Device,
    queue: &Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Texture {
    let (padded, bytes_per_row) = padded_rgba(width, height, rgba);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    texture
}

pub fn white_texture(device: &Device, queue: &Queue, label: &str) -> Texture {
    create_texture_from_rgba(device, queue, label, 1, 1, &[255, 255, 255, 255])
}

/// 生成 MMD 内部 toon01-toon10 的简化渐变纹理。
pub fn internal_toon_textures(device: &Device, queue: &Queue) -> Vec<TextureSlot> {
    (0..10)
        .map(|index| {
            let width = 16u32;
            let steps = 3 + index % 3;
            let mut rgba = Vec::with_capacity((width * 4) as usize);
            for x in 0..width {
                let value = ((x as f32 / (width - 1) as f32 * steps as f32).floor()
                    / (steps - 1) as f32
                    * 255.0)
                    .clamp(0.0, 255.0) as u8;
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
            let texture = create_texture_from_rgba(
                device,
                queue,
                &format!("internal-toon-{}", index + 1),
                width,
                1,
                &rgba,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            TextureSlot {
                label: format!("toon{:02}.bmp", index + 1),
                texture,
                view,
            }
        })
        .collect()
}

/// 按 PMX 纹理表加载贴图；路径相对 PMX 所在目录。
/// 缺失或解码失败时回退为 1×1 白色贴图，不中断模型加载。
pub fn load_textures(
    device: &Device,
    queue: &Queue,
    base_dir: &Path,
    paths: &[String],
) -> Result<Vec<TextureSlot>> {
    let mut slots = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let normalized = path.replace('\\', "/");
        let candidate = PathBuf::from(&normalized);
        let full_path = if candidate.is_absolute() {
            candidate
        } else {
            base_dir.join(candidate)
        };

        let texture = match image::open(&full_path) {
            Ok(image) => {
                let rgba = image.to_rgba8();
                create_texture_from_rgba(
                    device,
                    queue,
                    &format!("pmx-texture-{index}"),
                    rgba.width(),
                    rgba.height(),
                    rgba.as_raw(),
                )
            }
            Err(error) => {
                log::warn!(
                    "texture {} not usable ({}), using white fallback",
                    full_path.display(),
                    error
                );
                white_texture(device, queue, &format!("pmx-texture-{index}-fallback"))
            }
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        slots.push(TextureSlot {
            label: path.clone(),
            texture,
            view,
        });
    }
    Ok(slots)
}
