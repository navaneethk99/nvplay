/* Run through check-watch-together.sh; Playwright stays in a temporary directory. */
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.NVPLAY_PLAYWRIGHT_MODULE || 'playwright');
const dir = process.env.NVPLAY_WATCH_TEST_DIR;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(check, timeout = 30000) {
  const start = Date.now();
  while (Date.now() - start < timeout) { const value = await check(); if (value) return value; await sleep(100); }
  throw Error('Timed out waiting for watch party test condition');
}
(async () => {
  const invite = await waitFor(() => fs.existsSync(path.join(dir, 'invite')) && fs.readFileSync(path.join(dir, 'invite'), 'utf8'), 60000);
  const options = { headless: true, args: ['--autoplay-policy=no-user-gesture-required'] };
  if (process.env.NVPLAY_CHROME) options.executablePath = process.env.NVPLAY_CHROME;
  const browser = await chromium.launch(options);
  try {
    const pages = [];
    async function viewerPage(index) {
      const page = await browser.newPage();
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
    for (let i = 0; i < 2; i++) {
      const page = await viewerPage(i);
      page.on('pageerror', error => console.error('Viewer error:', error.message));
      await page.goto(invite, {waitUntil: "domcontentloaded"}); await page.locator('#join').click();
      await page.waitForFunction(() => peer?.connectionState === 'connected', {timeout: 30000});
      await page.waitForFunction(() => document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames > 10, {timeout: 30000});
      pages.push(page);
    }
    console.log('Two viewers connected and decoded video.');
    if (process.env.NVPLAY_WATCH_SCREENSHOT) await pages[0].screenshot({path: process.env.NVPLAY_WATCH_SCREENSHOT, fullPage: true});
    for (const page of pages) {
      const stats = await page.evaluate(async () => {
        const stats = [...(await peer.getStats()).values()];
        return stats.filter(s => s.type === 'inbound-rtp').map(s => ({kind: s.kind, bytes: s.bytesReceived, frames: s.framesDecoded}));
      });
      if (!stats.some(s => s.kind === 'audio' && s.bytes > 0)) throw Error('No received audio');
      console.log('Received WebRTC media:', JSON.stringify(stats));
      if (process.env.NVPLAY_WATCH_FORCE_RELAY === '1') {
        const candidate = await page.evaluate(async () => {
          const stats = await peer.getStats();
          const transport = [...stats.values()].find(s => s.type === 'transport' && s.selectedCandidatePairId);
          const pair = stats.get(transport.selectedCandidatePairId);
          const local = stats.get(pair.localCandidateId);
          return {type: local.candidateType, relayProtocol: local.relayProtocol};
        });
        if (candidate.type !== 'relay') throw Error('Viewer bypassed the TURN relay');
        console.log('Verified TURN route:', JSON.stringify(candidate));
      }
    }
    await pages[0].locator('#play').click();
    for (const page of pages) await page.waitForFunction(() => state?.paused === true);
    console.log('Viewer one paused both players through the host.');
    const paused = await pages[0].evaluate(() => state.position);
    await sleep(1000);
    const still = await pages[1].evaluate(() => state.position);
    if (Math.abs(still - paused) > 0.2) throw Error('Host timeline moved while paused');
    await pages[1].locator('#seek').evaluate(el => { el.value = '45'; el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true})); });
    for (const page of pages) await page.waitForFunction(() => state?.position >= 44 && state?.position <= 46);
    console.log('Viewer two sought to 45 seconds while paused.');
    const late = await viewerPage(2); await late.goto(invite, {waitUntil: 'domcontentloaded'}); await late.locator('#join').click();
    await late.waitForFunction(() => state?.paused && state.position >= 44 && document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames >= 1);
    console.log('A third viewer joined while paused and decoded the still frame.');
    await late.close();
    await pages[1].locator('#play').click();
    for (const page of pages) await page.waitForFunction(() => state?.paused === false && state?.position > 46);
    const before = await pages[0].evaluate(() => document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames);
    await sleep(1500);
    const after = await pages[0].evaluate(() => document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames);
    if (after <= before) throw Error('Video stopped decoding after seeking');
    console.log('Both viewers resumed and decoded frames after the seek.');
    fs.writeFileSync(path.join(dir, 'end-session'), 'end');
    for (const page of pages) await page.waitForFunction(() => document.querySelector('#status').textContent.includes('ended'));
    const expired = await browser.newPage(); await expired.goto(invite, {waitUntil: "domcontentloaded"}); await expired.locator('#join').click();
    await expired.waitForFunction(() => document.querySelector('#status').textContent.includes('expired'));
    console.log('Ending the host session disconnected both viewers and revoked the link.');
    fs.writeFileSync(path.join(dir, 'done'), 'ok');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
