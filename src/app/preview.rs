//! A separate, bounded thumbnail decoder never seeks or pauses the player.
use eframe::egui;
use std::{
    collections::VecDeque,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const SIZE: [usize; 2] = [160, 90];
const BYTES: usize = SIZE[0] * SIZE[1] * 4;
#[derive(Clone, PartialEq, Eq)]
struct Request {
    source: String,
    millis: u64,
    track: usize,
}
struct Frame {
    request: Request,
    pixels: Option<Vec<u8>>,
}
pub(super) struct Preview {
    sender: Option<mpsc::Sender<Request>>,
    receiver: Option<mpsc::Receiver<Frame>>,
    cache: VecDeque<(Request, Option<egui::TextureHandle>)>,
    requested: Option<Request>,
    last_request: Instant,
    ffmpeg: Option<PathBuf>,
}
impl Default for Preview {
    fn default() -> Self {
        Self {
            sender: None,
            receiver: None,
            cache: VecDeque::new(),
            requested: None,
            last_request: Instant::now() - Duration::from_secs(1),
            ffmpeg: crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg"),
        }
    }
}
impl Preview {
    #[cfg(test)]
    pub(super) fn has_frame(&self) -> bool {
        self.cache.iter().any(|(_, texture)| texture.is_some())
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, source: &str, seconds: f64, track: usize) {
        if self.ffmpeg.is_none() {
            ui.label("Install FFmpeg to preview frames");
            return;
        }
        if source.contains("://") || !Path::new(source).is_file() {
            ui.label("Frame preview unavailable for this stream");
            return;
        }
        if self.sender.is_none() {
            let (sender, requests) = mpsc::channel::<Request>();
            let (frames, receiver) = mpsc::channel();
            let ffmpeg = self.ffmpeg.clone().unwrap();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                while let Ok(mut request) = requests.recv() {
                    // Discard intermediate pointer positions while a frame decodes.
                    while let Ok(newer) = requests.try_recv() {
                        request = newer;
                    }
                    let pixels = extract(&ffmpeg, &request);
                    if frames.send(Frame { request, pixels }).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        if let Some(receiver) = &self.receiver {
            while let Ok(frame) = receiver.try_recv() {
                let texture = frame.pixels.map(|pixels| {
                    ui.ctx().load_texture(
                        "seek-preview",
                        egui::ColorImage::from_rgba_unmultiplied(SIZE, &pixels),
                        egui::TextureOptions::LINEAR,
                    )
                });
                self.cache.push_back((frame.request, texture));
                while self.cache.len() > 48 {
                    self.cache.pop_front();
                }
            }
        }
        let request = Request {
            source: source.into(),
            millis: (seconds * 1000.0).round() as u64,
            track,
        };
        if let Some((_, texture)) = self.cache.iter().find(|(key, _)| *key == request) {
            if let Some(texture) = texture {
                ui.image((texture.id(), egui::vec2(160.0, 90.0)));
            } else {
                ui.label("No video frame at this position");
            }
        } else {
            ui.allocate_ui(egui::vec2(160.0, 90.0), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                });
            });
            if self.requested.as_ref() != Some(&request)
                && self.last_request.elapsed() >= Duration::from_millis(100)
            {
                let _ = self.sender.as_ref().unwrap().send(request.clone());
                self.requested = Some(request);
                self.last_request = Instant::now();
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }
}
fn extract(ffmpeg: &Path, request: &Request) -> Option<Vec<u8>> {
    let mut command = Command::new(ffmpeg);
    command.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-threads", "1", "-ss"])
        .arg(format!("{:.3}", request.millis as f64 / 1000.0))
        .arg("-i").arg(&request.source).args(["-map", &format!("0:v:{}", request.track), "-frames:v", "1", "-an", "-sn", "-vf", "scale=160:90:force_original_aspect_ratio=decrease,pad=160:90:(ow-iw)/2:(oh-ih)/2,setsar=1", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take().unwrap();
    // Drain concurrently: raw pixels can exceed an OS pipe's capacity.
    let reader = std::thread::spawn(move || {
        let mut pixels = Vec::with_capacity(BYTES);
        stdout
            .take(BYTES as u64 + 1)
            .read_to_end(&mut pixels)
            .ok()?;
        (pixels.len() == BYTES).then_some(pixels)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let pixels = reader.join().ok().flatten();
    if success { pixels } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires ffmpeg and NVPLAY_TEST_MEDIA"]
    fn real_preview_decodes_distinct_frames() {
        let ffmpeg = crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg").unwrap();
        let source = std::env::var("NVPLAY_TEST_MEDIA").unwrap();
        let frame = |millis| {
            extract(
                &ffmpeg,
                &Request {
                    source: source.clone(),
                    millis,
                    track: 0,
                },
            )
            .expect("decoded thumbnail")
        };
        let first = frame(1000);
        let second = frame(10000);
        assert_eq!(first.len(), BYTES);
        assert_ne!(first, second);
    }
}
