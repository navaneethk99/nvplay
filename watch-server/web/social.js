/* Session-only names, participants and plain-text chat. */
'use strict';
class PartySocial {
  constructor({send, notify, showControls}) {
    this.send = send; this.notify = notify; this.showControls = showControls;
    this.name = ''; this.connected = false; this.messages = []; this.participants = new Map(); this.unread = 0; this.pending = undefined;
    try { document.getElementById('display-name').value = sessionStorage.getItem('nvplay-watch-name') || ''; } catch {}
    document.getElementById('chat-toggle').onclick = () => this.toggle();
    document.getElementById('chat-close').onclick = () => this.toggle(false);
    document.getElementById('chat-form').onsubmit = event => { event.preventDefault(); this.submit(); };
    document.getElementById('chat-input').addEventListener('input', () => this.update());
    document.getElementById('chat-input').addEventListener('keydown', event => {
      if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); this.submit(); }
    });
    document.getElementById('display-name').addEventListener('input', event => event.target.setCustomValidity(''));
    document.addEventListener('keydown', event => {
      if (event.key === 'Escape' && this.isOpen) { event.preventDefault(); event.stopImmediatePropagation(); this.toggle(false); }
    });
    this.update();
  }
  get isOpen() { return !document.getElementById('chat-panel').hidden; }
  prepareJoin() {
    const input = document.getElementById('display-name');
    const name = input.value.trim().replace(/\s+/gu, ' ');
    if (!name || [...name].length > 32 || /[\u0000-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/u.test(input.value)) {
      input.setCustomValidity('Choose a name of 1–32 characters.'); input.reportValidity(); input.focus(); return false;
    }
    this.name = name; input.value = name;
    try { sessionStorage.setItem('nvplay-watch-name', name); } catch {}
    document.getElementById('name-fields').hidden = true; input.disabled = true;
    return true;
  }
  rejectJoin(message) {
    this.disconnected();
    document.getElementById('name-fields').hidden = false;
    document.getElementById('display-name').disabled = false;
    document.getElementById('name-error').textContent = message;
  }
  joined(message) {
    this.connected = true; this.peer = message.peer; this.pending = undefined;
    this.participants = new Map((message.participants || []).map(p => [p.peer, p]));
    this.messages = (message.history || []).slice(-40);
    document.getElementById('name-error').textContent = '';
    this.renderParticipants(); this.renderMessages(); this.update();
  }
  peerJoined(message) {
    if (this.participants.has(message.peer)) return;
    this.participants.set(message.peer, {peer: message.peer, name: message.name || 'Viewer', host: false});
    this.renderParticipants();
    if (message.peer !== this.peer) this.notify(`${message.name || 'Viewer'} joined the watch party`);
  }
  peerLeft(message) { this.participants.delete(message.peer); this.renderParticipants(); }
  disconnected() { this.connected = false; this.pending = undefined; this.update(); }
  ended() { this.disconnected(); document.getElementById('name-fields').hidden = true; }
  toggle(open = !this.isOpen) {
    document.getElementById('chat-panel').hidden = !open;
    if (open) { this.unread = 0; this.renderMessages(); const list = document.getElementById('chat-messages'); list.scrollTop = list.scrollHeight; document.getElementById('chat-input').focus(); }
    else document.getElementById('player').focus({preventScroll: true});
    document.getElementById('chat-toggle').setAttribute('aria-expanded', String(open));
    this.update(); this.showControls();
  }
  submit() {
    const text = document.getElementById('chat-input').value.trim();
    if (!this.connected || this.pending !== undefined || !text || [...text].length > 500) return;
    this.pending = text;
    document.getElementById('chat-error').textContent = '';
    if (!this.send({type: 'chat_send', text})) { this.pending = undefined; document.getElementById('chat-error').textContent = 'Chat is reconnecting. Your draft is kept.'; }
    this.update();
  }
  rejected(message) { this.pending = undefined; document.getElementById('chat-error').textContent = message; this.update(); }
  received(entry) {
    if (this.messages.some(m => m.id === entry.id)) return;
    if (entry.sender.peer === this.peer) {
      const input = document.getElementById('chat-input');
      if (input.value.trim() === this.pending) input.value = '';
      this.pending = undefined;
    } else if (!this.isOpen) this.unread++;
    this.messages.push(entry); this.messages = this.messages.slice(-40);
    this.renderMessages(entry); this.update();
  }
  renderParticipants() {
    document.getElementById('participant-count').textContent = `Participants (${this.participants.size})`;
    const list = document.getElementById('participants'); list.replaceChildren();
    for (const p of this.participants.values()) {
      const item = document.createElement('li');
      item.textContent = `${p.name}${p.host ? ' · Host' : p.peer === this.peer ? ' · You' : ' · Viewer'}`;
      list.append(item);
    }
  }
  messageNode(entry) {
    const item = document.createElement('article'); item.className = 'chat-message';
    const author = document.createElement('strong'); author.textContent = entry.sender.host ? 'Host' : `${entry.sender.name}${entry.sender.peer === this.peer ? ' · You' : ''}`;
    const time = document.createElement('time'); time.dateTime = new Date(entry.sent_at).toISOString(); time.textContent = new Date(entry.sent_at).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});
    const body = document.createElement('p'); body.textContent = entry.text;
    item.append(author, time, body); return item;
  }
  renderMessages(newEntry) {
    const list = document.getElementById('chat-messages');
    const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 48;
    if (newEntry) {
      list.querySelector('.chat-empty')?.remove();
      list.append(this.messageNode(newEntry));
      while (list.childElementCount > 40) list.firstElementChild.remove();
    } else {
      // Initial history is quiet; incremental messages are announced by the log.
      list.setAttribute('aria-live', 'off'); list.replaceChildren();
      if (!this.messages.length) { const empty = document.createElement('p'); empty.className = 'chat-empty'; empty.textContent = 'Say hello to everyone watching.'; list.append(empty); }
      for (const entry of this.messages) list.append(this.messageNode(entry));
      list.setAttribute('aria-live', 'polite');
    }
    if (atBottom || !this.connected) list.scrollTop = list.scrollHeight;
  }
  update() {
    const input = document.getElementById('chat-input');
    const count = [...input.value].length;
    document.getElementById('chat-count').textContent = `${count}/500`;
    document.getElementById('chat-send').disabled = !this.connected || this.pending !== undefined || !input.value.trim() || count > 500;
    document.getElementById('chat-toggle').disabled = !this.connected && this.messages.length === 0;
    document.getElementById('chat-unread').textContent = this.unread || '';
    document.getElementById('chat-toggle').setAttribute('aria-label', this.unread ? `Chat, ${this.unread} unread messages` : 'Chat');
    document.getElementById('chat-connection').textContent = this.connected ? 'Messages stay in this session' : 'Chat disconnected · your draft is kept';
  }
}
