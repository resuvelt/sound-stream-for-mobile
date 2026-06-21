'use strict';
// Захват системного звука ПК через WASAPI loopback (нативно, без браузера).
// Открываем устройство ВЫВОДА по умолчанию на вход — RtAudio отдаёт всё, что
// звучит из колонок. Кадры даунмиксятся в моно Int16 — формат, который уже
// понимает страница слушателя.

let RtAudio, RtAudioFormat, RtAudioApi;
try {
  ({ RtAudio, RtAudioFormat, RtAudioApi } = require('audify'));
} catch (e) {
  // audify не установлен/не собрался — захват недоступен, но сервер работает
  // (остаётся браузерный источник). Печатаем подсказку один раз.
  console.warn('[capture] audify недоступен — системный захват выключен:', e.message);
}

class SystemAudioCapture {
  constructor() {
    this.rt = null;
    this.running = false;
    this.sampleRate = 48000;
    this.deviceName = '';
    this.onFrame = null; // (Buffer monoInt16) => void
  }

  get available() { return !!RtAudio; }

  start() {
    if (this.running || !RtAudio) return false;
    try {
      this.rt = new RtAudio(RtAudioApi.WINDOWS_WASAPI);
      const outId = this.rt.getDefaultOutputDevice();
      const dev = this.rt.getDevices().find(d => d.id === outId);
      if (!dev) { this.rt = null; return false; }

      this.sampleRate = dev.preferredSampleRate || 48000;
      this.deviceName = dev.name;
      const ch = dev.outputChannels || 2;
      const frame = Math.round(this.sampleRate / 100); // ~10 мс на кадр

      this.rt.openStream(
        null,                                                  // без вывода
        { deviceId: outId, nChannels: ch, firstChannel: 0 },   // вход = loopback вывода
        RtAudioFormat.RTAUDIO_SINT16,
        this.sampleRate,
        frame,
        'sound-stream',
        (pcm) => this._onPcm(pcm, ch),
        null
      );
      this.rt.start();
      this.running = true;
      return true;
    } catch (e) {
      console.error('[capture] не удалось открыть loopback:', e.message);
      try { this.rt && this.rt.closeStream(); } catch (_) {}
      this.rt = null;
      this.running = false;
      return false;
    }
  }

  _onPcm(pcm, ch) {
    if (!this.onFrame) return;
    const i16 = new Int16Array(pcm.buffer, pcm.byteOffset, pcm.length >> 1);
    const n = (i16.length / ch) | 0;
    const mono = new Int16Array(n);
    if (ch === 1) {
      mono.set(i16.subarray(0, n));
    } else {
      for (let i = 0, j = 0; i < n; i++, j += ch) {
        mono[i] = (i16[j] + i16[j + 1]) >> 1; // даунмикс L/R в моно
      }
    }
    this.onFrame(Buffer.from(mono.buffer, mono.byteOffset, mono.byteLength));
  }

  stop() {
    if (!this.running) return;
    try { this.rt.stop(); } catch (e) {}
    try { this.rt.closeStream(); } catch (e) {}
    this.rt = null;
    this.running = false;
  }
}

module.exports = SystemAudioCapture;
