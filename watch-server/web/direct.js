/* Provider playback adapters. Shared controls always round-trip through the host. */
'use strict';
let youtubeAPI, hlsAPI;
function loadScript(src) {
  return new Promise((resolve, reject) => {
    const script = document.createElement('script');
    const timer = setTimeout(() => reject(Error('Player library took too long to load.')), 15000);
    script.src = src;
    script.onload = () => { clearTimeout(timer); resolve(); };
    script.onerror = () => { clearTimeout(timer); reject(Error('Could not load the video player.')); };
    document.head.append(script);
  });
}
class BrowserPlayback {
  constructor(media, video, onReady, onError, onAutoplay, onFallback = () => {}) {
    Object.assign(this, {media, video, onReady, onError, onAutoplay, onFallback});
    this.ready = false; this.disposed = false; this.failed = false;
  }
  fail(message) {
    if (this.disposed || this.failed || this.switching) return;
    if (this.media.fallback_url && !this.usingProxy) {
      this.usingProxy = true;
      this.startFallback().catch(error => this.fail(error.message));
      return;
    }
    this.failed = true; this.pause(); this.hls?.stopLoad(); this.onError(message);
  }
  async start() {
    this.loadTimer = setTimeout(() => { if (!this.ready || (!this.youtube && !this.hasFrame)) this.fail('The video did not load. The provider may block browser access or the media link may have expired.'); }, 25000);
    try {
      if (this.media.kind === 'youtube') {
        youtubeAPI ||= new Promise((resolve, reject) => {
          window.onYouTubeIframeAPIReady = resolve;
          loadScript('https://www.youtube.com/iframe_api').catch(reject);
          setTimeout(() => { if (!window.YT?.Player) reject(Error('YouTube player did not respond.')); }, 15000);
        });
        await youtubeAPI;
        if (this.disposed) return;
        this.video.hidden = true;
        const mount = document.createElement('div'); mount.id = 'youtube-video';
        this.video.after(mount);
        this.youtube = new YT.Player(mount, {
          videoId: this.media.id,
          playerVars: {controls: 0, disablekb: 1, playsinline: 1, origin: location.origin},
          events: {
            onReady: () => { if (!this.disposed && !this.failed) { this.ready = true; this.onReady(); } },
            onAutoplayBlocked: () => { if (!this.disposed && !this.failed) this.onAutoplay(); },
            onError: () => this.fail('YouTube could not play this video here. It may be private, restricted, or unavailable for embedding.'),
          },
        });
        return;
      }
      const isHls = this.media.kind === 'hls_playlist' || /\.m3u8(?:$|[?#])/i.test(this.media.url);
      this.errorListener = () => this.fail('This source cannot play in your browser. Its URL may have expired, or the provider may require login, block cross-origin access, or use an unsupported codec.');
      this.readyListener = () => { if (!this.disposed && !this.failed) { this.ready = true; this.onReady(); } };
      this.dataListener = () => { this.hasFrame = true; };
      this.video.addEventListener('loadeddata', this.dataListener);
      this.video.addEventListener('error', this.errorListener);
      this.video.addEventListener('loadedmetadata', this.readyListener);
      let url = this.media.url;
      if (this.media.kind === 'hls_playlist') {
        this.blob = URL.createObjectURL(new Blob([this.media.playlist], {type: 'application/vnd.apple.mpegurl'}));
        url = this.blob;
      }
      if (isHls) await this.loadHls(url);
      else this.video.src = url;
    } catch (error) { this.fail(error.message); }
  }
  async loadHls(url) {
    // Prefer MSE even when a Chromium browser advertises native HLS. Native
    // support alone does not establish that it can open a blob master playlist.
    hlsAPI ||= loadScript('/hls.js');
    await hlsAPI;
    if (this.disposed) return;
    if (Hls.isSupported()) {
      const hls = new Hls({enableWorker: false});
      this.hls = hls;
      hls.on(Hls.Events.ERROR, (_, data) => {
        if (this.hls === hls && data.fatal) {
          this.lastError = {type: data.type, details: data.details, status: data.response?.code};
          this.fail(`Could not load the movie stream (${data.details || data.type}). The provider may block browser access or the media link may have expired.`);
        }
      });
      hls.loadSource(url); hls.attachMedia(this.video);
    } else if (this.video.canPlayType('application/vnd.apple.mpegurl')) this.video.src = url;
    else throw Error('This browser does not support HLS playback.');
  }
  async startFallback() {
    this.switching = true; this.ready = false; this.hasFrame = false;
    clearTimeout(this.loadTimer);
    this.hls?.destroy(); this.hls = undefined;
    this.video.removeAttribute('src'); this.video.load();
    this.onFallback();
    this.switching = false;
    this.loadTimer = setTimeout(() => { if (!this.hasFrame) this.fail('The movie could not load through the watch service. Try a fresh movie source or stream from the host device.'); }, 30000);
    await this.loadHls(this.media.fallback_url);
  }
  pause() {
    if (this.youtube) { if (this.ready) this.youtube.pauseVideo(); }
    else this.video.pause();
  }
  resume() {
    if (!this.ready || this.failed || this.disposed) return;
    if (this.youtube) this.youtube.playVideo();
    else this.video.play().catch(() => { if (!this.disposed && !this.failed) this.onAutoplay(); });
  }
  volume(volume, muted) {
    if (this.youtube && this.ready) { this.youtube.setVolume(volume * 100); muted ? this.youtube.mute() : this.youtube.unMute(); }
  }
  sync(state, age, latency = 0) {
    if (!this.ready || this.failed || this.disposed || !state) return;
    const paused = state.paused || state.buffering || age > 1.5;
    const target = Math.min(state.duration || Infinity, state.position + (paused ? 0 : (age + latency) * state.speed));
    const position = this.youtube ? this.youtube.getCurrentTime() : this.video.currentTime;
    const drift = target - position;
    if (Math.abs(drift) > (paused ? .15 : 1.5)) {
      if (this.youtube) this.youtube.seekTo(target, true);
      else if (!this.video.seeking) this.video.currentTime = target;
    }
    if (this.youtube) {
      const rates = this.youtube.getAvailablePlaybackRates();
      const rate = rates.reduce((a, b) => Math.abs(a - state.speed) < Math.abs(b - state.speed) ? a : b, 1);
      if (this.youtube.getPlaybackRate() !== rate) this.youtube.setPlaybackRate(rate);
      const playing = this.youtube.getPlayerState() === 1;
      if (paused) this.youtube.pauseVideo();
      else if (!playing && this.youtube.getPlayerState() !== 3) this.resume();
    } else {
      // Small corrections avoid a visible seek for ordinary clock drift.
      this.video.playbackRate = Math.max(.25, Math.min(4, state.speed * (!paused && Math.abs(drift) > .15 ? (drift > 0 ? 1.05 : .95) : 1)));
      if (paused) this.video.pause();
      else if (this.video.paused) this.resume();
    }
  }
  destroy() {
    this.disposed = true; clearTimeout(this.loadTimer); this.pause();
    this.youtube?.destroy(); this.hls?.destroy();
    if (this.dataListener) this.video.removeEventListener('loadeddata', this.dataListener);
    if (this.errorListener) this.video.removeEventListener('error', this.errorListener);
    if (this.readyListener) this.video.removeEventListener('loadedmetadata', this.readyListener);
    this.video.removeAttribute('src'); this.video.load(); this.video.hidden = false;
    if (this.blob) URL.revokeObjectURL(this.blob);
  }
}
