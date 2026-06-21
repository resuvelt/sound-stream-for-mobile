'use strict';

const path = require('path');
const http = require('http');
const os = require('os');
const express = require('express');
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
// Единственная страница — плеер слушателя. Открывается по http://IP:PORT
const listenPage = (req, res) => res.sendFile(path.join(__dirname, 'public', 'listen.html'));
app.get('/', listenPage);
app.get('/listen', listenPage);

const server = http.createServer(app);
const wss = new WebSocketServer({ server });

let formatMsg = null;          // JSON-описание формата PCM (sampleRate, channels)
const listeners = new Set();   // ws телефонов/браузеров, которые слушают

// Системный звук ПК через WASAPI loopback. Захват «по требованию»: поднимается
// при первом слушателе и гаснет, когда слушателей не осталось.
const capture = new SystemAudioCapture();
capture.onFrame = (buf) => {
  for (const l of listeners) {
    if (l.readyState === l.OPEN) l.send(buf, { binary: true });
  }
};

function broadcastJSON(ws, obj) {
  if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(obj));
}

function startCaptureIfNeeded() {
  if (listeners.size === 0 || capture.running || !capture.available) return;
  if (capture.start()) {
    formatMsg = JSON.stringify({ type: 'format', sampleRate: capture.sampleRate, channels: 1 });
    console.log(`[capture] loopback запущен: ${capture.deviceName} @ ${capture.sampleRate} Hz`);
    for (const l of listeners) {
      broadcastJSON(l, { type: 'live' });
      if (l.readyState === l.OPEN) l.send(formatMsg);
    }
  }
}

function stopCapture() {
  if (!capture.running) return;
  capture.stop();
  formatMsg = null;
  console.log('[capture] loopback остановлен (нет слушателей)');
}

wss.on('connection', (ws) => {
  listeners.add(ws);
  console.log(`[listener] connected (${listeners.size} total)`);

  if (capture.running) {
    // Захват уже идёт — сразу сообщаем live и формат.
    broadcastJSON(ws, { type: 'live' });
    if (formatMsg) ws.send(formatMsg);
  } else {
    // Захвата нет — сообщаем offline и поднимаем его; startCaptureIfNeeded
    // сам разошлёт live+format всем слушателям.
    broadcastJSON(ws, { type: 'offline' });
    startCaptureIfNeeded();
  }

  ws.on('close', () => {
    listeners.delete(ws);
    console.log(`[listener] disconnected (${listeners.size} total)`);
    if (listeners.size === 0) stopCapture();
  });
});

server.listen(PORT, () => {
  console.log(`\n  sound-stream running`);
  console.log(capture.available
    ? `  Источник: системный звук ПК (WASAPI loopback) — включится при подключении слушателя`
    : `  ВНИМАНИЕ: системный захват недоступен (audify не загрузился) — звука не будет`);
  const ips = lanAddresses();
  if (ips.length) {
    console.log(`  Открой на телефоне (та же WiFi):`);
    for (const ip of ips) console.log(`      http://${ip}:${PORT}/`);
  } else {
    console.log(`  Открой: http://localhost:${PORT}/ (LAN-адрес не найден)`);
  }
  console.log('');
});

// Чистая остановка захвата при выходе.
for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => { capture.stop(); process.exit(0); });
}
