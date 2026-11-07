//! Локальные (LAN) IPv4-адреса этого ПК — без виртуальных адаптеров
//! (Hyper-V, WSL, VPN, VirtualBox…), чтобы телефону не достался нерабочий адрес.

use std::net::Ipv4Addr;

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

use crate::wide::from_ptr;

/// Имена/описания адаптеров, которые телефону точно не подходят.
const SKIP: &[&str] = &[
    "vethernet",
    "wsl",
    "hyper-v",
    "default switch",
    "outline",
    "virtualbox",
    "vmware",
    "loopback",
    "docker",
    "veth",
    "vpn",
    "tap",
    "wintun",
    "wireguard",
    "tailscale",
    "zerotier",
    "bluetooth",
];

pub fn lan_ipv4() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    // SAFETY: буфер выровнен (u64) и имеет размер, который вернула сама
    // GetAdaptersAddresses; список читается только пока буфер жив.
    unsafe {
        let mut size: u32 = 16 * 1024;
        let mut buf: Vec<u64>;
        loop {
            buf = vec![0u64; (size as usize).div_ceil(8)];
            let r = GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                std::ptr::null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            );
            if r == ERROR_BUFFER_OVERFLOW {
                continue;
            }
            if r != ERROR_SUCCESS {
                return out;
            }
            break;
        }

        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            p = a.Next;
            if a.OperStatus != IfOperStatusUp || a.IfType == IF_TYPE_SOFTWARE_LOOPBACK {
                continue;
            }
            let label =
                format!("{} {}", from_ptr(a.FriendlyName), from_ptr(a.Description)).to_lowercase();
            if SKIP.iter().any(|s| label.contains(s)) {
                continue;
            }
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                let sa = (*u).Address.lpSockaddr;
                if !sa.is_null() && (*sa).sa_family == AF_INET {
                    let sin = &*(sa as *const SOCKADDR_IN);
                    let ip = Ipv4Addr::from(sin.sin_addr.S_un.S_addr.to_ne_bytes());
                    if !ip.is_loopback() && !ip.is_link_local() && !out.contains(&ip) {
                        out.push(ip);
                    }
                }
                u = (*u).Next;
            }
        }
    }
    // Домашние сети обычно 192.168.x.x — их вперёд.
    out.sort_by_key(|ip| match ip.octets() {
        [192, 168, ..] => 0,
        [10, ..] => 1,
        _ => 2,
    });
    out
}
