'use strict';

const path = require('path');
const http = require('http');
const os = require('os');
const express = require('express');
const QRCode = require('qrcode');
const { WebSocketServer } = require('ws');
const SystemAudioCapture = require('./capture');

const PORT = process.env.PORT || 47823;

// Локальные (LAN) IPv4-адреса этого ПК, без виртуальных адаптеров.
function lanAddresses() {
  const skip = /(vEthernet|WSL|Hyper-V|Default Switch|outline|VirtualBox|VMware|Loopback|docker|veth|VPN|TAP)/i;
  const out = [];
  for (const [name, addrs] of Object.entries(os.networkInterfaces())) {
    if (skip.test(name)) continue;
    for (const a of addrs || []) {
      if (a.family === 'IPv4' && !a.internal) out.push(a.address);
    }
  }
  const rank = ip => ip.startsWith('192.168.') ? 0 : ip.startsWith('10.') ? 1 : 2;
  return out.sort((x, y) => rank(x) - rank(y));
}

const app = express();

// Корень → страница слушателя: телефон сразу попадает на неё по http://IP:PORT
app.get('/', (req, res) => res.sendFile(path.join(__dirname, 'public', 'listen.html')));
app.get('/listen', (req, res) => res.sendFile(path.join(__dirname, 'public', 'listen.html')));
// Браузерный источник — необязателен (звук берётся системно): http://IP:PORT/source
app.get('/source', (req, res) => res.sendFile(path.join(__dirname, 'public', 'index.html')));

app.use(express.static(path.join(__dirname, 'public'), { index: false }));
app.get('/nosleep.js', (req, res) => res.sendFile(require.resolve('nosleep.js')));
app.get('/info', (req, res) => res.json({ port: PORT, addresses: lanAddresses() }));

// QR-код LAN-адреса слушателя — генерируется на сервере, без внешних библиотек.
app.get('/qr', async (req, res) => {
  const ips = lanAddresses();
  if (!ips.length) return res.status(404).end();
  const url = `http://${ips[0]}:${PORT}/`;
  try {
    const png = await QRCode.toBuffer(url, { width: 200, margin: 1 });
    res.type('png').send(png);
  } catch (e) {
    res.status(500).end();
  }
});

const server = http.createServer(app);
const wss = new WebSocketServer({ server });

// State of the single live broadcast.
let broadcaster = null;        // ws of the PC that captures system audio (браузерный источник)
let formatMsg = null;          // JSON string describing the PCM format (sampleRate, channels)
const listeners = new Set();   // ws of phones / browsers that listen

// Нативный системный захват (WASAPI loopback). Работает «по требованию»:
// поднимается, когда появляются слушатели, и гаснет, когда их не осталось.
// Браузерный источник (если подключён) имеет приоритет над системным.
const capture = new SystemAudioCapture();
capture.onFrame = (buf) => {
  for (const l of listeners) {
    if (l.readyState === l.OPEN) l.send(buf, { binary: true });
  }
};

function captureFormatMsg() {
  return JSON.stringify({ type: 'format', sampleRate: capture.sampleRate, channels: 1 });
}

function startServerCaptureIfNeeded() {
  if (broadcaster || listeners.size === 0 || capture.running || !capture.available) return;
  if (capture.start()) {
    formatMsg = captureFormatMsg();
    console.log(`[capture] loopback запущен: ${capture.deviceName} @ ${capture.sampleRate} Hz`);
    for (const l of listeners) {
      broadcastJSON(l, { type: 'live' });
      if (l.readyState === l.OPEN) l.send(formatMsg);
    }
  }
}

function stopServerCapture() {
  if (!capture.running) return;
  capture.stop();
  console.log('[capture] loopback остановлен (нет слушателей)');
  if (!broadcaster) {
    formatMsg = null;
    for (const l of listeners) broadcastJSON(l, { type: 'offline' });
  }
}

function broadcastJSON(ws, obj) {
  if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(obj));
}

function updateListenerCount() {
  if (broadcaster) broadcastJSON(broadcaster, { type: 'listeners', count: listeners.size });
}

wss.on('connection', (ws, req) => {
  const url = new URL(req.url, 'http://localhost');
  const role = url.searchParams.get('role');

  if (role === 'broadcaster') {
    // Only one broadcaster at a time — newest wins.
    if (broadcaster && broadcaster !== ws) {
      broadcastJSON(broadcaster, { type: 'replaced' });
      broadcaster.close();
    }
    // Браузерный источник важнее системного — глушим loopback, чтобы не было
    // двойного звука, и переходим на поток из браузера.
    if (capture.running) capture.stop();

    broadcaster = ws;
    formatMsg = null;
    console.log('[broadcaster] connected');

    // Tell every waiting listener the stream is live.
    for (const l of listeners) broadcastJSON(l, { type: 'live' });
    updateListenerCount();

    ws.on('message', (data, isBinary) => {
      if (!isBinary) {
        // A 'format' control message — cache it so that listeners who join
        // mid-stream learn the sample rate / channel count, then relay it.
        const text = data.toString();
        try { if (JSON.parse(text).type === 'format') formatMsg = text; } catch {}
        for (const l of listeners) {
          if (l.readyState === l.OPEN) l.send(text);
        }
        return;
      }
      // Binary = raw PCM (Int16) audio frame. Relay to every listener.
      for (const l of listeners) {
        if (l.readyState === l.OPEN) l.send(data, { binary: true });
      }
    });

    ws.on('close', () => {
      if (broadcaster === ws) {
        broadcaster = null;
        formatMsg = null;
        console.log('[broadcaster] disconnected');
        for (const l of listeners) broadcastJSON(l, { type: 'offline' });
        // Браузерный источник отвалился — поднимаем системный захват обратно.
        startServerCaptureIfNeeded();
      }
    });
    return;
  }

  // Default role: listener.
  listeners.add(ws);
  console.log(`[listener] connected (${listeners.size} total)`);
  updateListenerCount();

  if (broadcaster || capture.running) {
    // Источник уже активен — сразу сообщаем live и формат.
    broadcastJSON(ws, { type: 'live' });
    if (formatMsg) ws.send(formatMsg);
  } else {
    // Источника нет — сообщаем offline и пробуем поднять системный захват.
    // Если он поднимется, startServerCaptureIfNeeded сам разошлёт live+format всем.
    broadcastJSON(ws, { type: 'offline' });
    startServerCaptureIfNeeded();
  }

  ws.on('close', () => {
    listeners.delete(ws);
    console.log(`[listener] disconnected (${listeners.size} total)`);
    updateListenerCount();
    if (listeners.size === 0) stopServerCapture(); // никого не осталось — гасим захват
  });
});

server.listen(PORT, () => {
  console.log(`\n  sound-stream running`);
  console.log(capture.available
    ? `  Источник: системный звук ПК (WASAPI loopback) — включится при подключении слушателя`
    : `  Источник: системный захват недоступен — браузерный режим: http://localhost:${PORT}/source`);
  const ips = lanAddresses();
  if (ips.length) {
    console.log(`  Телефон (слушать), та же WiFi:`);
    for (const ip of ips) console.log(`      http://${ip}:${PORT}/`);
  } else {
    console.log(`  Телефон (слушать): http://localhost:${PORT}/ (LAN-адрес не найден)`);
  }
  console.log('');
});

// Чистая остановка захвата при выходе.
for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => { capture.stop(); process.exit(0); });
}
