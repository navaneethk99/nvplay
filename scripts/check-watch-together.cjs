/* Run through check-watch-together.sh; Playwright stays in a temporary directory. */
const fs = require('node:fs');
const path = require('node:path');
const { chromium, webkit, devices } = require(process.env.NVPLAY_PLAYWRIGHT_MODULE || 'playwright');
const safari = process.env.NVPLAY_WATCH_BROWSER === 'webkit';
const mobile = process.env.NVPLAY_WATCH_MOBILE === '1';
const dir = process.env.NVPLAY_WATCH_TEST_DIR;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(check, timeout = 30000) {
  const start = Date.now();
  while (Date.now() - start < timeout) { const value = await check(); if (value) return value; await sleep(100); }
  throw Error('Timed out waiting for watch party test condition');
}
(async () => {
  const invite = await waitFor(() => fs.existsSync(path.join(dir, 'invite')) && fs.readFileSync(path.join(dir, 'invite'), 'utf8'), 240000);
  const options = { headless: true, args: ['--autoplay-policy=no-user-gesture-required'] };
  if (!safari && process.env.NVPLAY_CHROME) options.executablePath = process.env.NVPLAY_CHROME;
  const browser = await (safari ? webkit : chromium).launch(safari ? {headless: true} : options);
  try {
    const pages = [];
    async function viewerPage(index) {
      const page = await browser.newPage(mobile ? devices[safari ? 'iPhone 13' : 'Pixel 7'] : {});
      if (process.env.NVPLAY_WATCH_DEBUG_RECONNECT === '1') {
        page.on('console', message => { if (message.text().startsWith('[watch-test]')) console.log(`Viewer ${index + 1}: ${message.text()}`); });
        await page.addInitScript(() => {
          const NativeSocket = WebSocket;
          window.WebSocket = class extends NativeSocket {
            close(...args) { console.log('[watch-test] close', new Error().stack.split('\n').slice(1, 3).join(' ')); return super.close(...args); }
          };
        });
      }
      if (process.env.NVPLAY_WATCH_FORCE_RELAY === '1') {
        await page.addInitScript(({tcp}) => {
          const NativePeer = window.RTCPeerConnection;
          window.RTCPeerConnection = class extends NativePeer {
            constructor(configuration) {
              const config = {...configuration, iceTransportPolicy: 'relay'};
              config.iceServers = config.iceServers.map(server => ({...server, urls: server.urls.filter(url => url.startsWith('turn:') && url.includes(tcp ? 'transport=tcp' : 'transport=udp'))}));
              super(config);
            }
          };
        }, {tcp: index === 1});
      }
      return page;
    }
    for (let i = 0; i < Math.min(5, Number(process.env.NVPLAY_WATCH_VIEWERS || 2)); i++) {
      const page = await viewerPage(i);
      page.on('pageerror', error => console.error('Viewer error:', error.message));
      await page.goto(invite, {waitUntil: "domcontentloaded"}); await page.locator('#display-name').fill(`Test viewer ${i + 1}`); await page.locator('#join').click();
      await page.waitForFunction(() => peer?.connectionState === 'connected', {timeout: 30000});
      if (i === 0 && process.env.NVPLAY_WATCH_TEST_START_PAUSED === '1') {
        await page.waitForFunction(() => state?.paused && !$('play').disabled);
        await page.locator('#play').click();
      }
      await page.waitForFunction(() => document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames > 10, {timeout: 30000});
      pages.push(page);
    }
    await pages[0].locator('#chat-toggle').click();
    await pages[0].locator('#chat-input').fill('Hello from the browser 🎬');
    await pages[0].locator('#chat-send').click();
    await waitFor(() => {
      try {
        const state = JSON.parse(fs.readFileSync(path.join(dir, 'social-state.json'), 'utf8'));
        return state.messages.some(m => m.text === 'Hello from the browser 🎬' && m.sender.name === 'Test viewer 1') && state.notices.some(n => n.includes('Test viewer 1 joined'));
      } catch { return false; }
    });
    fs.writeFileSync(path.join(dir, 'send-chat'), 'Hello from the desktop app');
    for (const page of pages) await page.waitForFunction(() => social.messages.some(m => m.text === 'Hello from the desktop app' && m.sender.host));
    if (process.env.NVPLAY_WATCH_SCREENSHOT) await pages[0].screenshot({path: process.env.NVPLAY_WATCH_SCREENSHOT.replace('.png', '-chat.png'), fullPage: true});
    await pages[0].locator('#chat-close').click();
    console.log('Desktop host received named joins and browser chat; desktop replies reached every browser.');
    const hostState = JSON.parse(fs.readFileSync(path.join(dir, 'host-state.json'), 'utf8'));
    for (const page of pages) {
      const size = await page.evaluate(() => ({width: video.videoWidth, height: video.videoHeight}));
      if (size.width !== hostState.width || size.height !== hostState.height) throw Error(`Stream resolution ${JSON.stringify(size)} differs from host ${hostState.width}x${hostState.height}`);
      console.log(`Verified original resolution: ${size.width}x${size.height}`);
    }
    if (process.env.NVPLAY_WATCH_CHECK_PICTURE === '1') {
      for (let i = 0; i < pages.length; i++) {
        const picture = await pages[i].evaluate(() => {
          const canvas = document.createElement('canvas'); canvas.width = video.videoWidth; canvas.height = video.videoHeight;
          const context = canvas.getContext('2d'); context.drawImage(video, 0, 0);
          const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
          let sum = 0, square = 0, count = 0;
          for (let index = 0; index < pixels.length; index += 64) {
            const value = (pixels[index] + pixels[index + 1] + pixels[index + 2]) / 3;
            sum += value; square += value * value; count++;
          }
          return {mean: sum / count, deviation: Math.sqrt(square / count - (sum / count) ** 2), png: canvas.toDataURL('image/png').split(',')[1]};
        });
        if (picture.mean < 5 || picture.deviation < 10) throw Error('Decoded video is black or missing picture detail');
        fs.writeFileSync(`/tmp/nvplay-received-picture-${i}.png`, Buffer.from(picture.png, 'base64'));
        console.log(`Viewer ${i + 1} decoded a non-black, detailed video frame.`);
      }
    }
    console.log('Two viewers connected and decoded video.');
    if (process.env.NVPLAY_WATCH_SCREENSHOT) await pages[0].screenshot({path: process.env.NVPLAY_WATCH_SCREENSHOT, fullPage: true});
    for (const page of pages) {
      const stats = await page.evaluate(async () => {
        const stats = [...(peer ? (await peer.getStats()).values() : [])];
        return stats.filter(s => s.type === 'inbound-rtp').map(s => ({kind: s.kind, bytes: s.bytesReceived, frames: s.framesDecoded, lost: s.packetsLost, jitterBufferMs: s.jitterBufferEmittedCount ? Math.round(1000 * s.jitterBufferDelay / s.jitterBufferEmittedCount) : null}));
      });
      if (!stats.some(s => s.kind === 'audio' && s.bytes > 0)) throw Error('No received audio');
      console.log('Received WebRTC media:', JSON.stringify(stats));
      if (process.env.NVPLAY_WATCH_FORCE_RELAY === '1') {
        const candidate = await page.evaluate(async () => {
          const stats = await peer.getStats();
          const transport = [...stats.values()].find(s => s.type === 'transport' && s.selectedCandidatePairId);
          const pair = stats.get(transport.selectedCandidatePairId);
          const local = stats.get(pair.localCandidateId);
          const remote = stats.get(pair.remoteCandidateId);
          return {type: local.candidateType, relayProtocol: local.relayProtocol, policy: peer.getConfiguration().iceTransportPolicy, localAddress: local.address || local.ip, remoteAddress: remote.address || remote.ip, port: local.port};
        });
        // Some engines record the selected TURN allocation as peer-reflexive.
        // Verify its actual allocation address/port as well as relay-only policy.
        const reflectedRelay = candidate.type === 'prflx' && candidate.policy === 'relay' && candidate.localAddress === candidate.remoteAddress && candidate.port >= 49160 && candidate.port <= 49259;
        if (candidate.type !== 'relay' && !reflectedRelay) throw Error(`Viewer bypassed the TURN relay: ${JSON.stringify(candidate)}`);
        console.log('Verified TURN route:', JSON.stringify(candidate));
      }
    }
    if (process.env.NVPLAY_WATCH_PERF_CHECK === '1') {
      const baseline = await Promise.all(pages.map(page => page.evaluate(() => video.getVideoPlaybackQuality().totalVideoFrames)));
      const started = Date.now(); await sleep(Number(process.env.NVPLAY_WATCH_PERF_SECONDS || 4) * 1000);
      for (let i = 0; i < pages.length; i++) {
        const frames = await pages[i].evaluate(() => video.getVideoPlaybackQuality().totalVideoFrames);
        const fps = (frames - baseline[i]) * 1000 / (Date.now() - started);
        console.log(`Viewer ${i + 1} sustained decoded frame rate: ${fps.toFixed(1)} fps`);
        if (fps < 20) throw Error('1080p30 stream is not sustaining smooth playback');
      }
    }
    if (process.env.NVPLAY_WATCH_LATENCY_CHECK === '1') {
      // Hidden tabs can retain an old presented video texture while still
      // decoding. Measure the foreground player a person is actually watching.
      await pages[0].bringToFront(); await sleep(100);
      const delays = [];
      for (let sample = 0; sample < 10; sample++) {
        delays.push(await pages[0].evaluate(() => {
          const canvas = document.createElement('canvas'); canvas.width = 1920; canvas.height = 1080;
          const ctx = canvas.getContext('2d'); ctx.drawImage(video, 0, 0, 1920, 1080);
          let ticks = 0;
          for (let bit = 0; bit < 10; bit++) {
            const pixel = ctx.getImageData(bit * 40 + 20, 1050, 1, 1).data;
            if ((pixel[0] + pixel[1] + pixel[2]) / 3 > 128) ticks += 2 ** bit;
          }
          return state.position + Math.min((performance.now() - receivedAt) / 1000, 1) * state.speed - ticks / 10;
        }));
        await sleep(500);
      }
      delays.sort((a,b) => a-b);
      console.log(`Measured movie-to-browser delay: median ${(delays[5]*1000).toFixed(0)} ms, worst ${(delays[9]*1000).toFixed(0)} ms`);
      if (delays[9] > 1.5 || delays[0] < -.5) throw Error('Movie-to-browser delay exceeds the live playback budget');
    }
    await pages[0].bringToFront();
    await pages[0].locator('#fullscreen').click();
    await pages[0].waitForFunction(() => document.fullscreenElement?.id === 'player' || player.classList.contains('is-expanded'));
    for (const id of (mobile ? ['play', 'seek', 'time'] : ['play', 'seek', 'time', 'volume'])) {
      if (!await pages[0].locator(`#${id}`).isVisible()) throw Error(`Fullscreen hid ${id}`);
    }
    await pages[0].locator('#play').click();
    for (const page of pages) await page.waitForFunction(() => state?.paused === true);
    console.log('Viewer one paused both players through the host.');
    const paused = await pages[0].evaluate(() => state.position);
    await sleep(1000);
    const still = await pages[1].evaluate(() => state.position);
    if (Math.abs(still - paused) > 0.2) throw Error('Host timeline moved while paused');
    await pages[0].locator('#seek').evaluate(el => { el.value = '45'; el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true})); });
    for (const page of pages) await page.waitForFunction(() => state?.position >= 44 && state?.position <= 46);
    console.log('Fullscreen play/pause and seeking to 45 seconds verified.');
    await pages[0].locator('#fullscreen').click();
    await pages[0].waitForFunction(() => !document.fullscreenElement && !player.classList.contains('is-expanded'));
    const late = await viewerPage(2); await late.goto(invite, {waitUntil: 'domcontentloaded'}); await late.locator('#display-name').fill('Test viewer'); await late.locator('#join').click();
    await late.waitForFunction(() => state?.paused && state.position >= 44 && document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames >= 1);
    console.log('A third viewer joined while paused and decoded the still frame.');
    await late.close();
    await pages[1].bringToFront();
    await pages[1].locator('#fullscreen').click();
    await pages[1].waitForFunction(() => document.fullscreenElement?.id === 'player' || player.classList.contains('is-expanded'));
    await pages[1].keyboard.press('Space');
    for (const page of pages) await page.waitForFunction(() => state?.paused === false && state?.position > 46);
    // Network HLS inputs need to fetch/probe media again after an absolute seek.
    // Verify actual frames on both viewers rather than assuming a 1.5-second warmup.
    const baselines = await Promise.all(pages.map(page => page.evaluate(() => document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames)));
    await Promise.all(pages.map((page, index) => page.waitForFunction(before => document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames > before + 10, baselines[index], {timeout: 30000})));
    console.log('Fullscreen keyboard playback resumed both viewers after seeking.');
    await pages[1].mouse.move(100, 100);
    if (process.env.NVPLAY_WATCH_SCREENSHOT) await pages[1].screenshot({path: process.env.NVPLAY_WATCH_SCREENSHOT.replace('.png', '-fullscreen.png')});
    await pages[1].keyboard.press('m');
    if (!await pages[1].evaluate(() => userMuted)) throw Error('Local mute shortcut failed');
    await pages[1].locator('#fullscreen').click();
    await pages[1].waitForFunction(() => !document.fullscreenElement && !player.classList.contains('is-expanded'));
    await pages[1].evaluate(() => { player.requestFullscreen = undefined; });
    await pages[1].locator('#fullscreen').click();
    await pages[1].waitForFunction(() => player.classList.contains('is-expanded'));
    if (!await pages[1].locator('#seek').isVisible()) throw Error('Fallback fullscreen lost timeline');
    await pages[1].keyboard.press('Escape');
    await pages[1].waitForFunction(() => !player.classList.contains('is-expanded'));
    await pages[1].setViewportSize({width: 375, height: 812});
    if (await pages[1].evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw Error('Mobile viewer overflows horizontally');
    if (process.env.NVPLAY_WATCH_SCREENSHOT) await pages[1].screenshot({path: process.env.NVPLAY_WATCH_SCREENSHOT.replace('.png', '-mobile.png'), fullPage: true});

    // A transient connection loss must rejoin without losing the invite/controls.
    const previousId = await pages[1].evaluate(() => peerId);
    await pages[1].evaluate(() => socket.close());
    await pages[1].waitForFunction(() => !controlsReady);
    await waitFor(() => pages[1].evaluate(async previous => {
      if (!peer || peerId === previous || peer.connectionState !== 'connected') return false;
      return [...(peer ? (await peer.getStats()).values() : [])].some(s => s.type === 'inbound-rtp' && (s.kind || s.mediaType) === 'video' && s.framesDecoded > 10);
    }, previousId), 30000);
    console.log('Connection interruption recovered with fresh decoded video.');

    fs.writeFileSync(path.join(dir, 'restrict-access'), 'restrict');
    for (const page of pages) await page.waitForFunction(() => $('play').disabled && $('seek').disabled && access.allow_join === false);
    const beforeDenied = await pages[0].evaluate(() => state.position);
    await pages[0].evaluate(() => {
      send({type: 'control', control: {action: 'pause'}});
      send({type: 'control', control: {action: 'seek', position: 5}});
    });
    await sleep(1000);
    const afterDenied = await pages[1].evaluate(() => ({position: state.position, paused: state.paused}));
    if (afterDenied.paused || afterDenied.position < beforeDenied - 0.2) throw Error('Denied controls changed host playback');
    const locked = await viewerPage(3); await locked.goto(invite); await locked.locator('#display-name').fill('Test viewer'); await locked.locator('#join').click();
    await locked.waitForFunction(() => $('status').textContent.includes('locked')); await locked.close();
    fs.writeFileSync(path.join(dir, 'restore-access'), 'restore');
    for (const page of pages) await page.waitForFunction(() => !$('play').disabled && !$('seek').disabled && access.allow_join === true);
    console.log('Live viewer permissions, forged-command rejection, and locked invitations verified.');
    fs.writeFileSync(path.join(dir, 'end-session'), 'end');
    for (const page of pages) await page.waitForFunction(() => document.querySelector('#status').textContent.includes('ended'));
    const expired = await browser.newPage(); await expired.goto(invite, {waitUntil: "domcontentloaded"}); await expired.locator('#display-name').fill('Test viewer'); await expired.locator('#join').click();
    await expired.waitForFunction(() => document.querySelector('#status').textContent.includes('expired'));
    console.log('Ending the host session disconnected both viewers and revoked the link.');
    fs.writeFileSync(path.join(dir, 'done'), 'ok');
  } catch (error) {
    for (const context of browser.contexts()) for (const page of context.pages()) {
      try {
        console.error('Viewer diagnostics:', JSON.stringify(await page.evaluate(async () => ({
          status: $("status").textContent, ended, access, connection: peer?.connectionState, ice: peer?.iceConnectionState, gathering: peer?.iceGatheringState, signaling: peer?.signalingState, localCandidates: (peer?.localDescription?.sdp.match(/a=candidate:/g) || []).length, remoteCandidates: (peer?.remoteDescription?.sdp.match(/a=candidate:/g) || []).length,
          paused: state?.paused, buffering: state?.buffering, position: state?.position,
          tracks: video.srcObject?.getTracks().map(t => ({kind: t.kind, readyState: t.readyState, muted: t.muted})),
          received: [...(peer ? (await peer.getStats()).values() : [])].filter(s => s.type === 'inbound-rtp').map(s => ({kind: s.kind, bytes: s.bytesReceived, packets: s.packetsReceived, decoded: s.framesDecoded, received: s.framesReceived, lost: s.packetsLost})),
        }))));
      } catch {}
    }
    if (fs.existsSync(path.join(dir, 'host-state.json'))) console.error('Host state:', fs.readFileSync(path.join(dir, 'host-state.json'), 'utf8'));
    throw error;
  } finally { await browser.close(); }
})().catch(error => { console.error(String(error.stack || error).replace(/https?:\/\/[^\s"']*\/watch\/[^\s"']*/g, '[private viewing link]')); process.exitCode = 1; });
