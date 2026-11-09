//! HTTP + WebSocket сервер: отдаёт страницу плеера и рассылает звук.
//!
//! Протокол со страницей прежний: текстовые JSON-сообщения
//! `{"type":"live"}`, `{"type":"offline"}`,
//! `{"type":"format","sampleRate":N,"channels":1}` и бинарные кадры моно Int16 LE.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::serve::ListenerExt;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::broadcast::error::RecvError;

use crate::state::{App, Event};
use crate::tray::{self, Kind};
use crate::{assets, net};

pub fn spawn(app: Arc<App>) {
    std::thread::Builder::new()
        .name("server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("server-rt")
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(serve(app));
        })
        .expect("server thread");
}

async fn serve(app: Arc<App>) {
    let port = app.cfg.port;

    // Порт может быть занят (например, ещё работает старая версия) — не падаем,
    // а ждём, пока освободится. Первые секунды пробуем часто: это нормальная
    // ситуация при «Перезапустить».
    let mut attempt = 0u32;
    let listener = loop {
        match bind(port) {
            Ok(l) => break l,
            Err(e) => {
                attempt += 1;
                if attempt == 1 {
                    log!("[server] порт {port} занят: {e} — жду, пока освободится");
                }
                if attempt == 10 {
                    app.port_busy.store(true, Ordering::SeqCst);
                    tray::notify(
                        format!("Порт {port} занят"),
                        "Его держит другая программа — возможно, старая версия sound-stream. \
                         Закрой её, и сервер запустится сам.",
                        Kind::Warning,
                    );
                }
                let pause = if attempt < 10 { 300 } else { 2000 };
                tokio::time::sleep(Duration::from_millis(pause)).await;
            }
        }
    };
    app.port_busy.store(false, Ordering::SeqCst);
    app.server_up.store(true, Ordering::SeqCst);
    announce(&app);

    let router = Router::new().fallback(handle).with_state(app.clone());
    // Без Nagle: кадры по 10 мс должны уходить сразу, а не копиться.
    let listener = listener.tap_io(|tcp| {
        let _ = tcp.set_nodelay(true);
    });
    if let Err(e) = axum::serve(listener, router).await {
        log!("[server] остановлен с ошибкой: {e}");
    }
    app.server_up.store(false, Ordering::SeqCst);
}

/// Слушаем и IPv4, и IPv6 (dual-stack), как и прежний сервер на Node.
fn bind(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let sock = match Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP)) {
        Ok(s) => {
            s.set_only_v6(false)?;
            s.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
            s
        }
        Err(_) => {
            let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
            s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
            s
        }
    };
    sock.listen(1024)?;
    sock.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(sock.into())
}

fn announce(app: &App) {
    let cfg = &app.cfg;
    log!("[server] слушаю порт {}", cfg.port);
    log!("  Постоянный адрес для телефона: {}", cfg.mdns_url());
    for ip in net::lan_ipv4() {
        log!("  По IP (запасной вариант): {}", cfg.ip_url(ip));
    }

    // Окно открыто и само показывает адрес — уведомление лишнее.
    if app.show_window && !app.migrated {
        return;
    }
    let title = if app.restarted {
        "sound-stream перезапущен"
    } else {
        "sound-stream запущен"
    };
    let mut text = format!("Постоянный адрес для телефона: {}", cfg.mdns_url());
    if app.migrated {
        text.push_str("\nАвтозапуск переведён на sound-stream.exe.");
    }
    tray::notify(title, text, Kind::Info);
}

async fn handle(
    State(app): State<Arc<App>>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    uri: Uri,
) -> Response {
    // WebSocket принимаем на любом пути (страница стучится в /?role=listener).
    if let Ok(ws) = ws {
        return ws.on_upgrade(move |socket| listener(socket, app));
    }
    match assets::find(uri.path()) {
        Some(a) => (
            [
                (header::CONTENT_TYPE, a.mime),
                (header::CACHE_CONTROL, a.cache),
            ],
            a.body,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

fn text(s: String) -> Message {
    Message::Text(s.into())
}

fn live() -> Message {
    text(r#"{"type":"live"}"#.into())
}

fn offline() -> Message {
    text(r#"{"type":"offline"}"#.into())
}

fn format_msg(sample_rate: u32) -> Message {
    text(format!(
        r#"{{"type":"format","sampleRate":{sample_rate},"channels":1}}"#
    ))
}

async fn listener(socket: WebSocket, app: Arc<App>) {
    // Подписываемся ДО того, как смотрим на текущее состояние, — иначе можно
    // пропустить «live», если захват поднимется между этими шагами.
    let mut events = app.events.subscribe();
    let mut audio = app.audio.subscribe();
    let n = app.listener_joined();
    log!("[listener] connected ({n} total)");

    let (mut tx, mut rx) = socket.split();

    // Захват уже идёт — сразу live и формат. Нет — offline; поток захвата
    // поднимется сам и разошлёт live + формат всем.
    let hello = match app.source() {
        Some(s) => vec![live(), format_msg(s.sample_rate)],
        None => vec![offline()],
    };
    let mut ok = true;
    for m in hello {
        if tx.send(m).await.is_err() {
            ok = false;
            break;
        }
    }

    while ok {
        tokio::select! {
            msg = rx.next() => match msg {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            },
            ev = events.recv() => match ev {
                Ok(Event::Live { sample_rate }) => {
                    ok = tx.send(live()).await.is_ok()
                        && tx.send(format_msg(sample_rate)).await.is_ok();
                }
                Ok(Event::Offline) => ok = tx.send(offline()).await.is_ok(),
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            frame = audio.recv() => match frame {
                Ok(b) => ok = tx.send(Message::Binary(b)).await.is_ok(),
                // Медленная сеть: старые кадры пропускаем, задержку не копим.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
        }
    }

    let n = app.listener_left();
    log!("[listener] disconnected ({n} total)");
}
