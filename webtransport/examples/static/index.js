// sessions[i] = { wt, datagramWriter, pendingPings }
const sessions = [null, null];
const sessionLabels = ['S1', 'S2'];
const sessionClasses = ['s1', 's2'];

// ── logging ───────────────────────────────────────────────────────────────────

function log(id, text, cls = '') {
  const el = document.getElementById(id);
  const div = document.createElement('div');
  if (cls) div.className = cls;
  div.textContent = text;
  el.appendChild(div);
  el.scrollTop = el.scrollHeight;
}

function sessionLog(logId, sessionIdx, text, cls = '') {
  log(logId, `[${sessionLabels[sessionIdx]}] ${text}`, cls || sessionClasses[sessionIdx]);
}

// ── text helpers ──────────────────────────────────────────────────────────────

const enc = new TextEncoder();
const dec = new TextDecoder();

async function readAll(readable) {
  const reader = readable.getReader();
  const chunks = [];
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const buf = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) { buf.set(c, off); off += c.length; }
  return dec.decode(buf);
}

// ── connection state ──────────────────────────────────────────────────────────

function setSessionConnected(i, on) {
  document.getElementById(`dot-${i}`).className = 'dot' + (on ? ' on' : '');
  document.getElementById(`status-${i}`).textContent =
    on ? `Session ${i + 1} connected` : `Session ${i + 1} disconnected`;
  // Enable controls once session 0 is up; session 1 connect btn follows session 0
  if (i === 0) {
    document.getElementById('connect-btn-1').disabled = !on;
    const hasAny = on;
    ['bidi-input', 'uni-input', 'bidi-btn', 'uni-btn', 'ping-btn']
      .forEach(id => document.getElementById(id).disabled = !hasAny);
  }
  document.getElementById(`connect-btn-${i}`).disabled = on;
}

// ── connect ───────────────────────────────────────────────────────────────────

async function connect(i) {
  const url = `https://${location.host}/`;
  let wt;
  try {
    wt = new WebTransport(url);
    await wt.ready;
  } catch (e) {
    log('push-log', `Session ${i + 1} connection failed: ${e}`, 'info');
    return;
  }

  const pendingPings = new Map();
  const datagramWriter = wt.datagrams.writable.getWriter();
  sessions[i] = { wt, datagramWriter, pendingPings };
  setSessionConnected(i, true);

  // ── server-initiated uni streams (welcome message) ────────────────────────
  (async () => {
    const reader = wt.incomingUnidirectionalStreams.getReader();
    while (true) {
      const { value: stream, done } = await reader.read();
      if (done) break;
      const text = await readAll(stream);
      sessionLog('push-log', i, `[uni] ${text}`, 'recv');
    }
  })().catch(() => {});

  // ── server-initiated bidi streams (greeting + reply) ─────────────────────
  (async () => {
    const reader = wt.incomingBidirectionalStreams.getReader();
    while (true) {
      const { value: stream, done } = await reader.read();
      if (done) break;
      const greeting = await readAll(stream.readable);
      sessionLog('push-log', i, `[bidi] ${greeting}`, 'recv');
      const writer = stream.writable.getWriter();
      await writer.write(enc.encode('Hello back from the browser!'));
      await writer.close();
    }
  })().catch(() => {});

  // ── incoming datagrams: ping echoes + uni acks ────────────────────────────
  (async () => {
    const reader = wt.datagrams.readable.getReader();
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      const text = dec.decode(value);
      if (text.startsWith('ack:')) {
        sessionLog('uni-log', i, `[ack] ${text.slice(4)}`, 'recv');
      } else if (text.startsWith('ping:')) {
        const id = text.slice(5);
        const sent = pendingPings.get(id);
        if (sent !== undefined) {
          pendingPings.delete(id);
          const rtt = Date.now() - sent;
          document.getElementById('rtt').textContent = `${sessionLabels[i]}: ${rtt} ms`;
          sessionLog('ping-log', i, `RTT: ${rtt} ms`, 'recv');
        }
      }
    }
  })().catch(() => {});

  wt.closed.then(() => {
    sessions[i] = null;
    setSessionConnected(i, false);
  }).catch(() => {
    sessions[i] = null;
    setSessionConnected(i, false);
  });
}

// ── bidi echo ─────────────────────────────────────────────────────────────────

async function sendBidi() {
  const i = parseInt(document.getElementById('bidi-session').value);
  const s = sessions[i];
  const input = document.getElementById('bidi-input');
  const msg = input.value.trim();
  if (!msg || !s) return;
  input.value = '';

  sessionLog('bidi-log', i, `→ "${msg}"`, 'sent');
  try {
    const stream = await s.wt.createBidirectionalStream();
    const writer = stream.writable.getWriter();
    await writer.write(enc.encode(msg));
    await writer.close();
    const response = await readAll(stream.readable);
    sessionLog('bidi-log', i, `← "${response}"`, 'recv');
  } catch (e) {
    sessionLog('bidi-log', i, `error: ${e}`, 'info');
  }
}

// ── uni log ───────────────────────────────────────────────────────────────────

async function sendUni() {
  const i = parseInt(document.getElementById('uni-session').value);
  const s = sessions[i];
  const input = document.getElementById('uni-input');
  const msg = input.value.trim();
  if (!msg || !s) return;
  input.value = '';

  sessionLog('uni-log', i, `→ "${msg}"`, 'sent');
  try {
    const stream = await s.wt.createUnidirectionalStream();
    const writer = stream.getWriter();
    await writer.write(enc.encode(msg));
    await writer.close();
  } catch (e) {
    sessionLog('uni-log', i, `error: ${e}`, 'info');
  }
}

// ── datagram ping ─────────────────────────────────────────────────────────────

async function sendPing() {
  const i = parseInt(document.getElementById('ping-session').value);
  const s = sessions[i];
  if (!s) return;
  const id = Math.random().toString(36).slice(2, 8);
  s.pendingPings.set(id, Date.now());
  sessionLog('ping-log', i, `→ ping (${id})`, 'sent');
  try {
    await s.datagramWriter.write(enc.encode(`ping:${id}`));
  } catch (e) {
    sessionLog('ping-log', i, `error: ${e}`, 'info');
  }
}

// ── wire up UI ────────────────────────────────────────────────────────────────

document.getElementById('connect-btn-0').addEventListener('click', () => connect(0));
document.getElementById('connect-btn-1').addEventListener('click', () => connect(1));
document.getElementById('bidi-btn').addEventListener('click', sendBidi);
document.getElementById('uni-btn').addEventListener('click', sendUni);
document.getElementById('ping-btn').addEventListener('click', sendPing);

document.getElementById('bidi-input').addEventListener('keydown', e => {
  if (e.key === 'Enter') sendBidi();
});
document.getElementById('uni-input').addEventListener('keydown', e => {
  if (e.key === 'Enter') sendUni();
});
