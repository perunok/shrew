//! Glyph atlas: an R8 coverage texture for ordinary glyphs and an RGBA texture for color glyphs (emoji), packed with
//! etagere and keyed by cosmic-text's [`CacheKey`] (font, glyph, size, subpixel x bin, weight, flags).
//!
//! Glyphs are rasterized with swash on first use and uploaded one by one with `write_texture`; a frame that hits only
//! cached glyphs uploads nothing. A full texture doubles (copying what it holds) up to the device limit; past that
//! the renderer clears the atlas and rebuilds the frame.

use cosmic_text::{CacheKey, SwashContent, SwashImage};
use etagere::{BucketedAtlasAllocator, size2};
use rustc_hash::FxHashMap;

use crate::fonts::Fonts;

#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphEntry {
    /// Top-left texel in its atlas.
    pub uv: [u16; 2],
    pub size: [u16; 2],
    /// Bitmap offset from the pen position: `left` to the right, `top` up from the baseline.
    pub left: i16,
    pub top: i16,
    pub color: bool,
}

/// The atlas cannot fit another glyph even at its maximum size.
#[derive(Debug)]
pub(crate) struct AtlasFull;

pub(crate) struct Page {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    alloc: BucketedAtlasAllocator,
    format: wgpu::TextureFormat,
    bpp: u32,
    size: u32,
}

impl Page {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, bpp: u32, size: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shrew glyph atlas"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Page { texture, view, alloc: BucketedAtlasAllocator::new(size2(size as i32, size as i32)), format, bpp, size }
    }

    /// Doubles the texture, keeping every allocation and its pixels.
    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let size = self.size * 2;
        let mut bigger = Page::new(device, self.format, self.bpp, size);
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shrew atlas grow") });
        enc.copy_texture_to_texture(
            self.texture.as_image_copy(),
            bigger.texture.as_image_copy(),
            wgpu::Extent3d { width: self.size, height: self.size, depth_or_array_layers: 1 },
        );
        // Pending write_texture calls on the old texture are flushed ahead of this submission, so the copy sees them.
        queue.submit([enc.finish()]);
        bigger.alloc = std::mem::replace(&mut self.alloc, BucketedAtlasAllocator::new(size2(1, 1)));
        bigger.alloc.grow(size2(size as i32, size as i32));
        *self = bigger;
        log::debug!("glyph atlas grown to {size}x{size} ({:?})", self.format);
    }

    fn upload(&self, queue: &wgpu::Queue, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * self.bpp), rows_per_image: None },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }
}

pub(crate) struct Atlas {
    pub mask: Page,
    pub color: Page,
    glyphs: FxHashMap<CacheKey, Option<GlyphEntry>>,
    pub max_size: u32,
    /// Bumped whenever a texture is replaced, so the renderer knows to rebuild its bind group.
    pub generation: u64,
    /// Bumped whenever entries are dropped, invalidating placements cached elsewhere.
    pub epoch: u64,
    pub hits: u32,
    pub misses: u32,
    rgba: Vec<u8>,
}

impl Atlas {
    pub fn new(device: &wgpu::Device) -> Self {
        // 4096^2 is 16 MB of coverage (or 64 MB of RGBA); past that a reset is cheaper than more memory.
        let max_size = device.limits().max_texture_dimension_2d.min(4096);
        Atlas {
            mask: Page::new(device, wgpu::TextureFormat::R8Unorm, 1, 1024.min(max_size)),
            // Color glyphs are stored and blended as sRGB-encoded values, like every other color (see the shader).
            color: Page::new(device, wgpu::TextureFormat::Rgba8Unorm, 4, 256.min(max_size)),
            glyphs: FxHashMap::default(),
            max_size,
            generation: 0,
            epoch: 0,
            hits: 0,
            misses: 0,
            rgba: Vec::new(),
        }
    }

    /// Forgets every glyph (textures are kept and overwritten).
    pub fn clear(&mut self) {
        self.epoch += 1;
        self.glyphs.clear();
        self.mask.alloc.clear();
        self.color.alloc.clear();
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    #[cfg(test)]
    pub fn mask_size(&self) -> u32 {
        self.mask.size
    }

    /// Adds glyphs rasterized ahead of time (coverage masks only) with a single texture upload.
    pub fn insert_prerendered(&mut self, glyphs: &[(CacheKey, SwashImage)], queue: &wgpu::Queue) {
        // The single bounding-box upload would wipe glyphs already in that box, so it needs a fresh page.
        let fresh = self.mask.alloc.is_empty();
        let mut placed = Vec::with_capacity(glyphs.len());
        let (mut x1, mut y1) = (0u32, 0u32);
        for (key, img) in glyphs {
            let (w, h) = (img.placement.width, img.placement.height);
            if self.glyphs.contains_key(key) || img.content != SwashContent::Mask {
                continue;
            }
            if w == 0 || h == 0 {
                self.glyphs.insert(*key, None);
                continue;
            }
            let Some(a) = self.mask.alloc.allocate(size2(w as i32, h as i32)) else { break };
            let (x, y) = (a.rectangle.min.x as u32, a.rectangle.min.y as u32);
            x1 = x1.max(x + w);
            y1 = y1.max(y + h);
            let e = GlyphEntry {
                uv: [x as u16, y as u16],
                size: [w as u16, h as u16],
                left: img.placement.left as i16,
                top: img.placement.top as i16,
                color: false,
            };
            self.glyphs.insert(*key, Some(e));
            placed.push((x, y, img));
        }
        if placed.is_empty() {
            return;
        }
        if !fresh {
            for (x, y, img) in placed {
                self.mask.upload(queue, x, y, img.placement.width, img.placement.height, &img.data);
            }
            return;
        }
        // One upload of the bounding box (etagere fills from the top-left, so it is compact).
        let mut buf = vec![0u8; (x1 * y1) as usize];
        for (x, y, img) in placed {
            let w = img.placement.width as usize;
            for (row, src) in img.data.chunks_exact(w).enumerate() {
                let at = (y as usize + row) * x1 as usize + x as usize;
                buf[at..at + w].copy_from_slice(src);
            }
        }
        self.mask.upload(queue, 0, 0, x1, y1, &buf);
    }

    /// The atlas entry for a glyph, rasterizing and uploading it on first use. `None` for blank glyphs (spaces).
    pub fn get(
        &mut self,
        key: CacheKey,
        fonts: &mut Fonts,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Option<GlyphEntry>, AtlasFull> {
        if let Some(e) = self.glyphs.get(&key) {
            self.hits += 1;
            return Ok(*e);
        }
        self.misses += 1;
        let Some(img) = fonts.swash.get_image_uncached(&mut fonts.system, key) else {
            self.glyphs.insert(key, None);
            return Ok(None);
        };
        let (w, h) = (img.placement.width, img.placement.height);
        if w == 0 || h == 0 || w > self.max_size || h > self.max_size {
            self.glyphs.insert(key, None);
            return Ok(None);
        }
        let (color, data): (bool, &[u8]) = match img.content {
            SwashContent::Mask => (false, &img.data),
            SwashContent::Color => {
                // Color outlines come out premultiplied, decoded bitmaps (CBDT/sbix PNGs) straight: a channel above
                // its alpha gives the latter away. The atlas stores premultiplied.
                self.rgba.clear();
                self.rgba.extend_from_slice(&img.data);
                if self.rgba.as_chunks::<4>().0.iter().any(|p| p[0].max(p[1]).max(p[2]) > p[3]) {
                    for p in self.rgba.as_chunks_mut::<4>().0 {
                        let a = p[3] as u16;
                        for c in &mut p[..3] {
                            *c = ((*c as u16 * a + 127) / 255) as u8;
                        }
                    }
                }
                (true, &self.rgba)
            }
            SwashContent::SubpixelMask => {
                // Not requested by cosmic-text today; fold RGB coverage to gray just in case.
                self.rgba.clear();
                self.rgba.extend(
                    img.data.as_chunks::<4>().0.iter().map(|p| ((p[0] as u16 + p[1] as u16 + p[2] as u16) / 3) as u8),
                );
                (false, &self.rgba)
            }
        };
        let page = if color { &mut self.color } else { &mut self.mask };
        let alloc = loop {
            if let Some(a) = page.alloc.allocate(size2(w as i32, h as i32)) {
                break a;
            }
            // Color glyphs are big and rare: their page stops at a quarter of the area.
            let limit = if color { self.max_size / 2 } else { self.max_size };
            if page.size >= limit.max(256) {
                return Err(AtlasFull);
            }
            page.grow(device, queue);
            self.generation += 1;
        };
        let (x, y) = (alloc.rectangle.min.x as u32, alloc.rectangle.min.y as u32);
        page.upload(queue, x, y, w, h, data);
        let e = GlyphEntry {
            uv: [x as u16, y as u16],
            size: [w as u16, h as u16],
            left: img.placement.left as i16,
            top: img.placement.top as i16,
            color,
        };
        self.glyphs.insert(key, Some(e));
        Ok(Some(e))
    }
}
