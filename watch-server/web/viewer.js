/* A WebRTC MediaStream is live; the shared movie timeline comes from the host. */
'use strict';
const $ = id => document.getElementById(id);
const video = $('video');
const room = location.pathname.split('/').pop();
const token = location.hash.slice(1);
let pendingCandidates = [];
let socket, peer, peerId, state, receivedAt = 0, dragging = false, ended = false, reconnectTimer;
const stream = new MediaStream();
const stamp = value => {
  const s = Math.max(0, Math.floor(value || 0));
  return s >= 3600 ? `${Math.floor(s / 3600)}:${String(Math.floor(s / 60) % 60).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};
function send(message) { if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message)); }
function status(message) { $('status').textContent = message; }
function enabled(value) { $('play').disabled = !value; $('seek').disabled = !value || !state?.duration; }
function disposePeer() {
  peer?.close(); peer = null; pendingCandidates = [];
  for (const track of stream.getTracks()) { stream.removeTrack(track); track.stop(); }
}
function finish(message) {
  ended = true; clearTimeout(reconnectTimer); enabled(false); disposePeer(); socket?.close();
  $('overlay').hidden = false; $('overlay-text').textContent = message; $('join').hidden = true; status(message);
}
async function resumeVideo() {
  try { await video.play(); $('overlay').hidden = true; }
  catch { $('overlay').hidden = false; $('overlay-text').textContent = 'Click to enable video and sound'; $('join').hidden = false; $('join').textContent = 'Enable playback'; }
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
  if (ended) return;
  status('Connecting to your friend…'); enabled(false); disposePeer();
  socket = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws`);
  socket.onopen = () => send({type: 'join', room, token});
  socket.onmessage = event => {
    // Serialize async SDP handling so trickled candidates cannot overtake the offer.
    messageQueue = messageQueue.then(async () => {
      const message = JSON.parse(event.data);
      switch (message.type) {
        case 'joined':
          peerId = message.peer;
          peer = new RTCPeerConnection({iceServers: message.ice_servers});
          peer.onicecandidate = event => { if (event.candidate) send({type: 'candidate', peer: peerId, candidate: event.candidate.toJSON()}); };
          peer.ontrack = event => {
            if (!stream.getTracks().some(t => t.id === event.track.id)) stream.addTrack(event.track);
            if (video.srcObject !== stream) video.srcObject = stream;
            resumeVideo();
          };
          peer.onconnectionstatechange = () => {
            if (peer?.connectionState === 'connected') { enabled(true); status('Connected · Everyone can control playback'); }
            if (peer?.connectionState === 'failed') { enabled(false); socket.close(); }
          };
          break;
        case 'offer': await offer(message); break;
        case 'candidate':
          if (peer && message.peer === peerId) {
            if (peer.remoteDescription) await peer.addIceCandidate(message.candidate);
            else if (pendingCandidates.length < 64) pendingCandidates.push(message.candidate);
          }
          break;
        case 'state':
          state = message.state; receivedAt = performance.now();
          $('title').textContent = state.title || 'Watch together'; $('play').textContent = state.paused ? 'Play' : 'Pause';
          // Keep the live stream rendering so a paused seek/new join can show
          // the host's still frame. No audio is sent while paused.
          video.muted = state.paused || state.buffering;
          resumeVideo();
          if (state.buffering) status('The host is buffering…');
          else if (peer?.connectionState === 'connected') status('Connected · Everyone can control playback');
          break;
        case 'ended': finish('The host ended this watch party.'); break;
        case 'error': finish(message.message); break;
      }
    }).catch(() => { status('Could not establish the video connection. Reconnecting…'); socket.close(); });
  };
  socket.onerror = () => status('Connection interrupted. Reconnecting…');
  socket.onclose = () => {
    enabled(false); disposePeer();
    if (!ended) { status('Connection lost. Reconnecting…'); reconnectTimer = setTimeout(connect, 2000); }
  };
}
$('join').onclick = () => {
  if (!socket) {
    if (!token || !room || !window.RTCPeerConnection) { finish('Open a complete viewing link in a browser that supports WebRTC.'); return; }
    // Unlock media playback during this user gesture, including on mobile Safari.
    video.play().catch(() => {}); $('overlay').hidden = true; connect();
  } else { video.play().then(() => { $('overlay').hidden = true; }).catch(() => status('Your browser could not start playback.')); }
};
$('play').onclick = () => { if (state) send({type: 'control', control: {action: state.paused ? 'play' : 'pause'}}); };
$('seek').oninput = () => { dragging = true; $('time').textContent = `${stamp(Number($('seek').value))} / ${stamp(state?.duration)}`; };
$('seek').onchange = () => { send({type: 'control', control: {action: 'seek', position: Number($('seek').value)}}); dragging = false; };
$('volume').oninput = () => { video.volume = Number($('volume').value); };
$('fullscreen').onclick = () => { const surface = document.querySelector('.stage'); if (surface.requestFullscreen) surface.requestFullscreen().catch(() => {}); else if (video.webkitEnterFullscreen) video.webkitEnterFullscreen(); };
setInterval(() => {
  if (!state || dragging) return;
  // Cap interpolation during a lost connection; never advance an unconfirmed timeline indefinitely.
  const elapsed = !state.paused && !state.buffering ? Math.min((performance.now() - receivedAt) / 1000, 1) * state.speed : 0;
  const position = Math.min(state.duration || Infinity, state.position + elapsed);
  $('seek').max = state.duration || 1; $('seek').value = position;
  $('time').textContent = `${stamp(position)} / ${stamp(state.duration)}`;
}, 100);
window.addEventListener('pagehide', () => { ended = true; socket?.close(); disposePeer(); });
