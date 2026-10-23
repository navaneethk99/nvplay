//! Tiny, opt-in samples of the rendered video, shared with the adaptive theme.
use eframe::{
    egui::Color32,
    glow::{self, HasContext},
};
use std::{
    num::NonZeroU32,
    time::{Duration, Instant},
};

pub(crate) type VideoPalette = [Color32; 3];
const WIDTH: i32 = 32;
const HEIGHT: i32 = 18;

#[derive(Default)]
pub(super) struct AmbientSampler {
    pub enabled: bool,
    pub palette: Option<VideoPalette>,
    generation: u64,
    last_sample: Option<Instant>,
    target: Option<(glow::NativeFramebuffer, glow::NativeTexture)>,
    failed: bool,
    dirty: bool,
}

impl AmbientSampler {
    pub fn configure(&mut self, enabled: bool, generation: u64) {
        if self.enabled != enabled || self.generation != generation {
            self.palette = None;
            self.last_sample = None;
            self.dirty = enabled;
        }
        self.enabled = enabled;
        self.generation = generation;
    }

    /// Called only with the renderer's OpenGL context current. Never reads the
    /// full video back to the CPU, and never touches the playback source again.
    pub unsafe fn sample(
        &mut self,
        gl: &glow::Context,
        source: glow::NativeFramebuffer,
        width: i32,
        height: i32,
        new_frame: bool,
    ) {
        self.dirty |= new_frame;
        if !self.enabled
            || self.failed
            || (!self.dirty && self.palette.is_some())
            || self
                .last_sample
                .is_some_and(|last| last.elapsed() < Duration::from_millis(250))
        {
            return;
        }
        self.last_sample = Some(Instant::now());
        // SAFETY: caller owns the current context and the source framebuffer.
        unsafe {
            let read = NonZeroU32::new(gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING) as u32)
                .map(glow::NativeFramebuffer);
            let draw = NonZeroU32::new(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32)
                .map(glow::NativeFramebuffer);
            if self.target.is_none() {
                self.target = create_target(gl);
                if self.target.is_none() {
                    self.failed = true;
                }
            }
            if let Some((fbo, _)) = self.target {
                let scissor = gl.is_enabled(glow::SCISSOR_TEST);
                gl.disable(glow::SCISSOR_TEST);
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(source));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(fbo));
                gl.blit_framebuffer(
                    0,
                    0,
                    width,
                    height,
                    0,
                    0,
                    WIDTH,
                    HEIGHT,
                    glow::COLOR_BUFFER_BIT,
                    glow::LINEAR,
                );
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
                // mpv may leave pack state behind. A CPU slice requires no PBO
                // and tightly packed rows; restore all of it after the read.
                let buffer =
                    NonZeroU32::new(gl.get_parameter_i32(glow::PIXEL_PACK_BUFFER_BINDING) as u32)
                        .map(glow::NativeBuffer);
                let alignment = gl.get_parameter_i32(glow::PACK_ALIGNMENT);
                let row_length = gl.get_parameter_i32(glow::PACK_ROW_LENGTH);
                let skip_pixels = gl.get_parameter_i32(glow::PACK_SKIP_PIXELS);
                let skip_rows = gl.get_parameter_i32(glow::PACK_SKIP_ROWS);
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
                gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 0);
                gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, 0);
                gl.pixel_store_i32(glow::PACK_SKIP_ROWS, 0);
                let mut pixels = [0; WIDTH as usize * HEIGHT as usize * 4];
                gl.read_pixels(
                    0,
                    0,
                    WIDTH,
                    HEIGHT,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixels)),
                );
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, buffer);
                gl.pixel_store_i32(glow::PACK_ALIGNMENT, alignment);
                gl.pixel_store_i32(glow::PACK_ROW_LENGTH, row_length);
                gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, skip_pixels);
                gl.pixel_store_i32(glow::PACK_SKIP_ROWS, skip_rows);
                if scissor {
                    gl.enable(glow::SCISSOR_TEST);
                }
                self.palette = Some(extract_palette(&pixels));
                self.dirty = false;
            }
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, read);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, draw);
        }
    }

    pub unsafe fn destroy(&mut self, gl: &glow::Context) {
        if let Some((fbo, texture)) = self.target.take() {
            // SAFETY: caller retains the owning GL context through destruction.
            unsafe {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(texture);
            }
        }
    }
}

unsafe fn create_target(
    gl: &glow::Context,
) -> Option<(glow::NativeFramebuffer, glow::NativeTexture)> {
    // SAFETY: caller restores framebuffer bindings; texture binding is local.
    unsafe {
        let texture = gl.create_texture().ok()?;
        let fbo = match gl.create_framebuffer() {
            Ok(fbo) => fbo,
            Err(_) => {
                gl.delete_texture(texture);
                return None;
            }
        };
        let previous_texture =
            NonZeroU32::new(gl.get_parameter_i32(glow::TEXTURE_BINDING_2D) as u32)
                .map(glow::NativeTexture);
        let unpack =
            NonZeroU32::new(gl.get_parameter_i32(glow::PIXEL_UNPACK_BUFFER_BINDING) as u32)
                .map(glow::NativeBuffer);
        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            WIDTH,
            HEIGHT,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.bind_texture(glow::TEXTURE_2D, previous_texture);
        gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, unpack);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::DRAW_FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        if gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(texture);
            return None;
        }
        Some((fbo, texture))
    }
}

fn distance(a: Color32, b: Color32) -> i32 {
    [(a.r(), b.r()), (a.g(), b.g()), (a.b(), b.b())]
        .into_iter()
        .map(|(a, b)| (i32::from(a) - i32::from(b)).pow(2))
        .sum()
}

fn extract_palette(pixels: &[u8]) -> VideoPalette {
    // Quantized clusters preserve real scene hues instead of averaging a red
    // object and a blue background into an unrelated purple. Ignore letterbox
    // black, weight population first, and modestly favor saturated colors.
    let mut buckets = [[0_u32; 4]; 512];
    for p in pixels.chunks_exact(4) {
        if p[3] == 0 || *p[..3].iter().max().unwrap() < 18 {
            continue;
        }
        let key =
            (usize::from(p[0] >> 5) << 6) | (usize::from(p[1] >> 5) << 3) | usize::from(p[2] >> 5);
        let bucket = &mut buckets[key];
        for i in 0..3 {
            bucket[i] += u32::from(p[i]);
        }
        bucket[3] += 1;
    }
    let mut candidates: Vec<_> = buckets
        .iter()
        .filter(|b| b[3] > 0)
        .map(|b| {
            let color = Color32::from_rgb(
                (b[0] / b[3]) as u8,
                (b[1] / b[3]) as u8,
                (b[2] / b[3]) as u8,
            );
            let rgb = [color.r(), color.g(), color.b()];
            let saturation =
                f32::from(rgb.iter().max().unwrap() - rgb.iter().min().unwrap()) / 255.0;
            (b[3] as f32 * (0.5 + saturation), color)
        })
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut selected = Vec::with_capacity(3);
    for (_, color) in candidates {
        if selected
            .iter()
            .all(|previous| distance(*previous, color) > 55 * 55)
        {
            selected.push(color);
            if selected.len() == 3 {
                break;
            }
        }
    }
    let primary = selected.first().copied().unwrap_or(Color32::from_gray(12));
    [
        primary,
        selected.get(1).copied().unwrap_or(primary),
        selected.get(2).copied().unwrap_or(primary),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_colors_survive_letterboxing_and_black_scenes_are_neutral() {
        let mut pixels = vec![0; 400 * 4];
        for pixel in pixels.chunks_exact_mut(4).take(100) {
            pixel.copy_from_slice(&[220, 40, 30, 255]);
        }
        for pixel in pixels.chunks_exact_mut(4).skip(100).take(100) {
            pixel.copy_from_slice(&[20, 80, 230, 255]);
        }
        let palette = extract_palette(&pixels);
        assert!(palette.contains(&Color32::from_rgb(220, 40, 30)));
        assert!(palette.contains(&Color32::from_rgb(20, 80, 230)));
        assert_eq!(
            extract_palette(&[0, 0, 0, 255].repeat(100)),
            [Color32::from_gray(12); 3]
        );
        assert_eq!(
            extract_palette(&[120, 120, 120, 255].repeat(100)),
            [Color32::from_gray(120); 3]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires macOS OpenGL; no windows are opened"]
    fn real_gl_sampling_follows_colors_and_restores_gl_state() {
        let native = crate::playback::Offscreen::new();
        let loader = |name: &std::ffi::CStr| native.address(name);
        // SAFETY: this test owns the offscreen context and every object below.
        unsafe {
            let gl = glow::Context::from_loader_function_cstr(loader);
            let source = create_target(&gl).unwrap();
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(source.0));
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(0.9, 0.1, 0.05, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            let mut sampler = AmbientSampler::default();
            sampler.configure(true, 1);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 8);
            gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 48);
            gl.pixel_store_i32(glow::PACK_SKIP_ROWS, 2);
            gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, 3);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(0, 0, 1, 1);
            sampler.sample(&gl, source.0, WIDTH, HEIGHT, true);
            let red = sampler.palette.unwrap()[0];
            assert!(red.r() > 220 && red.b() < 20, "{red:?}");
            assert!(gl.is_enabled(glow::SCISSOR_TEST));
            assert_eq!(
                gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING) as u32,
                source.0.0.get()
            );
            assert_eq!(
                gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) as u32,
                source.0.0.get()
            );
            assert_eq!(gl.get_parameter_i32(glow::PACK_ALIGNMENT), 8);
            assert_eq!(gl.get_parameter_i32(glow::PACK_ROW_LENGTH), 48);
            assert_eq!(gl.get_parameter_i32(glow::PACK_SKIP_ROWS), 2);
            assert_eq!(gl.get_parameter_i32(glow::PACK_SKIP_PIXELS), 3);
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(0.05, 0.15, 0.9, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            sampler.sample(&gl, source.0, WIDTH, HEIGHT, true);
            assert_eq!(
                sampler.palette.unwrap()[0],
                red,
                "sampling must be throttled"
            );
            sampler.last_sample = None;
            sampler.sample(&gl, source.0, WIDTH, HEIGHT, false);
            let blue = sampler.palette.unwrap()[0];
            assert!(blue.b() > 220 && blue.r() < 20, "{blue:?}");
            sampler.configure(false, 1);
            sampler.sample(&gl, source.0, WIDTH, HEIGHT, true);
            assert!(sampler.palette.is_none());
            assert_eq!(gl.get_error(), glow::NO_ERROR);
            sampler.destroy(&gl);
            gl.delete_framebuffer(source.0);
            gl.delete_texture(source.1);
        }
    }

    #[test]
    fn sampling_is_opt_in_and_clears_on_media_changes() {
        let mut sampler = AmbientSampler::default();
        assert!(!sampler.enabled);
        sampler.configure(true, 1);
        sampler.palette = Some([Color32::RED; 3]);
        sampler.configure(true, 1);
        assert!(sampler.palette.is_some());
        sampler.configure(true, 2);
        assert!(sampler.palette.is_none());
        sampler.palette = Some([Color32::RED; 3]);
        sampler.configure(false, 2);
        assert!(sampler.palette.is_none());
    }
}
