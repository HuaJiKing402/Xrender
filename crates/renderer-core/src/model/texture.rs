use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use wgpu::{Device, Queue, Texture, TextureView};

/// 已上传到 GPU 的材质贴图。
pub struct TextureSlot {
    pub label: String,
    pub texture: Texture,
    pub view: TextureView,
    /// 贴图 alpha 通道的最大值；用于区分“整体半透明”和“局部镂空”。
    pub alpha_max: u8,
    /// toon 贴图的渐变排布；非 toon 贴图保持默认值。
    pub toon: ToonSampling,
}

/// toon 图的渐变排布。
///
/// MMD 的 toon 图既有横向渐变也有纵向渐变，方向也不统一（亮端可能在首端也可能在末端）。
/// 采样轴和方向必须按图像实际内容决定，不能一概用 `(shade, 0.5)`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToonSampling {
    /// true = 渐变沿 Y（纵向排布），false = 沿 X。
    pub vertical: bool,
    /// true = 坐标 0 那头是亮端，采样时需要反向（用 `1 - shade`）。
    pub flip: bool,
}

impl ToonSampling {
    /// 打包成给 shader 的编码：0 横向、1 纵向、2 纵向+反向、3 横向+反向。
    pub fn code(self) -> f32 {
        match (self.vertical, self.flip) {
            (false, false) => 0.0,
            (true, false) => 1.0,
            (true, true) => 2.0,
            (false, true) => 3.0,
        }
    }
}

fn luminance(pixel: &[u8]) -> f32 {
    0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32
}

/// 用 16×16 降采样网格比较两个方向的梯度强度，判断渐变轴与亮端位置。
fn detect_toon_sampling(width: u32, height: u32, rgba: &[u8]) -> ToonSampling {
    const GRID: u32 = 16;
    if width < 2 || height < 2 {
        return ToonSampling::default();
    }
    let cols = GRID.min(width);
    let rows = GRID.min(height);
    let mut grid = vec![0.0f32; (cols * rows) as usize];
    for gy in 0..rows {
        for gx in 0..cols {
            let x = (gx * width / cols).min(width - 1);
            let y = (gy * height / rows).min(height - 1);
            let offset = ((y * width + x) * 4) as usize;
            if let Some(pixel) = rgba.get(offset..offset + 4) {
                grid[(gy * cols + gx) as usize] = luminance(pixel);
            }
        }
    }

    let mut gradient_x = 0.0f32;
    let mut gradient_y = 0.0f32;
    for gy in 0..rows {
        for gx in 0..cols {
            let here = grid[(gy * cols + gx) as usize];
            if gx + 1 < cols {
                gradient_x += (grid[(gy * cols + gx + 1) as usize] - here).abs();
            }
            if gy + 1 < rows {
                gradient_y += (grid[((gy + 1) * cols + gx) as usize] - here).abs();
            }
        }
    }
    if gradient_x + gradient_y <= 0.0 {
        return ToonSampling::default();
    }

    let vertical = gradient_y > gradient_x;
    let head = if vertical {
        let row = &grid[0..cols as usize];
        row.iter().sum::<f32>() / cols as f32
    } else {
        (0..rows).map(|gy| grid[(gy * cols) as usize]).sum::<f32>() / rows as f32
    };
    let tail = if vertical {
        let start = ((rows - 1) * cols) as usize;
        grid[start..start + cols as usize].iter().sum::<f32>() / cols as f32
    } else {
        (0..rows)
            .map(|gy| grid[(gy * cols + cols - 1) as usize])
            .sum::<f32>()
            / rows as f32
    };

    ToonSampling {
        vertical,
        // 首端更亮说明 `shade = 1` 应该落在坐标 0，采样时反向。
        flip: head > tail,
    }
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

/// MMD 官方内置 toon01-toon10（32×32、24 位 BMP，取自 MMD 安装目录）。
///
/// 打包进二进制，避免运行时依赖 MMD 安装路径：官方 toon 不随模型分发，
/// 模型目录里通常找不到，之前正是因此回退到程序生成的假渐变。
const BUILTIN_TOON: [&[u8]; 10] = [
    include_bytes!("../../assets/toon/toon01.bmp"),
    include_bytes!("../../assets/toon/toon02.bmp"),
    include_bytes!("../../assets/toon/toon03.bmp"),
    include_bytes!("../../assets/toon/toon04.bmp"),
    include_bytes!("../../assets/toon/toon05.bmp"),
    include_bytes!("../../assets/toon/toon06.bmp"),
    include_bytes!("../../assets/toon/toon07.bmp"),
    include_bytes!("../../assets/toon/toon08.bmp"),
    include_bytes!("../../assets/toon/toon09.bmp"),
    include_bytes!("../../assets/toon/toon10.bmp"),
];

/// 官方 toon 的采样排布：纵向渐变、白色在上半（图像 y=0），暗端在下半。
///
/// 固定为 `vertical + flip`（编码 2），不做内容检测——官方图的轴和朝向是确定的，
/// 靠梯度猜反而会被 toon05/06 这类带横向变化的图带偏。
fn builtin_toon_sampling() -> ToonSampling {
    ToonSampling {
        vertical: true,
        flip: true,
    }
}

fn decode_builtin_toon(index: usize) -> Option<image::RgbaImage> {
    let bytes = BUILTIN_TOON.get(index)?;
    image::load_from_memory(bytes)
        .ok()
        .map(|image| image.to_rgba8())
}

fn builtin_toon_slot(device: &Device, queue: &Queue, index: usize) -> TextureSlot {
    // 解码失败时退到官方 toon01 的影色，至少不会再把阴影压成纯黑。
    let (width, height, rgba) = match decode_builtin_toon(index) {
        Some(image) => (image.width(), image.height(), image.into_raw()),
        None => (1, 1, vec![205u8, 205, 205, 255]),
    };
    let texture = create_texture_from_rgba(
        device,
        queue,
        &format!("builtin-toon-{}", index + 1),
        width,
        height,
        &rgba,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    TextureSlot {
        label: format!("toon{:02}.bmp", index + 1),
        texture,
        view,
        alpha_max: 255,
        toon: builtin_toon_sampling(),
    }
}

/// 扫描单个目录，按文件名归类 toon 候选；同名的多种扩展名取 rank 最小者。
fn scan_toon_dir(dir: &Path) -> HashMap<String, (u8, PathBuf)> {
    let mut candidates = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(extension) = path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase())
            else {
                continue;
            };
            let rank = match extension.as_str() {
                "bmp" => 0,
                "png" => 1,
                "tga" => 2,
                "jpg" | "jpeg" => 3,
                _ => continue,
            };
            let Some(stem) = path
                .file_stem()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase())
            else {
                continue;
            };
            candidates
                .entry(stem)
                .and_modify(|(current, stored_path)| {
                    if rank < *current {
                        *current = rank;
                        *stored_path = path.clone();
                    }
                })
                .or_insert((rank, path));
        }
    }
    candidates
}

/// toon 贴图的搜索目录，按优先级排列：
/// 1. 环境变量 `MMD_TOON_DIR`（可直接指向 MMD 安装目录，最贴近原生）
/// 2. 模型所在目录
fn toon_search_dirs(base_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("MMD_TOON_DIR") {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }
    dirs.push(base_dir.to_path_buf());
    dirs
}

/// 加载 toon01-toon10：外部文件优先，全部缺失时回退到打包的官方原图。
pub fn load_toon_textures(device: &Device, queue: &Queue, base_dir: &Path) -> Vec<TextureSlot> {
    // 每个目录只扫一次，避免对每个候选名做 open 探测。
    let scans: Vec<HashMap<String, (u8, PathBuf)>> = toon_search_dirs(base_dir)
        .iter()
        .map(|dir| scan_toon_dir(dir))
        .collect();

    (0..10)
        .map(|index| {
            for candidates in &scans {
                for stem in [
                    format!("toon{:02}", index + 1),
                    format!("toon{}", index + 1),
                ] {
                    let Some((_, path)) = candidates.get(&stem) else {
                        continue;
                    };
                    let Ok(image) = image::open(path) else {
                        continue;
                    };
                    let rgba = image.to_rgba8();
                    let alpha_max = rgba.pixels().map(|pixel| pixel[3]).max().unwrap_or(255);
                    let toon = detect_toon_sampling(rgba.width(), rgba.height(), rgba.as_raw());
                    let texture = create_texture_from_rgba(
                        device,
                        queue,
                        &format!("toon-texture-{}", index + 1),
                        rgba.width(),
                        rgba.height(),
                        rgba.as_raw(),
                    );
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    return TextureSlot {
                        label: format!("toon{:02}", index + 1),
                        texture,
                        view,
                        alpha_max,
                        toon,
                    };
                }
            }
            builtin_toon_slot(device, queue, index)
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

        let (texture, alpha_max, toon) = match image::open(&full_path) {
            Ok(image) => {
                let rgba = image.to_rgba8();
                let alpha_max = rgba.pixels().map(|pixel| pixel[3]).max().unwrap_or(255);
                let toon = detect_toon_sampling(rgba.width(), rgba.height(), rgba.as_raw());
                (
                    create_texture_from_rgba(
                        device,
                        queue,
                        &format!("pmx-texture-{index}"),
                        rgba.width(),
                        rgba.height(),
                        rgba.as_raw(),
                    ),
                    alpha_max,
                    toon,
                )
            }
            Err(error) => {
                log::warn!(
                    "texture {} not usable ({}), using white fallback",
                    full_path.display(),
                    error
                );
                (
                    white_texture(device, queue, &format!("pmx-texture-{index}-fallback")),
                    255,
                    ToonSampling::default(),
                )
            }
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        slots.push(TextureSlot {
            label: path.clone(),
            texture,
            view,
            alpha_max,
            toon,
        });
    }
    Ok(slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成一张灰度渐变，用来验证轴与方向的检测。
    fn ramp(width: u32, height: u32, vertical: bool, bright_at_start: bool) -> Vec<u8> {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let t = if vertical {
                    y as f32 / (height - 1).max(1) as f32
                } else {
                    x as f32 / (width - 1).max(1) as f32
                };
                let value = if bright_at_start { 1.0 - t } else { t };
                let value = (value * 255.0) as u8;
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        rgba
    }

    #[test]
    fn detects_vertical_ramp_with_bright_head() {
        // 对应模型目录里的 toon3/4/5.png：纵向、顶部发白。
        let sampling = detect_toon_sampling(16, 16, &ramp(16, 16, true, true));
        assert!(sampling.vertical);
        assert!(sampling.flip);
        assert_eq!(sampling.code(), 2.0);
    }

    #[test]
    fn detects_horizontal_ramp_with_dark_head() {
        // 横向、暗端在左的图应判为横向不反向（code 0）。
        let sampling = detect_toon_sampling(16, 4, &ramp(16, 4, false, false));
        assert!(!sampling.vertical);
        assert!(!sampling.flip);
        assert_eq!(sampling.code(), 0.0);
    }

    #[test]
    fn flat_texture_keeps_default_sampling() {
        let flat = vec![255u8; 8 * 8 * 4];
        let sampling = detect_toon_sampling(8, 8, &flat);
        assert_eq!(sampling, ToonSampling::default());
    }

    #[test]
    fn builtin_toon_decodes_official_images() {
        // 官方 toon01：32×32，影色 205 灰，顶部白色。
        let toon01 = decode_builtin_toon(0).expect("toon01 should decode");
        assert_eq!((toon01.width(), toon01.height()), (32, 32));
        let shadow = toon01.get_pixel(0, 31);
        assert_eq!([shadow[0], shadow[1], shadow[2]], [205, 205, 205]);
        let lit = toon01.get_pixel(0, 0);
        assert_eq!([lit[0], lit[1], lit[2]], [255, 255, 255]);

        // 官方 toon02 的影色是暖调的浅粉灰，不能是纯灰。
        let toon02 = decode_builtin_toon(1).expect("toon02 should decode");
        let shadow = toon02.get_pixel(0, 31);
        assert_eq!([shadow[0], shadow[1], shadow[2]], [245, 225, 225]);
        assert!(shadow[0] > shadow[2], "toon02 shadow should be warm-tinted");

        // 官方 toon07–10 几乎全白，等于无阴影。
        for index in 6..10 {
            let toon = decode_builtin_toon(index).expect("toon07..10 should decode");
            let min = toon.pixels().map(|pixel| pixel[0]).min().unwrap_or(0);
            assert_eq!(min, 255, "toon{:02} should be all white", index + 1);
        }
    }

    #[test]
    fn builtin_toon_uses_fixed_vertical_sampling() {
        let sampling = builtin_toon_sampling();
        assert!(sampling.vertical);
        assert!(sampling.flip);
        assert_eq!(sampling.code(), 2.0);
    }
}
