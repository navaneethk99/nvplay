/* The host owns the timeline; media is provider playback or a live WebRTC stream. */
'use strict';
const $ = id => document.getElementById(id);
const video = $('video');
const player = $('player');
let userMuted = false, hideTimer, toastTimer;
let suspended = false, disconnectTimer, wakeLock;
const room = location.pathname.split('/').pop();
const token = location.hash.slice(1);
let pendingCandidates = [];
let socket, peer, peerId, state, receivedAt = 0, dragging = false, ended = false, reconnectTimer;
const stream = new MediaStream();
let access = {play_pause: false, seek: false};
let controlsReady = false;
let direct, pingSent = 0, latency = 0;
function syncDirect() { direct?.sync(state, (performance.now() - receivedAt) / 1000, latency); }
function autoplayPrompt() { $("overlay").hidden = false; $("overlay-text").textContent = "Click to enable video and sound"; $("join").hidden = false; $("join").textContent = "Enable playback"; }
let networkWarning = false;
let videoStalled = false, lastVideoProgress = performance.now(), decodedFrames = 0, qualitySample;
const accessStatus = () => videoStalled ? 'Waiting for video…' : networkWarning ? 'Connection unstable' : 'Connected';
const stamp = value => {
  const s = Math.max(0, Math.floor(value || 0));
  return s >= 3600 ? `${Math.floor(s / 3600)}:${String(Math.floor(s / 60) % 60).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};
function send(message) { if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message)); }
function status(message) {
  if ($('status').textContent !== message) $('status').textContent = message;
  $('screen-status').textContent = message;
}
function notify(message) {
  $('toast').textContent = message; $('toast').hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { $('toast').hidden = true; }, 2300);
}
function expanded() { return document.fullscreenElement === player || player.classList.contains('is-expanded'); }
function showControls() {
  player.classList.remove('controls-hidden'); clearTimeout(hideTimer);
  if (expanded() && controlsReady && state && !state.paused && !dragging && !$('shortcut-dialog').open) {
    hideTimer = setTimeout(() => {
      if (!$('controls').matches(':hover') && !player.querySelector(':focus-visible') && !state?.paused && !dragging) player.classList.add('controls-hidden');
    }, 3200);
  }
}
function syncFullscreen() {
  const active = expanded();
  $('fullscreen').setAttribute('aria-label', active ? 'Exit fullscreen (F)' : 'Enter fullscreen (F)');
  $('fullscreen').title = active ? 'Exit fullscreen (F)' : 'Enter fullscreen (F)';
  $('exit-expanded').hidden = !active;
  showControls();
}
async function toggleFullscreen() {
  if (document.fullscreenElement) await document.exitFullscreen().catch(() => {});
  else if (player.classList.contains('is-expanded')) {
    player.classList.remove('is-expanded'); document.body.classList.remove('expanded');
  } else {
    try {
      if (!player.requestFullscreen) throw Error('Use page fullscreen');
      await player.requestFullscreen();
    } catch {
      // Keep our full player and shared timeline on platforms without element
      // fullscreen, rather than switching to native MediaStream controls.
      player.classList.add('is-expanded'); document.body.classList.add('expanded');
    }
  }
  syncFullscreen(); player.focus({preventScroll: true});
}
function playback() {
  if (!state || !controlsReady || !access.play_pause) return;
  send({type: 'control', control: {action: state.paused ? 'play' : 'pause'}}); showControls();
}
function seekTo(position) {
  if (!state || !controlsReady || !access.seek || !state.duration) return;
  const target = Math.max(0, Math.min(position, state.duration));
  send({type: 'control', control: {action: 'seek', position: target}}); showControls();
}
function syncVolume() {
  video.muted = userMuted || video.volume === 0 || !!state?.paused || !!state?.buffering;
  direct?.volume(video.volume, video.muted);
  $('mute').setAttribute('aria-label', userMuted || video.volume === 0 ? 'Unmute (M)' : 'Mute (M)');
  $('mute').setAttribute('aria-pressed', String(userMuted || video.volume === 0));
  $('mute').style.opacity = userMuted || video.volume === 0 ? '.5' : '1';
}
function updateAccess() {
  $('control-note').textContent = !controlsReady ? 'Connecting…' : access.play_pause && access.seek ? 'Shared controls' : !access.play_pause && !access.seek ? 'Watch only' : 'Limited controls';
  $('rights').textContent = `Play / pause: ${access.play_pause ? 'allowed' : 'host only'}. Seeking: ${access.seek ? 'allowed' : 'host only'}. Your volume and fullscreen only affect you.`;
}
function enabled(value) {
  controlsReady = value; $('play').disabled = !value || !access.play_pause;
  for (const id of ['seek', 'back', 'forward']) $(id).disabled = !value || !access.seek || !state?.duration;
  updateAccess();
}
function disposePeer() {
  clearTimeout(disconnectTimer);
  direct?.destroy(); direct = undefined; state = undefined; latency = 0; pingSent = 0;
  if (peer) { peer.onconnectionstatechange = null; peer.onicecandidate = null; peer.close(); }
  peer = null; pendingCandidates = []; qualitySample = undefined; decodedFrames = 0;
  for (const track of stream.getTracks()) { stream.removeTrack(track); track.stop(); }
}
function finish(message) {
  ended = true; wakeLock?.release().catch(() => {}); clearTimeout(reconnectTimer); enabled(false); disposePeer(); socket?.close();
  showControls(); $('overlay-description').hidden = true;
  $('overlay').hidden = false; $('overlay-text').textContent = message; $('join').hidden = true; status(message);
}
async function resumeVideo() {
  if (direct) { syncDirect(); return; }
  try { await video.play(); if (!ended) $('overlay').hidden = true; }
  catch { if (ended) return; $('overlay').hidden = false; $('overlay-text').textContent = 'Click to enable video and sound'; $('join').hidden = false; $('join').textContent = 'Enable playback'; }
}
async function offer(message) {
  if (!peer || message.peer !== peerId) return;
  await peer.setRemoteDescription({type: 'offer', sdp: message.sdp});
  for (const candidate of pendingCandidates.splice(0)) await peer.addIceCandidate(candidate);
  await peer.setLocalDescription(await peer.createAnswer());
  send({type: 'answer', peer: peerId, sdp: peer.localDescription.sdp});
}
let messageQueue = Promise.resolve();
function connect() {
  if (ended || suspended) return;
  clearTimeout(reconnectTimer);
  if (socket) { socket.onclose = null; socket.close(); }
  status('Connecting to your friend…'); enabled(false); disposePeer();
  socket = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws`);
  const connectionSocket = socket;
  messageQueue = Promise.resolve();
  socket.onopen = () => { if (socket === connectionSocket) send({type: 'join', room, token, client_offers: true}); };
  socket.onmessage = event => {
    // Serialize async SDP handling so trickled candidates cannot overtake the offer.
    messageQueue = messageQueue.then(async () => {
      if (socket !== connectionSocket || ended || suspended) return;
      const message = JSON.parse(event.data);
      switch (message.type) {
        case 'joined':
          access = message.access;
          peerId = message.peer;
          if (message.media) {
            direct = new BrowserPlayback(message.media, video,
              () => { enabled(true); keepAwake(); $('overlay').hidden = true; status('Connected · direct playback'); syncDirect(); syncVolume(); },
              error => { enabled(false); status(error); $('overlay').hidden = false; $('overlay-text').textContent = error; $('overlay-description').hidden = false; $('overlay-description').textContent = 'Ask the host to start a new session with “Stream from my device instead”.'; $('join').hidden = true; },
              autoplayPrompt);
            direct.start();
            break;
          }
          peer = new RTCPeerConnection({iceServers: message.ice_servers});
          peer.onicecandidate = event => { if (event.candidate) send({type: 'candidate', peer: peerId, candidate: event.candidate.toJSON()}); };
          peer.ontrack = event => {
            // Keep a small jitter cushion instead of accumulating a long live
            // playback queue. Browsers without these hints retain their defaults.
            try {
              if ('jitterBufferTarget' in event.receiver) event.receiver.jitterBufferTarget = 80;
              else if ('playoutDelayHint' in event.receiver) event.receiver.playoutDelayHint = 0.08;
            } catch { /* Optional receiver hints vary between mobile browsers. */ }
            if (!stream.getTracks().some(t => t.id === event.track.id)) stream.addTrack(event.track);
            if (video.srcObject !== stream) video.srcObject = stream;
            resumeVideo();
          };
          peer.onconnectionstatechange = () => {
            clearTimeout(disconnectTimer);
            if (peer?.connectionState === 'disconnected') disconnectTimer = setTimeout(() => { if (peer?.connectionState === 'disconnected') socket?.close(); }, 8000);
            if (peer?.connectionState === 'connected') { lastVideoProgress = performance.now(); videoStalled = false; enabled(true); keepAwake(); status(accessStatus()); }
            if (peer?.connectionState === 'failed') { enabled(false); socket.close(); }
          };
          if (message.client_offers) {
            peer.addTransceiver('video', {direction: 'recvonly'});
            peer.addTransceiver('audio', {direction: 'recvonly'});
            await peer.setLocalDescription(await peer.createOffer());
            send({type: 'offer', peer: peerId, sdp: peer.localDescription.sdp});
          }
          break;
        case 'answer':
          if (peer && message.peer === peerId) {
            await peer.setRemoteDescription({type: 'answer', sdp: message.sdp});
            for (const candidate of pendingCandidates.splice(0)) await peer.addIceCandidate(candidate);
          }
          break;
        case 'offer': await offer(message); break;
        case 'candidate':
          if (peer && message.peer === peerId) {
            if (peer.remoteDescription) await peer.addIceCandidate(message.candidate);
            else if (pendingCandidates.length < 64) pendingCandidates.push(message.candidate);
          }
          break;
        case 'access': access = message.access; dragging = false; enabled(controlsReady); status(accessStatus()); break;
        case 'pong': if (pingSent) { latency = Math.min((performance.now() - pingSent) / 2000, .5); pingSent = 0; } break;
        case 'denied': status(message.message); break;
        case 'state':
          state = message.state; receivedAt = performance.now(); enabled(controlsReady);
          $('title').textContent = state.title || 'Watch together';
          document.title = `${state.title || 'Watch together'} · nvplay`;
          $('play').dataset.playing = String(!state.paused);
          $('play').setAttribute('aria-label', state.paused ? 'Play (Space)' : 'Pause (Space)');
          if (state.paused) showControls();
          // Keep the live stream rendering so a paused seek/new join can show
          // the host's still frame. No audio is sent while paused.
          syncVolume();
          resumeVideo();
          if (state.buffering) status('The host is buffering…');
          else if (direct?.ready && !direct.failed) status('Connected · direct playback');
          else if (peer?.connectionState === 'connected') status(accessStatus());
          break;
        case 'ended': finish('The host ended this watch party.'); break;
        case 'error': finish(message.message); break;
      }
    }).catch(() => { if (socket !== connectionSocket || ended || suspended) return; status('Could not establish the video connection. Reconnecting…'); connectionSocket.close(); });
  };
  socket.onerror = () => status('Connection interrupted. Reconnecting…');
  socket.onclose = () => {
    if (socket !== connectionSocket) return;
    enabled(false); disposePeer();
    if (!ended && !suspended) { status('Connection lost. Reconnecting…'); reconnectTimer = setTimeout(connect, 2000); }
  };
}
$('join').onclick = () => {
  if (!socket) {
    if (!token || !room) { finish('Open a complete viewing link in a supported browser.'); return; }
    // Unlock media playback during this user gesture, including on mobile Safari.
    video.play().catch(() => {}); $('overlay').hidden = true; connect();
  } else if (direct) { $('overlay').hidden = true; direct.resume(); syncDirect(); }
  else { video.play().then(() => { $('overlay').hidden = true; }).catch(() => status('Your browser could not start playback.')); }
};
$('play').onclick = playback;
$('back').onclick = () => seekTo((state?.position || 0) - 10);
$('forward').onclick = () => seekTo((state?.position || 0) + 10);
$('seek').oninput = () => { dragging = true; showControls(); $('time').textContent = `${stamp(Number($('seek').value))} / ${stamp(state?.duration)}`; };
$('seek').onchange = () => { seekTo(Number($('seek').value)); dragging = false; showControls(); };
$('seek').addEventListener('pointercancel', () => { dragging = false; showControls(); });
$('volume').oninput = () => { video.volume = Number($('volume').value); userMuted = false; syncVolume(); };
$('mute').onclick = () => { userMuted = !userMuted; if (!userMuted && video.volume === 0) { video.volume = .5; $('volume').value = '.5'; } syncVolume(); showControls(); };
$('fullscreen').onclick = toggleFullscreen;
$('exit-expanded').onclick = toggleFullscreen;
video.ondblclick = toggleFullscreen;
video.addEventListener('resize', () => { if (video.videoHeight) $('quality').textContent = `${video.videoHeight}p`; });
$('shortcuts').onclick = () => { showControls(); $('shortcut-dialog').showModal(); };
$('close-shortcuts').onclick = () => { $('shortcut-dialog').close(); player.focus(); showControls(); };
$('shortcut-dialog').addEventListener('close', showControls);
document.addEventListener('fullscreenchange', syncFullscreen);
for (const event of ['pointermove', 'pointerdown', 'focusin']) player.addEventListener(event, showControls);
$('controls').addEventListener('pointerleave', showControls);
$('copy-link').onclick = async () => {
  try { await navigator.clipboard.writeText(location.href); notify('Invite copied. Send it to a friend.'); }
  catch { notify('Copy this page’s address to share the invite.'); }
};
$('leave').onclick = () => { finish('You left the watch party.'); $('rejoin').hidden = false; };
$('rejoin').onclick = () => location.reload();
document.addEventListener('keydown', event => {
  if (event.ctrlKey || event.metaKey || event.altKey || event.target.closest('input,button,textarea,select,a,[contenteditable]')) return;
  if ($('shortcut-dialog').open) return;
  const key = event.key.toLowerCase();
  if (key === ' ' || key === 'k') { event.preventDefault(); playback(); }
  else if (key === 'arrowleft' || key === 'j') { event.preventDefault(); seekTo((state?.position || 0) - 10); }
  else if (key === 'arrowright' || key === 'l') { event.preventDefault(); seekTo((state?.position || 0) + 10); }
  else if (key === 'm') { event.preventDefault(); $('mute').click(); }
  else if (key === 'f') { event.preventDefault(); toggleFullscreen(); }
  else if (key === 'escape' && player.classList.contains('is-expanded')) { event.preventDefault(); toggleFullscreen(); }
  else if (key === '?') { event.preventDefault(); $('shortcuts').click(); }
  showControls();
});
setInterval(() => {
  syncDirect();
  if (!state || dragging) return;
  // Cap interpolation during a lost connection; never advance an unconfirmed timeline indefinitely.
  const elapsed = !state.paused && !state.buffering ? Math.min((performance.now() - receivedAt) / 1000, 1) * state.speed : 0;
  const position = Math.min(state.duration || Infinity, state.position + elapsed);
  $('seek').max = state.duration || 1; $('seek').value = position;
  $('seek').style.setProperty('--progress', `${state.duration ? position / state.duration * 100 : 0}%`);
  $('seek').setAttribute('aria-valuetext', `${stamp(position)} of ${stamp(state.duration)}`);
  $('time').textContent = `${stamp(position)} / ${stamp(state.duration)}`;
}, 100);
// ICE connectivity alone does not prove that video is arriving and decoding.
setInterval(async () => {
  if (!peer || peer.connectionState !== 'connected' || ended) return;
  const connection = peer;
  try {
    const reports = [...(await connection.getStats()).values()];
    if (peer !== connection) return;
    const inbound = reports.find(report => report.type === 'inbound-rtp' && (report.kind || report.mediaType) === 'video');
    const now = performance.now();
    const frames = inbound?.framesDecoded ?? video.getVideoPlaybackQuality?.().totalVideoFrames ?? video.webkitDecodedFrameCount ?? decodedFrames;
    if (frames > decodedFrames || state?.paused || state?.buffering) lastVideoProgress = now;
    decodedFrames = frames;
    videoStalled = stream.getVideoTracks().length > 0 && now - lastVideoProgress > 5000;
    if (inbound && qualitySample && now > qualitySample.at) {
      const mbps = Math.max(0, inbound.bytesReceived - qualitySample.bytes) * 8 / ((now - qualitySample.at) * 1000);
      const lost = Math.max(0, (inbound.packetsLost || 0) - qualitySample.lost);
      const received = Math.max(0, (inbound.packetsReceived || 0) - qualitySample.received);
      networkWarning = lost + received > 20 && lost / (lost + received) > .05;
      $('quality').title = `${video.videoWidth} × ${video.videoHeight} · ${mbps.toFixed(1)} Mbps received`;
    }
    qualitySample = {at: now, bytes: inbound?.bytesReceived || 0, lost: inbound?.packetsLost || 0, received: inbound?.packetsReceived || 0};
    if (videoStalled) { showControls(); status(accessStatus()); }
    if (videoStalled && now - lastVideoProgress > 15000 && !document.hidden) socket?.close();
  } catch { /* Transport state callbacks handle reconnects. */ }
}, 2000);
async function keepAwake() {
  if (ended || document.hidden || !controlsReady || wakeLock) return;
  try {
    wakeLock = await navigator.wakeLock?.request('screen');
    wakeLock?.addEventListener('release', () => { wakeLock = undefined; });
  } catch { /* Screen wake lock is optional and may be denied in power-saving mode. */ }
}
function recoverConnection() {
  if (ended || suspended || !socket || document.hidden) return;
  keepAwake();
  if (socket.readyState === WebSocket.CONNECTING) return;
  if (socket.readyState !== WebSocket.OPEN || ['failed', 'disconnected', 'closed'].includes(peer?.connectionState)) connect();
  else resumeVideo();
}
window.addEventListener('online', recoverConnection);
document.addEventListener('visibilitychange', () => { if (!document.hidden) recoverConnection(); });
window.addEventListener('pagehide', () => {
  suspended = true; clearTimeout(reconnectTimer); wakeLock?.release().catch(() => {});
  if (socket) { socket.onclose = null; socket.close(); } disposePeer();
});
window.addEventListener('pageshow', () => { if (suspended) { suspended = false; if (socket && !ended) connect(); } });

setInterval(() => {
  if (socket?.readyState === WebSocket.OPEN && !pingSent) { pingSent = performance.now(); send({type: 'ping'}); }
}, 5000);
