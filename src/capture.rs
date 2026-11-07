//! Захват системного звука через WASAPI loopback: открываем устройство ВЫВОДА
//! по умолчанию «на вход» — Windows отдаёт всё, что звучит из колонок.
//! Кадры даунмиксятся в моно Int16 по ~10 мс — формат, который понимает
//! страница слушателя.
//!
//! Захват «по требованию»: поднимается при первом слушателе и гаснет, когда
//! слушателей не осталось. Если сменилось устройство вывода по умолчанию
//! (подключили наушники) — переключается на него сам.

use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use windows::core::PWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::state::{App, CaptureCmd, Source};

/// Как часто проверять, не сменилось ли устройство вывода по умолчанию.
const DEVICE_CHECK: Duration = Duration::from_secs(1);
/// Пауза перед новой попыткой, если открыть захват не удалось.
const RETRY: Duration = Duration::from_secs(2);

pub fn spawn(app: Arc<App>, rx: mpsc::Receiver<CaptureCmd>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || run(&app, &rx))
        .expect("capture thread")
}

fn run(app: &App, rx: &mpsc::Receiver<CaptureCmd>) {
    // SAFETY: COM-вызовы ниже идут только из этого потока.
    let enumerator: Option<IMMDeviceEnumerator> = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => Some(e),
            Err(e) => {
                log!("[capture] WASAPI недоступен — звука не будет: {e}");
                None
            }
        }
    };

    let mut cap: Option<Loopback> = None;
    let mut retry_at = Instant::now();
    let mut next_device_check = Instant::now();
    let mut failing = false;

    loop {
        // Пока захват идёт, «спим» на событии WASAPI (в pump), а команды
        // только подбираем. Без захвата — спим на канале до пробуждения.
        let cmd = if cap.is_some() {
            match rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(CaptureCmd::Shutdown),
            }
        } else {
            let timeout = if app.listeners() > 0 {
                retry_at
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(20))
            } else {
                Duration::from_secs(3600)
            };
            match rx.recv_timeout(timeout) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(CaptureCmd::Shutdown),
            }
        };
        if matches!(cmd, Some(CaptureCmd::Shutdown)) {
            break;
        }
        let Some(enumerator) = enumerator.as_ref() else {
            continue;
        };

        let want = app.listeners() > 0;

        if !want {
            if cap.take().is_some() {
                app.go_offline(false);
                log!("[capture] loopback остановлен (нет слушателей)");
            }
            continue;
        }

        if cap.is_none() {
            if Instant::now() < retry_at {
                continue;
            }
            match Loopback::open(enumerator) {
                Ok(lb) => {
                    log!(
                        "[capture] loopback запущен: {} @ {} Hz, {} кан.",
                        lb.name,
                        lb.rate,
                        lb.channels
                    );
                    app.go_live(Source {
                        device: lb.name.clone(),
                        sample_rate: lb.rate,
                    });
                    failing = false;
                    next_device_check = Instant::now() + DEVICE_CHECK;
                    cap = Some(lb);
                }
                Err(e) => {
                    if !failing {
                        log!("[capture] не удалось открыть loopback: {e} — повторяю");
                        failing = true;
                    }
                    if app.source().is_some() {
                        app.go_offline(true);
                    }
                    retry_at = Instant::now() + RETRY;
                }
            }
            continue;
        }

        let lb = cap.as_mut().expect("checked above");
        if let Err(e) = lb.pump(app) {
            log!("[capture] захват прерван: {e}");
            cap = None;
            app.go_offline(true);
            retry_at = Instant::now() + Duration::from_millis(500);
            continue;
        }
        if Instant::now() >= next_device_check {
            next_device_check = Instant::now() + DEVICE_CHECK;
            let current = default_device_id(enumerator);
            if current.as_deref() != Some(lb.id.as_str()) {
                // Старый loopback продолжил бы «слушать» уже молчащее устройство.
                log!("[capture] устройство вывода по умолчанию сменилось — переключаюсь");
                cap = None;
                retry_at = Instant::now();
            }
        }
    }
    drop(cap);
}

#[derive(Clone, Copy, Debug)]
enum SampleFmt {
    F32,
    I16,
    I24,
    I32,
}

struct Loopback {
    id: String,
    name: String,
    rate: u32,
    channels: usize,
    center: Option<usize>,
    block_align: usize,
    fmt: SampleFmt,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
    /// Накопленный моно-звук до следующего 10-мс кадра.
    pending: Vec<i16>,
    frame_len: usize,
}

impl Loopback {
    fn open(enumerator: &IMMDeviceEnumerator) -> Result<Self, String> {
        // SAFETY: обычная последовательность WASAPI; указатель на формат
        // освобождается CoTaskMemFree сразу после Initialize.
        unsafe {
            let dev = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("нет устройства вывода по умолчанию ({e})"))?;
            let id = take_pwstr(dev.GetId().map_err(|e| e.to_string())?);
            let name = friendly_name(&dev).unwrap_or_else(|| "устройство вывода".into());
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;

            let wf = client.GetMixFormat().map_err(|e| e.to_string())?;
            let parsed = parse_format(wf);
            let init = parsed
                .ok_or_else(|| "неподдерживаемый формат устройства".to_string())
                .and_then(|_| {
                    client
                        .Initialize(
                            AUDCLNT_SHAREMODE_SHARED,
                            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                            200_000, // буфер 20 мс (в единицах по 100 нс)
                            0,
                            wf,
                            None,
                        )
                        .map_err(|e| e.to_string())
                });
            CoTaskMemFree(Some(wf as *const _));
            init?;
            let (fmt, rate, channels, block_align, center) = parsed.expect("checked above");

            let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
            let started = (|| {
                client.SetEventHandle(event)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                client.Start()?;
                Ok::<_, windows::core::Error>(capture)
            })();
            let capture = match started {
                Ok(c) => c,
                Err(e) => {
                    let _ = CloseHandle(event);
                    return Err(e.to_string());
                }
            };

            let frame_len = (rate / 100).max(1) as usize; // ~10 мс на кадр
            Ok(Loopback {
                id,
                name,
                rate,
                channels,
                center,
                block_align,
                fmt,
                client,
                capture,
                event,
                pending: Vec::with_capacity(frame_len * 4),
                frame_len,
            })
        }
    }

    /// Ждёт данных до 100 мс, забирает всё накопленное и рассылает кадры.
    fn pump(&mut self, app: &App) -> windows::core::Result<()> {
        // SAFETY: буфер от GetBuffer читается только до ReleaseBuffer, его
        // размер — frames * block_align байт.
        unsafe {
            WaitForSingleObject(self.event, 100);
            loop {
                if self.capture.GetNextPacketSize()? == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    // «Тишина»: содержимое буфера надо игнорировать.
                    self.pending.resize(self.pending.len() + n, 0);
                } else {
                    let bytes = std::slice::from_raw_parts(data, n * self.block_align);
                    self.downmix(bytes);
                }
                self.capture.ReleaseBuffer(frames)?;
                self.flush(app);
            }
        }
        Ok(())
    }

    /// Даунмикс в моно: (L + R) / 2, плюс центральный канал, если он есть
    /// (в 5.1/7.1 там диалоги).
    fn downmix(&mut self, bytes: &[u8]) {
        let fmt = self.fmt;
        let width = self.block_align / self.channels;
        let stereo = self.channels >= 2;
        let center = self.center;
        for frame in bytes.chunks_exact(self.block_align) {
            let s = |i: usize| sample(fmt, &frame[i * width..]);
            let mut v = if stereo { (s(0) + s(1)) * 0.5 } else { s(0) };
            if let Some(c) = center {
                v += s(c) * 0.707;
            }
            self.pending
                .push((v * 32767.0).round().clamp(-32768.0, 32767.0) as i16);
        }
    }

    fn flush(&mut self, app: &App) {
        while self.pending.len() >= self.frame_len {
            let mut out = Vec::with_capacity(self.frame_len * 2);
            for s in self.pending.drain(..self.frame_len) {
                out.extend_from_slice(&s.to_le_bytes());
            }
            app.publish(Bytes::from(out));
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        // SAFETY: client и event принадлежат только этому захвату.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

fn sample(fmt: SampleFmt, b: &[u8]) -> f32 {
    match fmt {
        SampleFmt::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        SampleFmt::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        SampleFmt::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        SampleFmt::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
    }
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SPEAKER_FRONT_LEFT_RIGHT: u32 = 0x3;
const SPEAKER_FRONT_CENTER: u32 = 0x4;

/// (формат сэмпла, частота, каналы, байт на кадр, индекс центрального канала)
type Parsed = (SampleFmt, u32, usize, usize, Option<usize>);

/// SAFETY: `wf` — указатель от GetMixFormat.
unsafe fn parse_format(wf: *const WAVEFORMATEX) -> Option<Parsed> {
    let f = std::ptr::read_unaligned(wf);
    let channels = f.nChannels as usize;
    let block_align = f.nBlockAlign as usize;
    if channels == 0 || block_align == 0 || block_align % channels != 0 {
        return None;
    }
    let (tag, center) = if f.wFormatTag == WAVE_FORMAT_EXTENSIBLE && f.cbSize >= 22 {
        let x = std::ptr::read_unaligned(wf as *const WAVEFORMATEXTENSIBLE);
        let mask = x.dwChannelMask;
        let center = (channels >= 3 && mask & SPEAKER_FRONT_CENTER != 0)
            .then(|| (mask & SPEAKER_FRONT_LEFT_RIGHT).count_ones() as usize);
        // Подтип: {00000001-...} — PCM, {00000003-...} — float.
        (x.SubFormat.data1 as u16, center)
    } else {
        (f.wFormatTag, None)
    };
    let fmt = match (tag, block_align / channels) {
        (WAVE_FORMAT_IEEE_FLOAT, 4) => SampleFmt::F32,
        (WAVE_FORMAT_PCM, 2) => SampleFmt::I16,
        (WAVE_FORMAT_PCM, 3) => SampleFmt::I24,
        (WAVE_FORMAT_PCM, 4) => SampleFmt::I32,
        _ => return None,
    };
    Some((fmt, f.nSamplesPerSec, channels, block_align, center))
}

/// SAFETY: `dev` — живой IMMDevice.
unsafe fn friendly_name(dev: &IMMDevice) -> Option<String> {
    let store = dev.OpenPropertyStore(STGM_READ).ok()?;
    let value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
    let name = value.to_string();
    (!name.is_empty()).then_some(name)
}

fn default_device_id(enumerator: &IMMDeviceEnumerator) -> Option<String> {
    // SAFETY: обычные COM-вызовы, строка ID освобождается в take_pwstr.
    unsafe {
        let dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        Some(take_pwstr(dev.GetId().ok()?))
    }
}

/// SAFETY: `p` выделен COM (CoTaskMemAlloc) и больше нигде не используется.
unsafe fn take_pwstr(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}
