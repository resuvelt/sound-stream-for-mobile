// Захват системного звука в аудио-потоке (без глитчей главного потока).
// Накопает моно-кадры по FRAME сэмплов и отправляет их Int16 в главный поток.
class CaptureProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.FRAME = 1024;
    this.buf = new Float32Array(this.FRAME);
    this.idx = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) return true;
    const ch0 = input[0];
    if (!ch0) return true;
    const ch1 = input.length > 1 ? input[1] : ch0;

    for (let i = 0; i < ch0.length; i++) {
      this.buf[this.idx++] = (ch0[i] + ch1[i]) * 0.5; // даунмикс в моно
      if (this.idx === this.FRAME) {
        this.flush();
        this.idx = 0;
      }
    }
    return true; // держим процессор живым
  }

  flush() {
    const n = this.FRAME;
    const pcm = new Int16Array(n);
    let peak = 0;
    for (let i = 0; i < n; i++) {
      let s = this.buf[i];
      if (s > 1) s = 1; else if (s < -1) s = -1;
      if (s > peak) peak = s; else if (-s > peak) peak = -s;
      pcm[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
    // Передаём буфер без копирования (transferable).
    this.port.postMessage({ pcm: pcm.buffer, peak }, [pcm.buffer]);
  }
}

registerProcessor('capture-processor', CaptureProcessor);
