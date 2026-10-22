use eframe::egui;
use std::time::Duration;

#[derive(Default)]
pub(super) struct MuteControl {
    pending: Option<Pending>,
}
struct Pending {
    id: u64,
    muted: bool,
    acknowledged: bool,
}
impl MuteControl {
    pub fn display(&self, observed: bool) -> bool {
        self.pending.as_ref().map_or(observed, |p| p.muted)
    }
    fn requested(&mut self, id: u64, muted: bool) {
        self.pending = Some(Pending {
            id,
            muted,
            acknowledged: false,
        });
    }
    fn applied(&mut self, id: u64, result: Result<(), String>) -> Result<(), String> {
        if let Some(pending) = &mut self.pending
            && pending.id == id
        {
            match result {
                Ok(()) => pending.acknowledged = true,
                Err(error) => {
                    self.pending = None;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    fn reconcile(&mut self, observed: bool) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.acknowledged && p.muted == observed)
        {
            self.pending = None;
        }
    }
}
impl super::PlayerApp {
    pub(super) fn is_muted(&self) -> bool {
        self.mute_control
            .display(self.player.as_ref().is_some_and(|p| p.state.muted))
    }
    pub(super) fn set_muted(&mut self, muted: bool) {
        if let Some(player) = &mut self.player {
            match player.set_muted(muted) {
                Ok(id) => self.mute_control.requested(id, muted),
                Err(error) => self.error = Some(error),
            }
            self.ctx.request_repaint();
        }
    }
    pub(super) fn mute_button_pressed(response: &egui::Response, ui: &egui::Ui) -> bool {
        let (pressed, released) = ui.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Primary),
            )
        });
        // Dispatch at mouse-down, suppress the later mouse-up click, and keep
        // keyboard/accessibility activation and same-frame taps working.
        (pressed && response.is_pointer_button_down_on())
            || (response.clicked() && (!released || pressed))
    }
    pub(super) fn toggle_mute(&mut self) {
        self.set_muted(!self.is_muted());
    }
    pub(super) fn mute_applied(&mut self, id: u64, result: Result<(), String>) {
        if let Err(error) = self.mute_control.applied(id, result) {
            self.error = Some(format!("Could not change mute: {error}"));
        }
    }
    pub(super) fn poll_mute(&mut self, ctx: &egui::Context) {
        self.mute_control
            .reconcile(self.player.as_ref().is_some_and(|p| p.state.muted));
        if self.mute_control.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mute_activates_on_mouse_down_once_and_does_not_wait_for_release() {
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        let mut activations = 0;
        let mut frame = |events: Vec<egui::Event>| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = ui.button("Mute");
                        rect = response.rect;
                        if super::super::PlayerApp::mute_button_pressed(&response, ui) {
                            activations += 1;
                        }
                    });
                },
            );
            (rect, activations)
        };
        let (rect, _) = frame(vec![]);
        let pos = rect.center();
        let pointer = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(
            frame(vec![egui::Event::PointerMoved(pos), pointer(true)]).1,
            1
        );
        assert_eq!(
            frame(vec![]).1,
            1,
            "holding the button must not repeat mute"
        );
        assert_eq!(
            frame(vec![pointer(false)]).1,
            1,
            "release must not toggle back"
        );
        assert_eq!(
            frame(vec![pointer(true), pointer(false)]).1,
            2,
            "a same-frame tap must still work"
        );
    }
    #[test]
    fn controls_respond_before_engine_notification_and_fast_toggles_keep_last_choice() {
        let mut mute = MuteControl::default();
        mute.requested(1, true);
        assert!(mute.display(false));
        mute.requested(2, false);
        assert!(!mute.display(true));
        mute.reconcile(false);
        assert!(
            mute.pending.is_some(),
            "an old observation cannot acknowledge the new request"
        );
        mute.applied(1, Ok(())).unwrap();
        mute.reconcile(true);
        assert!(!mute.display(true));
        mute.applied(2, Ok(())).unwrap();
        mute.reconcile(false);
        assert!(mute.pending.is_none());
        assert!(!mute.display(false));
    }
    #[test]
    fn failed_request_restores_engine_state_and_old_failures_do_not_undo_new_requests() {
        let mut mute = MuteControl::default();
        mute.requested(1, true);
        mute.requested(2, false);
        mute.applied(1, Err("old error".into())).unwrap();
        assert!(!mute.display(true));
        assert!(mute.applied(2, Err("latest error".into())).is_err());
        assert!(mute.display(true));
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;
    use crate::{app::PlayerApp, playback::Player};
    use std::time::Instant;

    fn pump(app: &mut PlayerApp, ctx: &egui::Context) {
        for event in app.player.as_mut().unwrap().poll() {
            app.handle_playback_event(event);
        }
        app.poll_mute(ctx);
        assert!(app.error.is_none(), "{:?}", app.error);
    }
    #[test]
    #[ignore = "requires libmpv and NVPLAY_TEST_MEDIA; NVPLAY_TEST_AUDIO selects audio output (use a silent fixture for device checks)"]
    fn real_mute_and_unmute_apply_promptly_and_preserve_volume() {
        let source = std::env::var_os("NVPLAY_TEST_MEDIA").expect("NVPLAY_TEST_MEDIA");
        let audio = std::env::var("NVPLAY_TEST_AUDIO").unwrap_or_else(|_| "null".into());
        let ctx = egui::Context::default();
        let player = Player::create("null", Some(&audio), Some(&ctx)).unwrap();
        let mut app = PlayerApp::from_parts(&ctx, Some(player), None, None, vec![source]);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            pump(&mut app, &ctx);
            if !app.loading && app.player.as_ref().unwrap().state.position > 0.2 {
                break;
            }
            assert!(Instant::now() < deadline, "Audio/video did not start");
            std::thread::sleep(Duration::from_millis(1));
        }
        let volume = app.player.as_ref().unwrap().state.volume;
        let mut worst = Duration::ZERO;
        for paused in [false, true] {
            app.command(&["set", "pause", if paused { "yes" } else { "no" }]);
            for target in [true, false, true, false, true, false] {
                let started = Instant::now();
                app.set_muted(target);
                assert_eq!(
                    app.is_muted(),
                    target,
                    "the UI must respond without waiting for mpv"
                );
                assert!(
                    started.elapsed() < Duration::from_millis(50),
                    "mute dispatch blocked"
                );
                loop {
                    pump(&mut app, &ctx);
                    let player = app.player.as_ref().unwrap();
                    let output_gated = if cfg!(target_os = "macos")
                        && matches!(audio.as_str(), "coreaudio" | "avfoundation")
                    {
                        player
                            .test_property("ao-volume")
                            .and_then(|gain| gain.parse::<f64>().ok())
                            .is_some_and(|gain| {
                                (gain - if target { 0.0 } else { 100.0 }).abs() < 0.1
                            })
                    } else {
                        true
                    };
                    if app.mute_control.pending.is_none()
                        && player.state.muted == target
                        && output_gated
                    {
                        break;
                    }
                    assert!(
                        started.elapsed() < Duration::from_millis(500),
                        "mute/unmute engine response exceeded 500 ms"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                worst = worst.max(started.elapsed());
                assert_eq!(app.player.as_ref().unwrap().state.volume, volume);
            }
        }
        for target in [true, false, true, false, true, false] {
            app.set_muted(target);
            assert_eq!(app.is_muted(), target);
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        while app.mute_control.pending.is_some() {
            pump(&mut app, &ctx);
            assert!(
                Instant::now() < deadline,
                "rapid mute requests failed to settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!app.player.as_ref().unwrap().state.muted);
        eprintln!(
            "{audio}: worst mute/unmute output-gain + state change {:.2} ms; immediate UI feedback; saved volume {volume}%",
            worst.as_secs_f64() * 1000.0
        );
        app.stop();
    }
}
