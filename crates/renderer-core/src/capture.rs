//! 帧画面的 CPU 侧表示。
//!
//! 渲染器的颜色目标在 GPU 上，回读之后以这份结构交给调用方：截图、像素断言、
//! 画面回归测试都从它取数据，不必各自再处理纹理布局与行对齐。

use std::path::Path;

use anyhow::{Context, Result};

/// 一帧的像素：行优先、紧密排布、每像素 4 字节 RGBA8。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl FrameImage {
    /// 由紧密排布的 RGBA8 数据构造。长度必须等于 `width * height * 4`。
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(
            pixels.len(),
            (width as usize) * (height as usize) * 4,
            "pixel buffer does not match the given frame size"
        );
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// 取出某个像素的 RGBA。越界返回 `None`，方便测试里写断言而不必先校验尺寸。
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        self.pixels
            .get(offset..offset + 4)
            .map(|slice| [slice[0], slice[1], slice[2], slice[3]])
    }

    /// 读入一张 PNG 作为 RGBA8 帧，用于与当前画面比对。
    pub fn load_png(path: &Path) -> Result<Self> {
        let decoded =
            image::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        Ok(Self::new(width, height, rgba.into_raw()))
    }

    /// 平均每通道绝对差。尺寸不同返回 `None`：那已经是另一张画面。
    pub fn channel_difference(&self, other: &Self) -> Option<f32> {
        if self.width != other.width || self.height != other.height || self.pixels.is_empty() {
            return None;
        }
        let total: u64 = self
            .pixels
            .iter()
            .zip(other.pixels.iter())
            .map(|(a, b)| u64::from((*a as i32 - *b as i32).unsigned_abs()))
            .sum();
        Some(total as f32 / self.pixels.len() as f32)
    }

    /// 任一通道差超过 `tolerance` 的像素占比。
    ///
    /// 平均值容易被大片相同背景拉低，这个比例专门盯"局部画错"。
    pub fn differing_ratio(&self, other: &Self, tolerance: u8) -> Option<f32> {
        if self.width != other.width || self.height != other.height || self.pixels.is_empty() {
            return None;
        }
        let mut differing = 0u32;
        for (a, b) in self
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(other.pixels.as_chunks::<4>().0.iter())
        {
            if a.iter()
                .zip(b.iter())
                .any(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() > u32::from(tolerance))
            {
                differing += 1;
            }
        }
        Some(differing as f32 / (self.pixels.len() / 4) as f32)
    }

    /// 把这一帧写成 PNG。父目录不存在时自动创建。
    pub fn save_png(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
        }
        image::save_buffer(
            path,
            &self.pixels,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )
        .with_context(|| format!("failed to write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_reads_rows_in_order() {
        // 2×2，左上红、右上绿、左下蓝、右下白
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let image = FrameImage::new(2, 2, pixels);
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 255, 0, 255]));
        assert_eq!(image.pixel(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(image.pixel(1, 1), Some([255, 255, 255, 255]));
        assert_eq!(image.pixel(2, 1), None);
        assert_eq!(image.pixel(1, 2), None);
    }
}
