//! Updates for locally compiled macOS installs. No release feed is configured yet.
use crate::source_version;
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

const SOURCE: &str = env!("CARGO_MANIFEST_DIR");
const VERSION: &str = env!("NVPLAY_SOURCE_VERSION");

enum Status {
    Idle,
    Checking,
    Current,
    Available,
    Installing,
    Installed,
    Error(String),
}
enum ResultMessage {
    Checked(Result<bool, String>),
    Installed(Result<(), String>),
}
pub(super) struct Updates {
    pub(super) open: bool,
    pub(super) automatic: bool,
    status: Status,
    receiver: Option<mpsc::Receiver<ResultMessage>>,
    next_check: Instant,
    config_error: Option<String>,
}
impl Default for Updates {
    fn default() -> Self {
        Self {
            open: false,
            automatic: config_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .is_some_and(|s| s.trim() == "automatic=true"),
            status: Status::Idle,
            receiver: None,
            next_check: Instant::now() + Duration::from_secs(3),
            config_error: None,
        }
    }
}
fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/nvplay/updates.conf"))
}
fn destination() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(bundle) = exe.parent().and_then(Path::parent).and_then(Path::parent)
        && bundle.extension().is_some_and(|ext| ext == "app")
    {
        return Ok(bundle.to_owned());
    }
    let installed = PathBuf::from("/Applications/nvplay.app");
    if installed.join("Contents/MacOS/nvplay").is_file() {
        Ok(installed)
    } else {
        Err("Install nvplay in Applications before updating it.".into())
    }
}
fn run_script(name: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let root = Path::new(SOURCE);
    if !root.join("Cargo.toml").is_file() {
        return Err(format!("The update source folder is unavailable: {SOURCE}"));
    }
    let home_bin = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cargo/bin"))
        .unwrap_or_default();
    let mut paths = vec![
        home_bin,
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let output = Command::new("/bin/sh")
        .arg(root.join("scripts").join(name))
        .args(args)
        .current_dir(root)
        .env(
            "PATH",
            std::env::join_paths(paths).map_err(|e| e.to_string())?,
        )
        .output()
        .map_err(|e| format!("Could not start update: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let details = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let tail: String = details
            .chars()
            .rev()
            .take(5000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        Err(format!(
            "Update failed. The installed app has been preserved.\n{tail}"
        ))
    }
}
fn install() -> Result<(), String> {
    let destination = destination()?;
    let before = source_version::fingerprint(Path::new(SOURCE)).map_err(|e| e.to_string())?;
    run_script("bundle-macos.sh", &[std::ffi::OsStr::new("release")])?;
    let after = source_version::fingerprint(Path::new(SOURCE)).map_err(|e| e.to_string())?;
    if before != after {
        return Err("Source files changed during the build. Check for updates again.".into());
    }
    run_script(
        "install-macos.sh",
        &[
            Path::new(SOURCE).join("target/nvplay.app").as_os_str(),
            destination.as_os_str(),
        ],
    )
}
impl Updates {
    pub(super) fn check(&mut self, ctx: &egui::Context) {
        self.open = true;
        if self.receiver.is_some() || matches!(self.status, Status::Installed) {
            return;
        }
        self.status = Status::Checking;
        self.spawn(ctx, || {
            ResultMessage::Checked(
                source_version::fingerprint(Path::new(SOURCE))
                    .map(|v| v != VERSION)
                    .map_err(|e| format!("Cannot check the local update source: {e}")),
            )
        });
    }
    pub(super) fn toggle_automatic(&mut self) {
        self.automatic = !self.automatic;
        self.next_check = Instant::now();
        let saved = (|| -> Result<(), String> {
            let path = config_path().ok_or("Cannot locate your preferences folder")?;
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(path, format!("automatic={}\n", self.automatic))
                .map_err(|e| e.to_string())
        })();
        self.config_error = saved.err();
        if self.config_error.is_some() {
            self.open = true;
        }
    }
    fn spawn(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce() -> ResultMessage + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            ctx.request_repaint();
        });
    }
    fn start_install(&mut self, ctx: &egui::Context) {
        self.status = Status::Installing;
        self.spawn(ctx, || ResultMessage::Installed(install()));
    }
    pub(super) fn ui(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(message) => {
                    self.receiver = None;
                    self.status = match message {
                        ResultMessage::Checked(Ok(true)) => Status::Available,
                        ResultMessage::Checked(Ok(false)) => Status::Current,
                        ResultMessage::Checked(Err(error))
                        | ResultMessage::Installed(Err(error)) => {
                            self.open = true;
                            Status::Error(error)
                        }
                        ResultMessage::Installed(Ok(())) => {
                            self.open = true;
                            Status::Installed
                        }
                    };
                    self.next_check = Instant::now() + Duration::from_secs(60);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.status = Status::Error(
                        "The update task stopped unexpectedly. Try checking again.".into(),
                    );
                    self.open = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.automatic && self.receiver.is_none() {
            if matches!(self.status, Status::Available) {
                self.start_install(ctx);
            } else if Instant::now() >= self.next_check
                && matches!(self.status, Status::Idle | Status::Current)
            {
                let was_open = self.open;
                self.check(ctx);
                self.open = was_open;
            }
        }
        if self.automatic {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("nvplay Updates").open(&mut open).collapsible(false).default_width(440.0).show(ctx, |ui| {
            ui.label(format!("nvplay {}", env!("CARGO_PKG_VERSION")));
            ui.label("Updates are built from your local nvplay source folder.");
            ui.label(egui::RichText::new(SOURCE).small());
            ui.separator();
            match &self.status {
                Status::Idle => { ui.label("Check whether a newer local build is available."); }
                Status::Checking => { ui.spinner(); ui.label("Checking for updates…"); }
                Status::Current => { ui.label("You’re up to date with the local source."); }
                Status::Available => { ui.label("Source changes are available."); if ui.button("Build and Install Update").clicked() { self.start_install(ctx); } }
                Status::Installing => { ui.spinner(); ui.label("Building and installing… You can keep using the player."); }
                Status::Installed => {
                    ui.label("Update installed. Restart nvplay to use the new version.");
                    if ui.button("Restart Now").clicked() {
                        match restart() {
                            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(error) => self.config_error = Some(error),
                        }
                    }
                }
                Status::Error(error) => { egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| { ui.label(error); }); }
            }
            if self.receiver.is_none() && !matches!(self.status, Status::Installed) && ui.button("Check for Updates").clicked() { self.check(ctx); }
            ui.separator();
            let mut automatic = self.automatic;
            if ui.checkbox(&mut automatic, "Automatically check and install updates").changed() { self.toggle_automatic(); }
            ui.small("Automatic updates rebuild changed source in the background. Restart when convenient; playback is not interrupted. Rust and this source folder are required.");
            if let Some(error) = &self.config_error { ui.colored_label(egui::Color32::RED, error); }
        });
        self.open = open;
    }
}
fn restart() -> Result<(), String> {
    let bundle = destination()?;
    // Pass paths as positional arguments, never interpolate them into shell code.
    Command::new("/bin/sh")
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open -n \"$2\"",
            "nvplay-restart",
        ])
        .arg(std::process::id().to_string())
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not restart nvplay: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_result(message: ResultMessage) -> Updates {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(message)
            .unwrap_or_else(|_| panic!("queue test result"));
        Updates {
            open: false,
            automatic: false,
            status: Status::Checking,
            receiver: Some(receiver),
            next_check: Instant::now(),
            config_error: None,
        }
    }
    fn draw(updates: &mut Updates) {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| updates.ui(ctx));
    }
    #[test]
    fn checking_offers_install_without_starting_it_automatically() {
        let mut updates = with_result(ResultMessage::Checked(Ok(true)));
        draw(&mut updates);
        assert!(matches!(updates.status, Status::Available));
        assert!(updates.receiver.is_none());
        let mut updates = with_result(ResultMessage::Checked(Ok(false)));
        draw(&mut updates);
        assert!(matches!(updates.status, Status::Current));
    }
    #[test]
    fn failed_install_is_visible_and_can_be_retried() {
        let mut updates = with_result(ResultMessage::Installed(Err("build failed".into())));
        draw(&mut updates);
        assert!(matches!(updates.status, Status::Error(_)));
        assert!(updates.open);
        assert!(updates.receiver.is_none());
    }
    #[test]
    fn installed_update_waits_for_restart_even_with_automatic_mode() {
        let mut updates = with_result(ResultMessage::Installed(Ok(())));
        updates.automatic = true;
        draw(&mut updates);
        updates.next_check = Instant::now();
        draw(&mut updates);
        assert!(matches!(updates.status, Status::Installed));
        assert!(updates.open);
        assert!(updates.receiver.is_none());
    }
}
