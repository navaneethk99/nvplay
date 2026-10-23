/* Control-only browser integration. Run after cargo build -p nvplay-watch-server.
 * NVPLAY_PLAYWRIGHT_MODULE may point to an existing Playwright installation. */
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const {spawn, execFileSync} = require('node:child_process');
const {chromium} = require(process.env.NVPLAY_PLAYWRIGHT_MODULE || 'playwright');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
(async () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'nvplay-direct-'));
  let browser, service;
  const sockets = [], requests = [], timers = [];
  const files = http.createServer((req, res) => {
    requests.push(req.url);
    const file = path.join(temp, path.basename(req.url.split('?')[0]));
    if (!fs.existsSync(file)) { res.writeHead(404, {'Access-Control-Allow-Origin': '*'}); res.end(); return; }
    const stat = fs.statSync(file), range = req.headers.range?.match(/bytes=(\d+)-(\d*)/);
    const start = range ? Number(range[1]) : 0, end = range?.[2] ? Math.min(Number(range[2]), stat.size - 1) : stat.size - 1;
    const type = file.endsWith('.m3u8') ? 'application/vnd.apple.mpegurl' : file.endsWith('.ts') ? 'video/mp2t' : 'video/mp4';
    res.writeHead(range ? 206 : 200, {'Content-Type': type, 'Access-Control-Allow-Origin': '*', 'Accept-Ranges': 'bytes', 'Content-Length': end - start + 1, ...(range ? {'Content-Range': `bytes ${start}-${end}/${stat.size}`} : {})});
    fs.createReadStream(file, {start, end}).pipe(res);
  });
  try {
    execFileSync(process.env.NVPLAY_FFMPEG || 'ffmpeg', ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24', '-f', 'lavfi', '-i', 'sine=frequency=440', '-t', '60', '-c:v', 'libx264', '-preset', 'ultrafast', '-g', '48', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-movflags', '+faststart', path.join(temp, 'movie.mp4')]);
    execFileSync(process.env.NVPLAY_FFMPEG || 'ffmpeg', ['-hide_banner', '-loglevel', 'error', '-i', path.join(temp, 'movie.mp4'), '-c', 'copy', '-hls_time', '2', '-hls_list_size', '0', path.join(temp, 'movie.m3u8')]);
    await new Promise(resolve => files.listen(0, '127.0.0.1', resolve));
    const mediaOrigin = `http://127.0.0.1:${files.address().port}`;
    const port = Number(process.env.NVPLAY_DIRECT_TEST_PORT || 18787), origin = `http://localhost:${port}`;
    service = spawn(path.resolve('target/debug/nvplay-watch-server'), {env: {...process.env, NVPLAY_WATCH_BIND: `127.0.0.1:${port}`, NVPLAY_WATCH_PUBLIC_URL: origin, NVPLAY_WATCH_HOST_KEY: '', NVPLAY_TURN_URLS: '', NVPLAY_TURN_SECRET: ''}, stdio: ['ignore', 'ignore', 'pipe']});
    let logs = ''; service.stderr.on('data', chunk => { logs += chunk; });
    for (let i = 0; ; i++) {
      try { if ((await fetch(`${origin}/health`)).ok) break; } catch {}
      if (i > 100 || service.exitCode !== null) throw Error(`Watch server did not start: ${logs}`);
      await delay(100);
    }
    browser = await chromium.launch({headless: true, ...(process.env.NVPLAY_CHROME ? {executablePath: process.env.NVPLAY_CHROME} : {}), args: ['--autoplay-policy=no-user-gesture-required']});
    async function room(media) {
      const socket = new WebSocket(`${origin.replace('http', 'ws')}/ws`); sockets.push(socket);
      const session = {state: {position: 10, duration: 60, paused: true, buffering: false, speed: 1, title: 'Direct playback test'}, at: Date.now(), controls: []};
      const created = new Promise((resolve, reject) => {
        socket.onopen = () => socket.send(JSON.stringify({type: 'create', key: '', media}));
        socket.onerror = reject;
        socket.onmessage = event => {
          const message = JSON.parse(event.data);
          if (message.type === 'created') resolve(message.url);
          if (message.type === 'control') {
            session.controls.push(message.control);
            const next = {...current()};
            if (message.control.action === 'seek') next.position = message.control.position;
            else next.paused = message.control.action === 'pause';
            session.update(next);
          }
          assert(!['offer', 'answer', 'candidate', 'uplink'].includes(message.type), 'Direct session used WebRTC');
        };
      });
      function current() { return {...session.state, position: session.state.position + (!session.state.paused && !session.state.buffering ? (Date.now() - session.at) / 1000 * session.state.speed : 0)}; }
      session.update = next => { session.state = {...session.state, ...next}; session.at = Date.now(); session.send(); };
      session.send = () => { if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({type: 'state', state: current()})); };
      session.url = await created; session.send();
      session.timer = setInterval(session.send, 250); timers.push(session.timer);
      session.stop = () => { clearInterval(session.timer); socket.send(JSON.stringify({type: 'ended'})); };
      session.access = access => socket.send(JSON.stringify({type: 'access', access}));
      return session;
    }
    async function join(session, setup) {
      const page = await browser.newPage();
      page.on('pageerror', error => { throw error; });
      if (setup) await setup(page);
      await page.goto(session.url); await page.locator('#join').click();
      return page;
    }
    const session = await room({kind: 'url', url: `${mediaOrigin}/movie.mp4`});
    const a = await join(session), b = await join(session);
    for (const page of [a, b]) {
      await page.waitForFunction(() => controlsReady && video.readyState >= 2 && Math.abs(video.currentTime - 10) < .25);
      assert.equal(await page.evaluate(() => peer), null);
    }
    await a.locator('#play').click();
    await Promise.all([a, b].map(page => page.waitForFunction(() => !state.paused && !video.paused && video.currentTime > 11)));
    session.update({...session.state, paused: true, position: 35});
    await Promise.all([a, b].map(page => page.waitForFunction(() => video.paused && Math.abs(video.currentTime - 35) < .25)));
    const late = await join(session);
    await late.waitForFunction(() => controlsReady && video.paused && Math.abs(video.currentTime - 35) < .25);
    await a.locator('#seek').evaluate(el => { el.value = '20'; el.dispatchEvent(new Event('change')); });
    await b.waitForFunction(() => Math.abs(video.currentTime - 20) < .25);
    session.update({paused: false, position: 20, speed: 1.5});
    await b.waitForFunction(() => !video.paused && video.playbackRate > 1.4);
    session.update({paused: true, position: 25, speed: 1});
    await b.waitForFunction(() => video.paused && Math.abs(video.currentTime - 25) < .25);
    await b.evaluate(() => video.currentTime = 3);
    await b.waitForFunction(() => Math.abs(video.currentTime - 25) < .25);
    const previous = await b.evaluate(() => peerId);
    await b.evaluate(() => socket.close());
    await b.waitForFunction(id => controlsReady && peerId !== id && Math.abs(video.currentTime - 25) < .25, previous);
    session.access({play_pause: false, seek: false, allow_join: false});
    await a.waitForFunction(() => $('play').disabled && $('seek').disabled);
    await a.evaluate(() => send({type: 'control', control: {action: 'seek', position: 1}}));
    await a.waitForFunction(() => $('status').textContent.includes('disabled'));
    assert.equal(session.state.position, 25);
    session.stop();
    await b.waitForFunction(() => ended && video.paused && !video.hasAttribute('src'));
    console.log('MP4: two viewers, late join, host and viewer controls, speed, drift correction, reconnect, permissions, and cleanup passed.');
    const playlist = `#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\n${mediaOrigin}/movie.m3u8\n`;
    const hls = await room({kind: 'hls_playlist', playlist});
    const h = await join(hls, page => page.addInitScript(() => {
      const original = HTMLMediaElement.prototype.canPlayType;
      HTMLMediaElement.prototype.canPlayType = function(type) {
        return type === 'application/vnd.apple.mpegurl' ? 'probably' : original.call(this, type);
      };
    }));
    await h.waitForFunction(() => controlsReady && video.readyState >= 2 && Math.abs(video.currentTime - 10) < .25);
    await h.locator('#play').click();
    await h.waitForFunction(() => video.getVideoPlaybackQuality().totalVideoFrames > 10 && !video.paused);
    assert(requests.some(url => url.endsWith('.ts')));
    assert(await h.evaluate(() => !!direct.hls), 'MSE should take priority over advertised native HLS');
    hls.stop();
    console.log('HLS: separate-origin decode passed even when native HLS is advertised.');
    const fallback = await room({kind: 'hls_playlist', playlist});
    const f = await join(fallback, async page => {
      await page.route(`${mediaOrigin}/**`, route => route.fulfill({status: 403, body: 'Provider requires headers'}));
      await page.route('**/media/**', async route => {
        const url = new URL(route.request().url());
        const file = url.pathname.endsWith('/master') ? 'movie.m3u8' : path.basename(url.pathname);
        if (file === 'movie.m3u8') {
          const text = fs.readFileSync(path.join(temp, file), 'utf8').replace(/^(movie\d+\.ts)$/gm, `/media/mock/$1`);
          await route.fulfill({contentType: 'application/vnd.apple.mpegurl', body: text});
        } else await route.fulfill({contentType: 'video/mp2t', body: fs.readFileSync(path.join(temp, file))});
      });
    });
    await f.waitForFunction(() => direct?.usingProxy && direct.ready && video.readyState >= 2, null, {timeout: 30000});
    await f.locator('#play').click();
    await f.waitForFunction(() => video.getVideoPlaybackQuality().totalVideoFrames > 10 && !video.paused);
    fallback.stop();
    console.log('Blocked provider: automatic original-media fallback decoded without WebRTC.');
    const youtube = await room({kind: 'youtube', id: 'dQw4w9WgXcQ'});
    const y = await join(youtube, page => page.route('https://www.youtube.com/iframe_api', route => route.fulfill({contentType: 'text/javascript', body: `window.YT={Player:class { constructor(el,options){this.position=0;this.rate=1;this.state=2;setTimeout(()=>options.events.onReady(),0);}getCurrentTime(){return this.position;}seekTo(p){this.position=p;}getAvailablePlaybackRates(){return [1,1.5,2];}getPlaybackRate(){return this.rate;}setPlaybackRate(r){this.rate=r;}getPlayerState(){return this.state;}playVideo(){this.state=1;}pauseVideo(){this.state=2;}setVolume(){}mute(){}unMute(){}destroy(){} }};window.onYouTubeIframeAPIReady();`})));
    await y.waitForFunction(() => controlsReady && direct.youtube.getCurrentTime() === 10 && direct.youtube.getPlayerState() === 2);
    await y.locator('#play').click();
    await y.waitForFunction(() => direct.youtube.getPlayerState() === 1);
    youtube.stop();
    console.log('YouTube adapter contract: provider API setup and host-authoritative synchronization passed (mock API).');
    if (process.env.NVPLAY_DIRECT_CINEJOY_METADATA) {
      const metadata = JSON.parse(fs.readFileSync(process.env.NVPLAY_DIRECT_CINEJOY_METADATA, 'utf8'));
      const live = await room({kind: 'hls_playlist', playlist: metadata.playlist});
      const livePage = await join(live, process.env.NVPLAY_DIRECT_FORCE_PROXY === '1'
        ? page => page.route('https://**/*', route => route.fulfill({status: 403, body: 'Direct access intentionally blocked by regression test'}))
        : undefined);
      await livePage.waitForFunction(() => direct?.ready || direct?.failed, null, {timeout: 30000});
      assert.equal(await livePage.evaluate(() => direct.failed), false, await livePage.locator('#status').textContent());
      await livePage.locator('#play').click();
      await livePage.waitForFunction(() => video.getVideoPlaybackQuality().totalVideoFrames > 10 && !video.paused, null, {timeout: 30000}).catch(async error => {
        const details = await livePage.evaluate(() => ({status: $('status').textContent, ready: direct?.ready, failed: direct?.failed, hostPaused: state?.paused, paused: video.paused, position: video.currentTime, readyState: video.readyState, frames: video.getVideoPlaybackQuality().totalVideoFrames, mediaError: video.error?.code}));
        throw Error(`Live provider playback did not advance: ${JSON.stringify(details)}`);
      });
      const delivery = await livePage.evaluate(() => direct.usingProxy ? 'service fallback' : 'direct access');
      live.stop();
      console.log(`Live Cinejoy: browser decoded resolved playlist via ${delivery}.`);
    }
    if (process.env.NVPLAY_DIRECT_LIVE_YOUTUBE) {
      const live = await room({kind: 'youtube', id: process.env.NVPLAY_DIRECT_LIVE_YOUTUBE});
      const livePage = await join(live);
      await livePage.waitForFunction(() => direct?.ready || direct?.failed, null, {timeout: 30000});
      assert.equal(await livePage.evaluate(() => direct.failed), false, await livePage.locator('#status').textContent());
      await livePage.locator('#play').click();
      await livePage.waitForFunction(() => direct.youtube.getPlayerState() === 1, null, {timeout: 30000});
      live.stop();
      console.log('Live YouTube: embedded provider player started playback.');
    }
    const bad = await room({kind: 'url', url: `${mediaOrigin}/missing.mp4`});
    const errorPage = await join(bad);
    await errorPage.waitForFunction(() => direct?.failed && !controlsReady && $('overlay-text').textContent.includes('cannot play'));
    bad.stop();
    console.log('Unavailable source: actionable playback error shown without a reconnect loop.');
  } catch (error) {
    if (browser) for (const context of browser.contexts()) for (const page of context.pages()) {
      console.error('Playback diagnostics:', await page.evaluate(() => ({status: $('status')?.textContent, mode: direct?.media.kind, failed: direct?.failed, proxy: direct?.usingProxy, mse: !!direct?.hls, ready: direct?.ready, videoState: video.readyState, error: video.error?.code, position: video.currentTime, hostPosition: state?.position, lastError: direct?.lastError})).catch(() => ({})));
    }
    throw error;
  } finally {
    for (const timer of timers) clearInterval(timer);
    for (const socket of sockets) socket.close();
    if (browser) await browser.close();
    service?.kill(); files.close();
    fs.rmSync(temp, {recursive: true, force: true});
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
