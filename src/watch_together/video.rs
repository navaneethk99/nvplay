//! Keep source decoding alive while the H.264 bitrate changes. YUV4MPEG2 carries
//! original dimensions/frame rate; a restarted encoder gets a fresh header and
//! the next complete raw frame, without reopening or seeking the movie source.
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::watch,
    task::JoinHandle,
};

struct Process {
    child: Child,
    input: Option<ChildStdin>,
    errors: JoinHandle<Vec<u8>>,
    rate: u64,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        self.errors.abort();
    }
}
impl Process {
    async fn start(
        path: &PathBuf,
        hardware: bool,
        rate: u64,
        port: u16,
        header: &[u8],
    ) -> Result<Self, String> {
        let mut command = tokio::process::Command::from(crate::streaming::tool_command(path));
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-probesize",
                "32",
                "-analyzeduration",
                "0",
                "-f",
                "yuv4mpegpipe",
                "-i",
                "pipe:0",
                "-an",
            ]);
        if hardware {
            command.args([
                "-c:v",
                "h264_videotoolbox",
                "-realtime",
                "1",
                "-allow_sw",
                "0",
                "-b:v",
                &rate.to_string(),
            ]);
        } else {
            command.args([
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-tune",
                "zerolatency",
                "-crf",
                "18",
            ]);
        }
        command.args([
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-bf",
            "0",
            "-maxrate",
            &rate.to_string(),
            "-bufsize",
            &(rate / 2).to_string(),
            "-flush_packets",
            "1",
            "-force_key_frames",
            "expr:gte(t,n_forced*1)",
            "-sc_threshold",
            "0",
            "-f",
            "rtp",
            "-payload_type",
            "102",
            &format!("rtp://127.0.0.1:{port}?pkt_size=1200"),
        ]);
        let mut child = command
            .spawn()
            .map_err(|_| "Could not start the video encoder")?;
        let mut input = child.stdin.take().expect("piped video input");
        let mut stderr = child.stderr.take().expect("piped video errors");
        let errors = tokio::spawn(async move {
            let mut tail = Vec::new();
            let mut buffer = [0; 1024];
            while let Ok(n) = stderr.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
                tail.extend_from_slice(&buffer[..n]);
                if tail.len() > 8192 {
                    tail.drain(..tail.len() - 8192);
                }
            }
            tail
        });
        input
            .write_all(header)
            .await
            .map_err(|_| "Could not initialize video encoding")?;
        Ok(Self {
            child,
            input: Some(input),
            errors,
            rate,
        })
    }
    async fn finish(&mut self) -> Result<(), String> {
        drop(self.input.take());
        let status = tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .map_err(|_| "The video encoder did not finish")?
            .map_err(|_| "Could not read video encoder status")?;
        if status.success() {
            return Ok(());
        }
        // Encoder diagnostics contain only the local pipe/RTP destination.
        let stderr = (&mut self.errors).await.unwrap_or_default();
        Err(format!(
            "Video encoding failed: {}",
            String::from_utf8_lossy(&stderr)
        ))
    }
}

fn frame_size(header: &[u8]) -> Result<usize, String> {
    let header = std::str::from_utf8(header).map_err(|_| "Invalid decoded video header")?;
    if !header.starts_with("YUV4MPEG2 ")
        || !header.split_whitespace().any(|s| s.starts_with("C420"))
    {
        return Err("The decoder did not produce 4:2:0 video".into());
    }
    let value = |prefix| {
        header
            .split_whitespace()
            .find_map(|s| s.strip_prefix(prefix).and_then(|v| v.parse::<usize>().ok()))
    };
    let (Some(width), Some(height)) = (value('W'), value('H')) else {
        return Err("Missing decoded video dimensions".into());
    };
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .map(|n| n / 2)
        .filter(|n| *n > 0 && *n <= 128 * 1024 * 1024 && width % 2 == 0 && height % 2 == 0)
        .ok_or_else(|| "Decoded video dimensions are not supported".into())
}

pub fn start(
    path: PathBuf,
    source: ChildStdout,
    hardware: bool,
    rate: watch::Receiver<u64>,
    port: u16,
) -> JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        let mut source = BufReader::new(source);
        let mut header = Vec::new();
        (&mut source)
            .take(4096)
            .read_until(b'\n', &mut header)
            .await
            .map_err(|_| "Could not read decoded video")?;
        let size = frame_size(&header)?;
        let mut frame = vec![0; size];
        let mut marker = Vec::new();
        let mut process: Option<Process> = None;
        loop {
            marker.clear();
            let n = (&mut source)
                .take(4096)
                .read_until(b'\n', &mut marker)
                .await
                .map_err(|_| "Could not read decoded frame")?;
            if n == 0 {
                break;
            }
            if !(marker == b"FRAME\n" || marker.starts_with(b"FRAME ")) || !marker.ends_with(b"\n")
            {
                return Err("Invalid decoded frame boundary".into());
            }
            source
                .read_exact(&mut frame)
                .await
                .map_err(|_| "The decoder stopped during a video frame")?;
            let next_rate = *rate.borrow();
            if process.as_ref().is_none_or(|p| p.rate != next_rate) {
                if let Some(mut previous) = process.take() {
                    let _ = previous.child.start_kill();
                    let _ =
                        tokio::time::timeout(Duration::from_secs(1), previous.child.wait()).await;
                }
                process = Some(Process::start(&path, hardware, next_rate, port, &header).await?);
            }
            let process = process.as_mut().unwrap();
            if process
                .input
                .as_mut()
                .unwrap()
                .write_all(&marker)
                .await
                .is_err()
                || process
                    .input
                    .as_mut()
                    .unwrap()
                    .write_all(&frame)
                    .await
                    .is_err()
            {
                return process
                    .finish()
                    .await
                    .and(Err("The video encoder stopped accepting frames".into()));
            }
        }
        if let Some(process) = &mut process {
            process.finish().await?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoded_frame_dimensions_are_bounded_and_keep_original_resolution() {
        assert_eq!(
            frame_size(b"YUV4MPEG2 W1920 H1080 F25:1 Ip A1:1 C420jpeg\n").unwrap(),
            1920 * 1080 * 3 / 2
        );
        assert!(frame_size(b"YUV4MPEG2 W999999999999 H999999999999 C420\n").is_err());
        assert!(frame_size(b"YUV4MPEG2 W1919 H1080 C420\n").is_err());
    }
}
