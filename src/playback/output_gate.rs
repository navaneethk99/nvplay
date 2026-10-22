//! Per-renderer gain gate for macOS outputs. These drivers control their own
//! AudioUnit/AVSampleBufferAudioRenderer, not the machine's master volume.
const REPLY_BIT: u64 = 1 << 63;
#[derive(Default)]
pub(super) struct OutputGate {
    backend: String,
    gain: Option<f64>,
    restore_gain: Option<f64>,
    applied: Option<bool>,
    unsupported: bool,
    serial: u64,
}
impl OutputGate {
    pub fn backend(&mut self, backend: &str) {
        if self.backend != backend {
            self.backend = backend.into();
            self.gain = None;
            self.restore_gain = None;
            self.reconfigured();
        }
    }
    pub fn gain(&mut self, gain: f64) {
        if gain.is_finite() {
            self.gain = Some(gain.clamp(0.0, 100.0));
        }
    }
    pub fn reconfigured(&mut self) {
        self.applied = None;
        self.unsupported = false;
    }
    pub fn request(&mut self, muted: bool) -> Option<(u64, f64)> {
        if self.unsupported
            || !matches!(self.backend.as_str(), "coreaudio" | "avfoundation")
            || self.applied == Some(muted)
        {
            return None;
        }
        self.applied = Some(muted);
        let gain = if muted {
            self.restore_gain.get_or_insert(self.gain.unwrap_or(100.0));
            0.0
        } else {
            self.restore_gain?
        };
        self.serial = self.serial.wrapping_add(1) & !REPLY_BIT;
        Some((self.serial | REPLY_BIT, gain))
    }
    pub fn replied(&mut self, id: u64, success: bool) {
        if (id & !REPLY_BIT) == self.serial && !success {
            self.unsupported = true;
        }
    }
    pub fn is_reply(id: u64) -> bool {
        id & REPLY_BIT != 0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mute_zeroes_output_and_rapid_toggles_preserve_original_renderer_gain() {
        let mut gate = OutputGate::default();
        gate.backend("coreaudio");
        gate.gain(73.0);
        assert!(gate.request(false).is_none());
        assert_eq!(gate.request(true).unwrap().1, 0.0);
        gate.gain(0.0);
        assert_eq!(gate.request(false).unwrap().1, 73.0);
        assert_eq!(gate.request(true).unwrap().1, 0.0);
        assert_eq!(gate.request(false).unwrap().1, 73.0);
        assert!(gate.request(false).is_none());
    }
    #[test]
    fn only_per_app_backends_are_gated_and_new_outputs_get_current_mute() {
        let mut gate = OutputGate::default();
        for backend in ["alsa", "pulse", "coreaudio_exclusive", "null"] {
            gate.backend(backend);
            assert!(gate.request(true).is_none());
        }
        gate.backend("avfoundation");
        let (id, gain) = gate.request(true).unwrap();
        assert_eq!(gain, 0.0);
        gate.replied(id, false);
        assert!(gate.request(false).is_none());
        gate.reconfigured();
        assert_eq!(gate.request(true).unwrap().1, 0.0);
    }
}
