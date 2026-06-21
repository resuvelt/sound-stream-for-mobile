'use strict';

const path = require('path');
const http = require('http');
const os = require('os');
const express = require('express');
const QRCode = require('qrcode');
const { WebSocketServer } = require('ws');

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
app.use(express.static(path.join(__dirname, 'public')));
app.get('/listen', (req, res) => res.sendFile(path.join(__dirname, 'public', 'listen.html')));
app.get('/nosleep.js', (req, res) => res.sendFile(require.resolve('nosleep.js')));
app.get('/info', (req, res) => res.json({ port: PORT, addresses: lanAddresses() }));

// QR-код LAN-адреса слушателя — генерируется на сервере, без внешних библиотек.
app.get('/qr', async (req, res) => {
  const ips = lanAddresses();
  if (!ips.length) return res.status(404).end();
  const url = `http://${ips[0]}:${PORT}/listen`;
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
let broadcaster = null;        // ws of the PC that captures system audio
let formatMsg = null;          // JSON string describing the PCM format (sampleRate, channels)
const listeners = new Set();   // ws of phones / browsers that listen

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
      }
    });
    return;
  }

  // Default role: listener.
  listeners.add(ws);
  console.log(`[listener] connected (${listeners.size} total)`);
  broadcastJSON(ws, { type: broadcaster ? 'live' : 'offline' });
  // Send the cached format so the listener can set up its audio decoder.
  if (broadcaster && formatMsg) ws.send(formatMsg);
  updateListenerCount();

  ws.on('close', () => {
    listeners.delete(ws);
    console.log(`[listener] disconnected (${listeners.size} total)`);
    updateListenerCount();
  });
});

server.listen(PORT, () => {
  console.log(`\n  sound-stream running`);
  console.log(`  PC (broadcast): http://localhost:${PORT}/`);
  const ips = lanAddresses();
  if (ips.length) {
    console.log(`  Phone (listen), та же WiFi:`);
    for (const ip of ips) console.log(`      http://${ip}:${PORT}/listen`);
  } else {
    console.log(`  Phone (listen): http://localhost:${PORT}/listen (LAN-адрес не найден)`);
  }
  console.log('');
});
