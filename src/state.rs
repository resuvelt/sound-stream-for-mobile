//! Общее состояние: слушатели, захват, статус сервера. Связывает поток захвата,
//! WebSocket-сервер и интерфейс (окно, трей).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};

use axum::body::Bytes;
use tokio::sync::broadcast;

use crate::Config;

/// Сообщения слушателям (кроме самих PCM-кадров).
#[derive(Clone, Debug)]
pub enum Event {
    /// Захват идёт: формат моно Int16 с такой частотой.
    Live { sample_rate: u32 },
    /// Захват пропал (устройство отвалилось) — ждём восстановления.
    Offline,
}

/// Команды потоку захвата.
pub enum CaptureCmd {
    /// Число слушателей изменилось — пересмотреть, нужен ли захват.
    Wake,
    Shutdown,
}

#[derive(Clone)]
pub struct Source {
    pub device: String,
    pub sample_rate: u32,
}

pub struct App {
    pub cfg: Config,
    /// Запущен пунктом «Перезапустить».
    pub restarted: bool,
    /// При старте старый ярлык автозапуска (Node + PowerShell) заменён на exe.
    pub migrated: bool,
    /// При старте открывается окно (запуск вручную, а не из автозапуска).
    pub show_window: bool,

    /// PCM-кадры (моно Int16 LE, ~10 мс) для всех слушателей.
    pub audio: broadcast::Sender<Bytes>,
    pub events: broadcast::Sender<Event>,

    listeners: AtomicUsize,
    source: Mutex<Option<Source>>,
    capture: Mutex<mpsc::Sender<CaptureCmd>>,

    pub server_up: AtomicBool,
    pub port_busy: AtomicBool,
}

impl App {
    pub fn new(
        cfg: Config,
        restarted: bool,
        migrated: bool,
        show_window: bool,
    ) -> (Self, mpsc::Receiver<CaptureCmd>) {
        // ~0,3 с аудио на слушателя: кто отстал сильнее (медленный Wi-Fi) —
        // пропускает кадры, а не копит задержку.
        let (audio, _) = broadcast::channel(32);
        let (events, _) = broadcast::channel(16);
        let (tx, rx) = mpsc::channel();
        let app = App {
            cfg,
            restarted,
            migrated,
            show_window,
            audio,
            events,
            listeners: AtomicUsize::new(0),
            source: Mutex::new(None),
            capture: Mutex::new(tx),
            server_up: AtomicBool::new(false),
            port_busy: AtomicBool::new(false),
        };
        (app, rx)
    }

    pub fn listeners(&self) -> usize {
        self.listeners.load(Ordering::SeqCst)
    }

    pub fn listener_joined(&self) -> usize {
        let n = self.listeners.fetch_add(1, Ordering::SeqCst) + 1;
        self.send_capture(CaptureCmd::Wake);
        n
    }

    pub fn listener_left(&self) -> usize {
        let n = self.listeners.fetch_sub(1, Ordering::SeqCst) - 1;
        self.send_capture(CaptureCmd::Wake);
        n
    }

    pub fn shutdown_capture(&self) {
        self.send_capture(CaptureCmd::Shutdown);
    }

    fn send_capture(&self, cmd: CaptureCmd) {
        if let Ok(tx) = self.capture.lock() {
            let _ = tx.send(cmd);
        }
    }

    /// Текущий источник, если захват идёт.
    pub fn source(&self) -> Option<Source> {
        self.source.lock().ok().and_then(|s| s.clone())
    }

    /// Захват поднят (или переключился на другое устройство) — всем live + формат.
    pub fn go_live(&self, src: Source) {
        let sample_rate = src.sample_rate;
        if let Ok(mut s) = self.source.lock() {
            *s = Some(src);
        }
        let _ = self.events.send(Event::Live { sample_rate });
    }

    /// Захват остановлен. `notify` — сообщить слушателям (если они ещё есть).
    pub fn go_offline(&self, notify: bool) {
        if let Ok(mut s) = self.source.lock() {
            *s = None;
        }
        if notify {
            let _ = self.events.send(Event::Offline);
        }
    }

    pub fn publish(&self, frame: Bytes) {
        let _ = self.audio.send(frame);
    }
}
