use super::{
    Client, Player,
    ffi::{Fbo, GlInit, Param},
};
use eframe::{
    egui, egui_glow,
    glow::{self, HasContext},
};
use std::{
    ffi::{CStr, c_char, c_void},
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

type ProcAddress<'a> = &'a dyn Fn(&CStr) -> *const c_void;

pub struct VideoRenderer {
    client: Arc<Client>,
    context: *mut c_void,
    gl: Arc<glow::Context>,
    target: Option<(glow::NativeFramebuffer, glow::NativeTexture, i32, i32)>,
    wakeup: Box<RenderWake>,
    pub frames_rendered: u64,
    pub frames_reused: u64,
    pub max_render_ms: f64,
    pub error: Option<String>,
}
struct RenderWake {
    dirty: AtomicBool,
    ui: egui::Context,
}
unsafe extern "C" fn frame_ready(data: *mut c_void) {
    // SAFETY: the renderer retains this box until render_context_free completes.
    let wakeup = unsafe { &*data.cast::<RenderWake>() };
    wakeup.dirty.store(true, Ordering::Release);
    wakeup.ui.request_repaint();
}
// SAFETY: egui requires Send for callbacks, but this object is used and freed
// only on eframe's GL thread, with its original OpenGL context current.
unsafe impl Send for VideoRenderer {}

unsafe extern "C" fn get_proc_address(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: ctx points to the stack-held loader for the duration of create.
    // mpv only invokes this callback during render_context_create.
    let loader = unsafe { &*ctx.cast::<ProcAddress<'_>>() };
    loader(unsafe { CStr::from_ptr(name) }).cast_mut()
}

impl VideoRenderer {
    pub fn new(player: &Player, cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let gl = cc.gl.clone().ok_or("An OpenGL context is required")?;
        let loader = cc
            .get_proc_address
            .ok_or("OpenGL function loader unavailable")?;
        Self::create(player, gl, loader, &cc.egui_ctx)
    }

    pub(crate) fn create(
        player: &Player,
        gl: Arc<glow::Context>,
        loader: ProcAddress<'_>,
        ui: &egui::Context,
    ) -> Result<Self, String> {
        let mut init = GlInit {
            get_proc_address,
            ctx: (&loader as *const ProcAddress<'_>).cast_mut().cast(),
        };
        let mut params = [
            Param {
                kind: 1,
                data: c"opengl".as_ptr().cast_mut().cast(),
            },
            Param::new(2, &mut init),
            Param::end(),
        ];
        let mut context = std::ptr::null_mut();
        // SAFETY: eframe's GL context is current during creation. Stack parameters
        // remain live throughout the call; mpv does not retain the loader.
        player.client.api.check(unsafe {
            (player.client.api.render_context_create)(
                &mut context,
                player.client.handle,
                params.as_mut_ptr(),
            )
        })?;
        let mut wakeup = Box::new(RenderWake {
            dirty: AtomicBool::new(true),
            ui: ui.clone(),
        });
        // SAFETY: this callback only sets an atomic and wakes egui. It does not
        // render or call any mpv API from mpv's callback thread.
        unsafe {
            (player.client.api.render_context_set_update_callback)(
                context,
                Some(frame_ready),
                (&mut *wakeup as *mut RenderWake).cast(),
            )
        };
        Ok(Self {
            client: player.client.clone(),
            context,
            gl,
            target: None,
            wakeup,
            frames_rendered: 0,
            frames_reused: 0,
            max_render_ms: 0.0,
            error: None,
        })
    }

    pub fn callback(renderer: Arc<Mutex<Option<Self>>>, rect: egui::Rect) -> egui::PaintCallback {
        egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, _painter| {
                if let Ok(mut renderer) = renderer.lock()
                    && let Some(renderer) = renderer.as_mut()
                {
                    renderer.paint(info);
                }
            })),
        }
    }

    fn paint(&mut self, info: egui::PaintCallbackInfo) {
        let viewport = info.viewport_in_pixels();
        let (width, height) = (viewport.width_px, viewport.height_px);
        if width <= 0 || height <= 0 {
            return;
        }
        // SAFETY: callback executes on the current GL context; all objects below
        // belong to it. Use a private FBO so mpv cannot paint over the UI.
        unsafe {
            let previous = self.gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
            let previous = NonZeroU32::new(previous as u32).map(glow::NativeFramebuffer);
            let resized = self
                .target
                .as_ref()
                .is_none_or(|t| t.2 != width || t.3 != height);
            let dirty = self.wakeup.dirty.swap(false, Ordering::AcqRel);
            let updates = if dirty {
                (self.client.api.render_context_update)(self.context)
            } else {
                0
            };
            if resized {
                self.delete_target();
                let texture = match self.gl.create_texture() {
                    Ok(texture) => texture,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
                let fbo = match self.gl.create_framebuffer() {
                    Ok(fbo) => fbo,
                    Err(error) => {
                        self.gl.delete_texture(texture);
                        self.error = Some(error);
                        return;
                    }
                };
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    width,
                    height,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                self.gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                self.gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                if self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE
                {
                    self.gl.delete_framebuffer(fbo);
                    self.gl.delete_texture(texture);
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER, previous);
                    self.error = Some("Video framebuffer is incomplete".into());
                    return;
                }
                self.gl.bind_texture(glow::TEXTURE_2D, None);
                self.target = Some((fbo, texture, width, height));
            }
            let target = self.target.as_ref().unwrap();
            let mut fbo = Fbo {
                fbo: target.0.0.get() as i32,
                width,
                height,
                internal_format: glow::RGBA8 as i32,
            };
            let mut flip = 1_i32;
            // The player sets video-timing-offset=0, so callbacks arrive when
            // frames are due. Keep rendering nonblocking; eframe owns vsync.
            let mut block = 0_i32;
            let mut params = [
                Param::new(3, &mut fbo),
                Param::new(4, &mut flip),
                Param::new(12, &mut block),
                Param::end(),
            ];
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.disable(glow::BLEND);
            // mpv only renders new frames or a resized surface. UI hover/menu
            // repaints reuse the texture instead of invoking the video renderer.
            if resized || updates & 1 != 0 {
                let start = Instant::now();
                let result =
                    (self.client.api.render_context_render)(self.context, params.as_mut_ptr());
                self.max_render_ms = self
                    .max_render_ms
                    .max(start.elapsed().as_secs_f64() * 1000.0);
                self.frames_rendered += 1;
                if let Err(error) = self.client.api.check(result) {
                    self.error = Some(error);
                }
            } else {
                self.frames_reused += 1;
            }
            self.gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.0));
            self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous);
            let clip = info.clip_rect_in_pixels();
            self.gl.enable(glow::SCISSOR_TEST);
            self.gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            self.gl.blit_framebuffer(
                0,
                0,
                width,
                height,
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.left_px + width,
                viewport.from_bottom_px + height,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, previous);
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_sampler(0, None);
        }
    }

    unsafe fn delete_target(&mut self) {
        if let Some((fbo, texture, _, _)) = self.target.take() {
            unsafe {
                self.gl.delete_framebuffer(fbo);
                self.gl.delete_texture(texture);
            }
        }
    }
}
impl Drop for VideoRenderer {
    fn drop(&mut self) {
        // SAFETY: PlayerApp releases the renderer in on_exit while GL is current,
        // before the client handle or dynamically loaded library can be dropped.
        unsafe {
            (self.client.api.render_context_set_update_callback)(
                self.context,
                None,
                std::ptr::null_mut(),
            );
            (self.client.api.render_context_free)(self.context);
            self.delete_target();
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) mod tests {
    use super::*;
    use crate::playback::ffi::FrameInfo;
    use std::{ffi::c_int, time::Duration};

    // CGL creates an offscreen context; this test opens no window and does not
    // interact with other applications. Run explicitly with a generated clip.
    #[link(name = "OpenGL", kind = "framework")]
    unsafe extern "C" {
        fn CGLChoosePixelFormat(
            attributes: *const c_int,
            format: *mut *mut c_void,
            count: *mut c_int,
        ) -> c_int;
        fn CGLCreateContext(
            format: *mut c_void,
            share: *mut c_void,
            context: *mut *mut c_void,
        ) -> c_int;
        fn CGLSetCurrentContext(context: *mut c_void) -> c_int;
        fn CGLDestroyPixelFormat(format: *mut c_void) -> c_int;
        fn CGLDestroyContext(context: *mut c_void) -> c_int;
    }
    pub(crate) struct Offscreen {
        context: *mut c_void,
        library: libloading::Library,
    }
    impl Offscreen {
        pub(crate) fn new() -> Self {
            // SAFETY: CGL attribute list is terminated; returned objects are
            // confined to this test thread and destroyed with the context.
            unsafe {
                let mut format = std::ptr::null_mut();
                let mut count = 0;
                assert_eq!(
                    CGLChoosePixelFormat([99, 0x3200, 73, 0].as_ptr(), &mut format, &mut count),
                    0
                );
                assert!(!format.is_null());
                let mut context = std::ptr::null_mut();
                assert_eq!(
                    CGLCreateContext(format, std::ptr::null_mut(), &mut context),
                    0
                );
                CGLDestroyPixelFormat(format);
                assert_eq!(CGLSetCurrentContext(context), 0);
                Self {
                    context,
                    library: libloading::Library::new(
                        "/System/Library/Frameworks/OpenGL.framework/OpenGL",
                    )
                    .unwrap(),
                }
            }
        }
        pub(crate) fn address(&self, name: &CStr) -> *const c_void {
            // SAFETY: all function pointers belong to the retained OpenGL library.
            unsafe {
                self.library
                    .get::<*const c_void>(name.to_bytes_with_nul())
                    .map(|symbol| *symbol)
                    .unwrap_or(std::ptr::null())
            }
        }
    }
    impl Drop for Offscreen {
        fn drop(&mut self) {
            // SAFETY: test-only context, with no remaining renderer or GL objects.
            unsafe {
                CGLSetCurrentContext(std::ptr::null_mut());
                CGLDestroyContext(self.context);
            }
        }
    }

    fn paint(renderer: &mut VideoRenderer) {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 720.0));
        renderer.paint(egui::PaintCallbackInfo {
            viewport: rect,
            clip_rect: rect,
            pixels_per_point: 1.0,
            screen_size_px: [1280, 720],
        });
    }
    fn pixels(gl: &glow::Context) -> Vec<u8> {
        let mut bytes = vec![0; 1280 * 720 * 4];
        // SAFETY: destination FBO is current and the buffer has the exact size.
        unsafe {
            gl.read_pixels(
                0,
                0,
                1280,
                720,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
        }
        bytes
    }

    #[test]
    #[ignore = "requires macOS OpenGL, libmpv and NVPLAY_TEST_MEDIA; no windows are opened"]
    fn real_gl_frame_timing() {
        let native = Offscreen::new();
        let loader = |name: &CStr| native.address(name);
        // SAFETY: the test's CGL context is current for all GL calls.
        let gl = Arc::new(unsafe { glow::Context::from_loader_function_cstr(loader) });
        let ctx = egui::Context::default();
        let audio = if std::env::var("NVPLAY_TEST_AUDIO").as_deref() == Ok("coreaudio") {
            "coreaudio"
        } else {
            "null"
        };
        let mut player = Player::create("libmpv", Some(audio), Some(&ctx)).unwrap();
        let mut renderer = VideoRenderer::create(&player, gl.clone(), &loader, &ctx).unwrap();
        let media = std::env::var("NVPLAY_TEST_MEDIA").unwrap();
        player.load(&media).unwrap();
        // The destination is another FBO, replacing the normal window buffer.
        let destination = unsafe {
            let texture = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1280,
                720,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            let fbo = gl.create_framebuffer().unwrap();
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            assert_eq!(
                gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE
            );
            (fbo, texture)
        };
        let mut timing = Vec::new();
        let started = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && player.state.position < 4.0 {
            for event in player.poll() {
                if let crate::playback::PlaybackEvent::Error(error) = event {
                    panic!("{error}");
                }
            }
            if renderer.wakeup.dirty.load(Ordering::Acquire) {
                let mut frame = FrameInfo::default();
                let result = unsafe {
                    (player.client.api.render_context_get_info)(
                        renderer.context,
                        Param::new(11, &mut frame),
                    )
                };
                assert_eq!(result, 0);
                let before = renderer.frames_rendered;
                paint(&mut renderer);
                unsafe {
                    gl.finish();
                }
                if renderer.frames_rendered > before
                    && frame.flags & 1 != 0
                    && frame.flags & 2 == 0
                    && frame.target_time > 0
                {
                    timing.push(
                        player
                            .client
                            .api
                            .frame_time_us(player.client.handle, frame.target_time),
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        player.poll();
        assert!(
            player.state.position >= 4.0,
            "GPU playback did not advance: {:?}",
            player.state
        );
        eprintln!(
            "Real-time check: {:.2} media seconds in {:.2} wall seconds, speed {:.2}, audio {}",
            player.state.position,
            started.elapsed().as_secs_f64(),
            player.state.speed,
            audio
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "playback fell behind real time"
        );
        assert!((player.state.speed - 1.0).abs() < 0.01);
        assert!(
            timing.len() >= 50,
            "too few rendered frames: {}",
            timing.len()
        );
        timing.sort_unstable();
        eprintln!(
            "GPU timing: {} frames, p10 {:.2}ms, median {:.2}ms, p90 {:.2}ms from presentation target; drops {}/{}, decoder {}, longest render {:.2}ms",
            timing.len(),
            timing[timing.len() / 10] as f64 / 1000.0,
            timing[timing.len() / 2] as f64 / 1000.0,
            timing[timing.len() * 9 / 10] as f64 / 1000.0,
            player.state.dropped_frames,
            player.state.decoder_dropped_frames,
            player.state.hardware_decoder,
            renderer.max_render_ms
        );
        // Catch the original regression: nonblocking renders must not display
        // a stream of frames tens of milliseconds before their target times.
        assert!(
            timing[timing.len() / 10] >= -2000,
            "video frames were rendered before their presentation targets"
        );
        assert!(
            player.state.dropped_frames <= 2,
            "too many GPU output drops: {}",
            player.state.dropped_frames
        );
        player.command(&["set", "pause", "yes"]).unwrap();
        let settle = Instant::now() + Duration::from_millis(300);
        while Instant::now() < settle {
            player.poll();
            paint(&mut renderer);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(player.state.paused);
        let paused = pixels(&gl);
        let rendered = renderer.frames_rendered;
        let reused = renderer.frames_reused;
        for _ in 0..10 {
            paint(&mut renderer);
        }
        assert_eq!(
            renderer.frames_rendered, rendered,
            "paused UI repaints re-rendered video"
        );
        assert!(renderer.frames_reused >= reused + 10);
        assert!(paused == pixels(&gl), "paused GPU pixels changed");
        player.command(&["seek", "2", "absolute+exact"]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            player.poll();
            paint(&mut renderer);
            if (player.state.position - 2.0).abs() < 0.1
                && renderer.frames_rendered > rendered
                && paused != pixels(&gl)
            {
                break;
            }
            assert!(Instant::now() < deadline, "paused GPU seek did not redraw");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(player.state.paused);
        assert!(paused != pixels(&gl), "GPU seek displayed the old frame");
        drop(renderer);
        drop(player);
        unsafe {
            gl.delete_framebuffer(destination.0);
            gl.delete_texture(destination.1);
        }
    }
}
