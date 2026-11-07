//! Объявляем ПК в локальной сети по mDNS/Bonjour под постоянным именем
//! `<name>.local`. Телефон один раз добавляет http://<name>.local:PORT/ в
//! закладку — и адрес больше не зависит от смены IP по DHCP.
//!
//! Свой маленький ответчик, а не готовая библиотека: те объявляют A-записи по
//! ВСЕМ интерфейсам (включая Hyper-V/WSL/VPN/VirtualBox), и телефон может
//! выбрать нерабочий адрес. Здесь отвечаем только реальными LAN-адресами и,
//! привязавшись к интерфейсу основного из них, шлём ответ именно через него.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};

use crate::net::lan_ipv4;
use crate::state::App;

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;
const TYPE_A: u16 = 1;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
/// Бит cache-flush: устройства заменяют закэшированную запись, а не копят.
const CACHE_FLUSH: u16 = 0x8000;
const TTL: u32 = 120;
/// Раз в столько переанонсируемся и проверяем, не сменился ли IP.
const TICK: Duration = Duration::from_secs(30);

pub fn spawn(app: Arc<App>) {
    std::thread::Builder::new()
        .name("mdns".into())
        .spawn(move || run(&app))
        .expect("mdns thread");
}

fn run(app: &App) {
    let host = app.cfg.mdns_host();
    let mut bound: Option<Ipv4Addr> = lan_ipv4().first().copied();
    let mut sock = open(bound, &host);
    let mut next_tick = Instant::now() + TICK;
    let mut buf = [0u8; 9000];

    loop {
        match &sock {
            Some(s) => match s.recv_from(&mut buf) {
                Ok((n, src)) => on_packet(s, &buf[..n], src, &host),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                // Windows отдаёт в recv «connection reset» после ICMP на наш
                // unicast-ответ — это не повод что-то делать.
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            },
            None => std::thread::sleep(Duration::from_secs(1)),
        }

        if Instant::now() >= next_tick {
            next_tick = Instant::now() + TICK;
            // Следим за сменой IP (DHCP): при смене основного LAN-адреса
            // пересобираем сокет под новый интерфейс; иначе просто переанонс.
            let ip = lan_ipv4().first().copied();
            if ip != bound || sock.is_none() {
                if ip != bound {
                    log!(
                        "[mdns] LAN-адрес сменился ({} → {}) — перепривязка",
                        show(bound),
                        show(ip)
                    );
                }
                bound = ip;
                drop(sock.take()); // освободить порт до повторного bind
                sock = open(bound, &host);
            } else if let Some(s) = &sock {
                announce(s, &host);
            }
        }
    }
}

fn show(ip: Option<Ipv4Addr>) -> String {
    ip.map_or_else(|| "—".into(), |ip| ip.to_string())
}

fn open(iface: Option<Ipv4Addr>, host: &str) -> Option<UdpSocket> {
    match try_open(iface) {
        Ok(s) => {
            log!("[mdns] отвечаю на {host} (интерфейс {})", show(iface));
            announce(&s, host); // проактивный анонс — заранее прогреть кэши устройств
            Some(s)
        }
        Err(e) => {
            log!("[mdns] не удалось открыть сокет: {e} — адрес {host} пока не работает");
            None
        }
    }
}

fn try_open(iface: Option<Ipv4Addr>) -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // 5353 делим с mDNS самой Windows и другими ответчиками.
    s.set_reuse_address(true)?;
    // bind на 0.0.0.0 (так Windows принимает мультикаст), а членство в группе
    // и исходящий интерфейс — на нужном LAN-адаптере.
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    let ifc = iface.unwrap_or(Ipv4Addr::UNSPECIFIED);
    s.join_multicast_v4(&GROUP, &ifc)?;
    if let Some(ip) = iface {
        s.set_multicast_if_v4(&ip)?;
    }
    s.set_multicast_ttl_v4(255)?;
    s.set_multicast_loop_v4(true)?;
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    Ok(s.into())
}

fn announce(s: &UdpSocket, host: &str) {
    let ips = lan_ipv4();
    if !ips.is_empty() {
        let _ = s.send_to(&response(host, &ips, None), (GROUP, PORT));
    }
}

fn on_packet(s: &UdpSocket, pkt: &[u8], src: SocketAddr, host: &str) {
    let Some((id, questions)) = parse_query(pkt) else {
        return;
    };
    for (name, qtype) in questions {
        if (qtype == TYPE_A || qtype == TYPE_ANY) && name.eq_ignore_ascii_case(host) {
            let ips = lan_ipv4();
            if ips.is_empty() {
                return;
            }
            if src.port() == PORT {
                let _ = s.send_to(&response(host, &ips, None), (GROUP, PORT));
            } else {
                // «Простой» DNS-запрос не с порта 5353 (RFC 6762 §6.7): ответ
                // напрямую, с тем же ID и вопросом.
                let _ = s.send_to(&response(host, &ips, Some((id, qtype))), src);
            }
            return; // на пакет отвечаем один раз
        }
    }
}

/// Разбирает запрос: (ID, [(имя, тип)]). Ответы и не-запросы — None.
fn parse_query(p: &[u8]) -> Option<(u16, Vec<(String, u16)>)> {
    if p.len() < 12 {
        return None;
    }
    let be16 = |o: usize| -> Option<u16> { Some(u16::from_be_bytes([*p.get(o)?, *p.get(o + 1)?])) };
    let id = be16(0)?;
    let flags = be16(2)?;
    if flags & 0x8000 != 0 || (flags >> 11) & 0xF != 0 {
        return None; // это ответ или не стандартный запрос
    }
    let count = be16(4)?;
    let mut off = 12;
    let mut out = Vec::new();
    for _ in 0..count {
        let (name, next) = read_name(p, off)?;
        let qtype = be16(next)?;
        be16(next + 2)?; // класс (с битом unicast-response) — не важен
        off = next + 4;
        out.push((name, qtype));
    }
    Some((id, out))
}

/// Имя в DNS-формате (с поддержкой сжатия указателями) → ("a.b.local", смещение после имени).
fn read_name(p: &[u8], mut off: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *p.get(off)? as usize;
        match len & 0xC0 {
            0x00 if len == 0 => {
                off += 1;
                break;
            }
            0x00 => {
                let label = p.get(off + 1..off + 1 + len)?;
                labels.push(String::from_utf8_lossy(label).into_owned());
                off += 1 + len;
            }
            0xC0 => {
                let ptr = ((len & 0x3F) << 8) | *p.get(off + 1)? as usize;
                end.get_or_insert(off + 2);
                jumps += 1;
                if jumps > 16 {
                    return None;
                }
                off = ptr;
            }
            _ => return None,
        }
    }
    Some((labels.join("."), end.unwrap_or(off)))
}

fn put_name(v: &mut Vec<u8>, name: &str) {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        let b = &label.as_bytes()[..label.len().min(63)];
        v.push(b.len() as u8);
        v.extend_from_slice(b);
    }
    v.push(0);
}

/// Ответ с A-записями. `legacy` = Some((id, тип вопроса)) — для простого
/// DNS-клиента: с вопросом, без cache-flush и с коротким TTL.
fn response(host: &str, ips: &[Ipv4Addr], legacy: Option<(u16, u16)>) -> Vec<u8> {
    let mut v = Vec::with_capacity(64 + ips.len() * (host.len() + 16));
    let (id, questions) = legacy.map_or((0, 0u16), |(id, _)| (id, 1));
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&0x8400u16.to_be_bytes()); // ответ, authoritative
    v.extend_from_slice(&questions.to_be_bytes());
    v.extend_from_slice(&(ips.len() as u16).to_be_bytes());
    v.extend_from_slice(&[0, 0, 0, 0]);
    if let Some((_, qtype)) = legacy {
        put_name(&mut v, host);
        v.extend_from_slice(&qtype.to_be_bytes());
        v.extend_from_slice(&CLASS_IN.to_be_bytes());
    }
    let (class, ttl) = match legacy {
        Some(_) => (CLASS_IN, 10),
        None => (CLASS_IN | CACHE_FLUSH, TTL),
    };
    for ip in ips {
        put_name(&mut v, host);
        v.extend_from_slice(&TYPE_A.to_be_bytes());
        v.extend_from_slice(&class.to_be_bytes());
        v.extend_from_slice(&ttl.to_be_bytes());
        v.extend_from_slice(&4u16.to_be_bytes());
        v.extend_from_slice(&ip.octets());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut v = vec![0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        put_name(&mut v, name);
        v.extend_from_slice(&qtype.to_be_bytes());
        v.extend_from_slice(&(0x8000u16 | CLASS_IN).to_be_bytes());
        v
    }

    #[test]
    fn parses_query() {
        let (id, qs) = parse_query(&query("Sound-Stream.local", TYPE_A)).unwrap();
        assert_eq!(id, 0x1234);
        assert_eq!(qs, vec![("Sound-Stream.local".to_string(), TYPE_A)]);
    }

    #[test]
    fn follows_compression_pointers() {
        // Второй вопрос ссылается на «local» из первого.
        let mut v = vec![0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0];
        put_name(&mut v, "a.local"); // a@12, local@14
        v.extend_from_slice(&[0, 1, 0, 1]);
        v.extend_from_slice(&[1, b'b', 0xC0, 14, 0, 1, 0, 1]);
        let (_, qs) = parse_query(&v).unwrap();
        assert_eq!(qs[1].0, "b.local");
    }

    #[test]
    fn ignores_responses_and_garbage() {
        let mut r = query("x.local", TYPE_A);
        r[2] = 0x84;
        assert!(parse_query(&r).is_none());
        assert!(parse_query(&[0; 5]).is_none());
        // Петля указателей не вешает разбор.
        let lp = [0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0, 12, 0, 1, 0, 1];
        assert!(parse_query(&lp).is_none());
    }

    #[test]
    fn builds_answer() {
        let ips = [Ipv4Addr::new(192, 168, 1, 5)];
        let r = response("sound-stream.local", &ips, None);
        assert_eq!(&r[2..4], &[0x84, 0x00]);
        assert_eq!(&r[6..8], &[0, 1]); // одна A-запись
        assert_eq!(&r[r.len() - 4..], &[192, 168, 1, 5]);
        let (name, end) = read_name(&r, 12).unwrap();
        assert_eq!(name, "sound-stream.local");
        assert_eq!(&r[end..end + 4], &[0, 1, 0x80, 0x01]); // A, IN + cache-flush
    }
}
