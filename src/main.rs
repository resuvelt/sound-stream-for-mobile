//! sound-stream — системный звук ПК на телефон прямо в браузере.
//!
//! Одно приложение без окна консоли: значок в трее, HTTP + WebSocket сервер
//! со вшитой страницей плеера, захват WASAPI loopback и постоянный адрес
//! `<имя>.local` по mDNS.

// Релизная сборка — GUI-подсистема: никакого окна консоли. В отладочной
// консоль остаётся, чтобы видеть журнал прямо в терминале.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod log;
mod assets;
mod autostart;
mod capture;
mod instance;
mod mdns;
mod net;
mod server;
mod state;
mod tray;
mod wide;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Config {
    pub port: u16,
    /// Постоянное mDNS-имя: http://<mdns_name>.local:PORT/ вместо адреса по IP.
    pub mdns_name: String,
}

impl Config {
    fn from_env() -> Self {
        let port = std::env::var("PORT")
            .ok()
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(47823);
        let mdns_name = std::env::var("MDNS_NAME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "sound-stream".into());
        Config { port, mdns_name }
    }

    pub fn mdns_host(&self) -> String {
        format!("{}.local", self.mdns_name)
    }

    pub fn mdns_url(&self) -> String {
        format!("http://{}:{}/", self.mdns_host(), self.port)
    }

    pub fn ip_url(&self, ip: Ipv4Addr) -> String {
        format!("http://{}:{}/", ip, self.port)
    }

    pub fn local_url(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }
}

fn main() {
    // `--restart` передаёт сам себе пункт меню «Перезапустить»: новый процесс
    // дожидается, пока старый освободит блокировку, вместо того чтобы выйти.
    let restarted = std::env::args().any(|a| a == "--restart");
    let Some(_instance) = instance::acquire(restarted) else {
        // Уже запущено — просим работающий экземпляр показать подсказку.
        tray::poke_running_instance();
        return;
    };

    let cfg = Config::from_env();
    log::init();
    log!(
        "=== sound-stream {} — порт {} ===",
        env!("CARGO_PKG_VERSION"),
        cfg.port
    );

    let migrated = autostart::migrate_legacy();
    autostart::refresh_path();

    let (app, capture_rx) = state::App::new(cfg, restarted, migrated);
    let app = Arc::new(app);
    let capture = capture::spawn(app.clone(), capture_rx);
    server::spawn(app.clone());
    mdns::spawn(app.clone());

    let exit = tray::run(app.clone());
    log!("выход: {:?}", exit);

    // Аккуратно гасим захват (не дольше пары секунд — выход не должен зависать).
    app.shutdown_capture();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !capture.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    if exit == tray::Exit::Restart {
        instance::spawn_restart();
    }
    std::process::exit(0);
}
