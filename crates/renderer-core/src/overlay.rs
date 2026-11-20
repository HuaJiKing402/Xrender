use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use glyphon::{
    fontdb, Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::{Device, MultisampleState, Queue, RenderPass, TextureFormat};

use crate::fps::FpsCounter;

const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 10.0;
const OUTLINE: f32 = 1.5;
const REFRESH: Duration = Duration::from_millis(250);

/// 屏幕空间 FPS 文字层。文字画在现有 mesh pass 末尾，深度比较恒通过。
pub(crate) struct FpsOverlay {
    font_system: FontSystem,
    swash_cache: SwashCache,
    // 由 TextAtlas/TextRenderer 共享；保留字段确保缓存生命周期覆盖渲染器
    #[allow(dead_code)]
    cache: Cache,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
    buffer: Buffer,
    family: Option<String>,
    counter: FpsCounter,
    text: String,
    resolution: (u32, u32),
    next_refresh: Instant,
    enabled: bool,
}

impl FpsOverlay {
    pub fn new(
        device: &Device,
        queue: &Queue,
        format: TextureFormat,
        size: (u32, u32),
        font_path: Option<&Path>,
    ) -> Self {
        let (mut font_system, family, enabled) = build_font_system(font_path);
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(26.0, 32.0));
        buffer.set_size(Some(size.0.max(1) as f32), Some(size.1.max(1) as f32));

        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text_renderer = TextRenderer::new(
            &mut atlas,
            device,
            MultisampleState::default(),
            Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
        );

        let mut overlay = Self {
            font_system,
            swash_cache: SwashCache::new(),
            cache,
            atlas,
            text_renderer,
            viewport,
            buffer,
            family,
            counter: FpsCounter::new(),
            text: format_fps_text(None),
            resolution: (size.0.max(1), size.1.max(1)),
            next_refresh: Instant::now(),
            enabled,
        };
        overlay.reshape();
        overlay
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn tick(&mut self, now: Instant) {
        self.counter.tick(now);
    }

    pub fn fps(&self) -> Option<f32> {
        self.counter.value()
    }

    pub fn update(&mut self, queue: &Queue, size: (u32, u32), now: Instant) {
        let size = (size.0.max(1), size.1.max(1));
        if self.resolution != size {
            self.resolution = size;
            self.viewport.update(
                queue,
                Resolution {
                    width: size.0,
                    height: size.1,
                },
            );
            self.reshape();
            self.next_refresh = now;
        }

        if now >= self.next_refresh {
            let text = format_fps_text(self.counter.value());
            if text != self.text {
                self.text = text;
                self.reshape();
            }
            self.next_refresh = now + REFRESH;
        }
    }

    pub fn prepare(&mut self, device: &Device, queue: &Queue) -> Result<()> {
        if !self.enabled || self.text.is_empty() {
            return Ok(());
        }

        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: self.resolution.0 as i32,
            bottom: self.resolution.1 as i32,
        };
        let offsets = [
            (-OUTLINE, 0.0),
            (OUTLINE, 0.0),
            (0.0, -OUTLINE),
            (0.0, OUTLINE),
            (-OUTLINE, -OUTLINE),
            (OUTLINE, -OUTLINE),
            (-OUTLINE, OUTLINE),
            (OUTLINE, OUTLINE),
        ];
        let mut areas = Vec::with_capacity(9);
        for (dx, dy) in offsets {
            areas.push(TextArea {
                buffer: &self.buffer,
                left: PAD_X + dx,
                top: PAD_Y + dy,
                scale: 1.0,
                bounds,
                default_color: Color::rgb(0, 0, 0),
                custom_glyphs: &[],
            });
        }
        areas.push(TextArea {
            buffer: &self.buffer,
            left: PAD_X,
            top: PAD_Y,
            scale: 1.0,
            bounds,
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: &[],
        });

        self.text_renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .map_err(|error| anyhow!("text prepare failed: {error:?}"))
    }

    pub fn render(&self, pass: &mut RenderPass<'_>) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.text_renderer
            .render(&self.atlas, &self.viewport, pass)
            .map_err(|error| anyhow!("text render failed: {error:?}"))
    }

    fn reshape(&mut self) {
        let family = self.family.clone();
        let attrs = match family.as_deref() {
            Some(name) => Attrs::new().family(Family::Name(name)),
            None => Attrs::new().family(Family::Monospace),
        };
        self.buffer
            .set_text(&self.text, &attrs, Shaping::Basic, None);
        self.buffer.set_size(
            Some(self.resolution.0 as f32),
            Some(self.resolution.1 as f32),
        );
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }
}

fn build_font_system(font_path: Option<&Path>) -> (FontSystem, Option<String>, bool) {
    if let Some(path) = font_path {
        match std::fs::read(path) {
            Ok(data) => {
                let mut db = fontdb::Database::new();
                db.load_font_data(data);
                let family = db
                    .faces()
                    .next()
                    .and_then(|face| face.families.first().map(|(name, _)| name.clone()));
                if db.is_empty() {
                    log::warn!("font {} contains no usable faces", path.display());
                } else {
                    log::info!(
                        "loaded OpenType font {} ({})",
                        path.display(),
                        family.as_deref().unwrap_or("unnamed")
                    );
                    let font_system = FontSystem::new_with_locale_and_db("en".into(), db);
                    return (font_system, family, true);
                }
            }
            Err(error) => {
                log::warn!("failed to read font {}: {error}", path.display());
            }
        }
    }

    let font_system = FontSystem::new();
    let enabled = !font_system.db().is_empty();
    if !enabled {
        log::warn!("no system font available for FPS overlay");
    }
    (font_system, None, enabled)
}

fn format_fps_text(fps: Option<f32>) -> String {
    match fps {
        Some(value) => format!("FPS: {:>3}", value.round() as u32),
        None => "FPS: ---".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::format_fps_text;

    #[test]
    fn formats_known_and_unknown_fps() {
        assert_eq!(format_fps_text(None), "FPS: ---");
        assert_eq!(format_fps_text(Some(59.6)), "FPS:  60");
        assert_eq!(format_fps_text(Some(30.2)), "FPS:  30");
    }
}
