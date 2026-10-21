//! A bounded background decoder prewarms the timeline without touching playback.
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
const OVERVIEW_LIMIT: usize = 96;
const EXACT_LIMIT: usize = 64;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    source: String,
    millis: u64,
    track: usize,
}
enum Work {
    Prepare(VecDeque<Request>),
    Hover(Request),
}
struct Frame {
    request: Request,
    pixels: Option<Vec<u8>>,
    overview: bool,
}
type Cache = VecDeque<(Request, Option<egui::TextureHandle>)>;
pub(super) struct Preview {
    sender: Option<mpsc::Sender<Work>>,
    receiver: Option<mpsc::Receiver<Frame>>,
    cache: Cache,
    overview: Cache,
    active: Option<(String, usize)>,
    requested: Option<Request>,
    ffmpeg: Option<PathBuf>,
}
impl Default for Preview {
    fn default() -> Self {
        Self {
            sender: None,
            receiver: None,
            cache: VecDeque::new(),
            overview: VecDeque::new(),
            active: None,
            requested: None,
            ffmpeg: crate::streaming::find_tool("NVPLAY_FFMPEG", "ffmpeg"),
        }
    }
}
impl Preview {
    #[cfg(test)]
    pub(super) fn has_frame(&self) -> bool {
        self.cache
            .iter()
            .chain(self.overview.iter())
            .any(|(_, texture)| texture.is_some())
    }

    pub(super) fn prepare(
        &mut self,
        ctx: &egui::Context,
        source: &str,
        duration: f64,
        track: usize,
    ) {
        if self.ffmpeg.is_none()
            || !duration.is_finite()
            || duration <= 0.0
            || source.contains("://")
            || !Path::new(source).is_file()
        {
            if self.active.take().is_some() {
                self.cache.clear();
                self.overview.clear();
                self.requested = None;
                if let Some(sender) = &self.sender {
                    let _ = sender.send(Work::Prepare(VecDeque::new()));
                }
            }
            self.collect(ctx);
            return;
        }
        if self.sender.is_none() {
            let (sender, requests) = mpsc::channel();
            // Bound both CPU pixel backlog and GPU texture memory.
            let (frames, receiver) = mpsc::sync_channel(8);
            let ffmpeg = self.ffmpeg.clone().unwrap();
            let ctx = ctx.clone();
            std::thread::spawn(move || decode_worker(&ffmpeg, requests, frames, ctx));
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        if self
            .active
            .as_ref()
            .is_none_or(|(active, active_track)| active != source || *active_track != track)
        {
            self.cache.clear();
            self.overview.clear();
            self.requested = None;
            self.active = Some((source.into(), track));
            let _ = self
                .sender
                .as_ref()
                .unwrap()
                .send(Work::Prepare(overview_requests(source, duration, track)));
        }
        self.collect(ctx);
    }

    fn collect(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.receiver {
            while let Ok(frame) = receiver.try_recv() {
                if self.active.as_ref().is_none_or(|(source, track)| {
                    *source != frame.request.source || *track != frame.request.track
                }) {
                    continue;
                }
                let texture = frame.pixels.map(|pixels| {
                    ctx.load_texture(
                        "seek-preview",
                        egui::ColorImage::from_rgba_unmultiplied(SIZE, &pixels),
                        egui::TextureOptions::LINEAR,
                    )
                });
                let (cache, limit) = if frame.overview {
                    (&mut self.overview, OVERVIEW_LIMIT)
                } else {
                    (&mut self.cache, EXACT_LIMIT)
                };
                cache.push_back((frame.request, texture));
                while cache.len() > limit {
                    cache.pop_front();
                }
            }
        }
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
        // Also supports callers that have not started prewarming yet.
        if self
            .active
            .as_ref()
            .is_none_or(|(active, active_track)| active != source || *active_track != track)
        {
            self.prepare(ui.ctx(), source, seconds.max(1.0), track);
        }
        self.collect(ui.ctx());
        let request = Request {
            source: source.into(),
            millis: (seconds * 1000.0).round() as u64,
            track,
        };
        let exact = self
            .cache
            .iter()
            .chain(self.overview.iter())
            .find(|(key, _)| *key == request);
        // A prewarmed nearby thumbnail appears immediately, then refines to the exact frame.
        let texture = exact.and_then(|(_, texture)| texture.as_ref()).or_else(|| {
            self.cache
                .iter()
                .chain(self.overview.iter())
                .filter(|(_, texture)| texture.is_some())
                .min_by_key(|(key, _)| key.millis.abs_diff(request.millis))
                .and_then(|(_, texture)| texture.as_ref())
        });
        if let Some(texture) = texture {
            ui.image((texture.id(), egui::vec2(160.0, 90.0)));
        } else {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 90.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        }
        if exact.is_none() && self.requested.as_ref() != Some(&request) {
            let _ = self
                .sender
                .as_ref()
                .unwrap()
                .send(Work::Hover(request.clone()));
            self.requested = Some(request);
        }
    }
}

fn overview_requests(source: &str, duration: f64, track: usize) -> VecDeque<Request> {
    let count = ((duration / 2.0).ceil() as usize).clamp(1, OVERVIEW_LIMIT);
    // Breadth-first coverage: start, middle, quarters, eighths, ...
    let mut ranges = VecDeque::from([(0, count)]);
    let mut indices = vec![0];
    while let Some((left, right)) = ranges.pop_front() {
        if left >= right {
            continue;
        }
        let mid = (left + right) / 2;
        if mid != 0 {
            indices.push(mid);
        }
        if left < mid {
            ranges.push_back((left, mid));
        }
        if mid + 1 < right {
            ranges.push_back((mid + 1, right));
        }
    }
    indices
        .into_iter()
        .map(|index| Request {
            source: source.into(),
            track,
            millis: (duration * 1000.0 * index as f64 / count as f64).round() as u64,
        })
        .collect()
}

fn decode_worker(
    ffmpeg: &Path,
    requests: mpsc::Receiver<Work>,
    frames: mpsc::SyncSender<Frame>,
    ctx: egui::Context,
) {
    let mut overview = VecDeque::new();
    let mut hover = None;
    let mut incoming = VecDeque::new();
    let mut overview_ready = Instant::now();
    loop {
        if incoming.is_empty() && overview.is_empty() && hover.is_none() {
            match requests.recv() {
                Ok(work) => incoming.push_back(work),
                Err(_) => return,
            }
        }
        incoming.extend(requests.try_iter());
        for work in incoming.drain(..) {
            match work {
                Work::Prepare(plan) => {
                    overview = plan;
                    hover = None;
                }
                Work::Hover(request) => hover = Some(request),
            }
        }
        // Give playback CPU time between speculative decodes. Hover work wakes
        // this wait immediately and never inherits the prewarm pacing.
        if hover.is_none() && !overview.is_empty() && Instant::now() < overview_ready {
            match requests.recv_timeout(overview_ready.saturating_duration_since(Instant::now())) {
                Ok(work) => {
                    incoming.push_back(work);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        let (request, is_overview) = match hover.take() {
            Some(request) => (request, false),
            None => match overview.pop_front() {
                Some(request) => (request, true),
                None => continue,
            },
        };
        let mut cancelled = false;
        let pixels = extract_cancellable(ffmpeg, &request, || {
            incoming.extend(requests.try_iter());
            cancelled = incoming
                .iter()
                .any(|work| matches!(work, Work::Prepare(_)) || is_overview);
            cancelled
        });
        if cancelled {
            if is_overview {
                overview.push_front(request);
            }
            continue;
        }
        if frames
            .send(Frame {
                request,
                pixels,
                overview: is_overview,
            })
            .is_err()
        {
            return;
        }
        if is_overview {
            overview_ready = Instant::now() + Duration::from_millis(50);
        }
        ctx.request_repaint();
    }
}
#[cfg(test)]
fn extract(ffmpeg: &Path, request: &Request) -> Option<Vec<u8>> {
    extract_cancellable(ffmpeg, request, || false)
}

fn extract_cancellable(
    ffmpeg: &Path,
    request: &Request,
    mut cancelled: impl FnMut() -> bool,
) -> Option<Vec<u8>> {
    let mut command = Command::new(ffmpeg);
    command.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-threads", "1", "-ss"])
        .arg(format!("{:.3}", request.millis as f64 / 1000.0))
        .arg("-i").arg(&request.source).args(["-map", &format!("0:v:{}", request.track), "-frames:v", "1", "-an", "-sn", "-filter_threads", "1", "-vf", "scale=160:90:force_original_aspect_ratio=decrease,pad=160:90:(ow-iw)/2:(oh-ih)/2,setsar=1", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"])
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
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
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
    fn overview_is_bounded_and_covers_the_whole_timeline_without_duplicates() {
        for duration in [0.1, 1.0, 30.0, 7200.0] {
            let plan = overview_requests("movie.mkv", duration, 2);
            assert!(!plan.is_empty() && plan.len() <= OVERVIEW_LIMIT);
            let mut times = plan
                .iter()
                .map(|request| {
                    assert_eq!(request.track, 2);
                    request.millis
                })
                .collect::<Vec<_>>();
            times.sort_unstable();
            times.dedup();
            assert_eq!(times.len(), plan.len());
            assert_eq!(times[0], 0);
            assert!((*times.last().unwrap() as f64) < duration * 1000.0);
        }
    }

    #[test]
    fn decoder_results_are_bounded_and_stale_sources_are_discarded() {
        let ctx = egui::Context::default();
        let (sender, receiver) = mpsc::channel();
        let mut preview = Preview::default();
        preview.active = Some(("new.mkv".into(), 0));
        preview.receiver = Some(receiver);
        for (source, count) in [("old.mkv", 1), ("new.mkv", EXACT_LIMIT + 10)] {
            for index in 0..count {
                sender
                    .send(Frame {
                        request: Request {
                            source: source.into(),
                            millis: index as u64,
                            track: 0,
                        },
                        pixels: Some(vec![0; BYTES]),
                        overview: false,
                    })
                    .unwrap();
            }
        }
        preview.collect(&ctx);
        assert_eq!(preview.cache.len(), EXACT_LIMIT);
        assert!(
            preview
                .cache
                .iter()
                .all(|(request, _)| request.source == "new.mkv")
        );
    }

    #[test]
    #[ignore = "requires ffmpeg and NVPLAY_TEST_MEDIA"]
    fn real_preview_prewarming_serves_cached_hover_without_decoding() {
        let source = std::env::var("NVPLAY_TEST_MEDIA").unwrap();
        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let started = Instant::now();
        while preview.overview.len() < 4 {
            preview.prepare(&ctx, &source, 30.0, 0);
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "prewarm timeout"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let millis = preview.overview.back().unwrap().0.millis;
        assert!(preview.overview.back().unwrap().1.is_some());
        let started = Instant::now();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                preview.show(ui, &source, millis as f64 / 1000.0, 0)
            });
        });
        assert!(
            preview.requested.is_none(),
            "cached hover requested a new decode"
        );
        eprintln!("Prewarmed hover rendered in {:?}", started.elapsed());
        // An uncached time must still refine to its exact requested frame.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| preview.show(ui, &source, 3.125, 0));
        });
        let started = Instant::now();
        while !preview
            .cache
            .iter()
            .any(|(request, texture)| request.millis == 3125 && texture.is_some())
        {
            preview.collect(&ctx);
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "exact hover timeout"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("Uncached hover refined in {:?}", started.elapsed());
    }

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
