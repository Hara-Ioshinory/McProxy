use std::{
    collections::{HashMap, HashSet},
    fs,
    net::{IpAddr, SocketAddr},
};
use regex::Regex;
use serde::Deserialize;

use crate::Router;

#[derive(Deserialize)]
struct Config {
    tcp_port: u16,
    endpoints: HashMap<String, HashMap<String, (u16, u16)>>,
}

fn valid_ip(ip_str: &str) -> Option<IpAddr> {
    match ip_str.parse::<IpAddr>() {
        Ok(ip) => {
            if ip.is_unspecified() { return None; }
            if ip.is_multicast() { return None; }
            if let IpAddr::V4(v4) = ip {
                if v4.octets() == [255,255,255,255] { return None; }
            }
            Some(ip)
        }
        Err(_) => None,
    }
}

pub fn load_and_sync(router: &Router, path: &str) -> Result<u16, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("Невозможно прочитать {}: {}", path, e))?;
    let cfg: Config = serde_json::from_str(&data).map_err(|e| format!("Синтаксическая ошибка: {}", e))?;

    // Регекс для проверки имени домена (латиница + цифры)
    let domain_re = Regex::new(r"^[A-Za-z0-9]+$").unwrap();

    // Наборы для проверки дубликатов
    let mut seen_dest_addrs: HashSet<SocketAddr> = HashSet::new();

    let mut desired: HashMap<String, (String, String)> = HashMap::new();

    println!("\x1b[1;32m{}\x1b[0m", "Валидация конфига");
    for (host, map) in cfg.endpoints {
        // валидируем IP один раз
        let ip = match valid_ip(&host) {
            Some(ip) => ip,
            None => {
                eprintln!("\x1b[33m{}\x1b[0m", format!("Пропущен узел: '{}' - некорректный IP", host));
                continue;
            }
        };

        for (domain, ports) in map {
            if !&domain_re.is_match(&domain) {
                eprintln!("\x1b[33m{}\x1b[0m", format!("Пропущен маршрут для {}: неверное имя поддомена '{}'", host, domain));
                continue;
            }

            let tcp_sock = SocketAddr::new(ip, ports.0);
            let udp_sock = SocketAddr::new(ip, ports.1);

            // Проверка дубликатов портов назначения (по SocketAddr)
            if seen_dest_addrs.contains(&tcp_sock) {
                eprintln!("\x1b[33m{}\x1b[0m", format!("Skipping {}:{} — tcp destination {} already used", host, domain, tcp_sock));
                continue;
            }
            if seen_dest_addrs.contains(&udp_sock) {
                eprintln!("\x1b[33m{}\x1b[0m", format!("Skipping {}:{} — udp destination {} already used", host, domain, udp_sock));
                continue;
            }

            // Всё ок — помечаем как использованные и добавляем
            seen_dest_addrs.insert(tcp_sock);
            seen_dest_addrs.insert(udp_sock);

            let tcp_addr = tcp_sock.to_string();
            let udp_addr = udp_sock.to_string();
            desired.insert(domain.clone(), (tcp_addr, udp_addr));
        }
    }

    println!("\x1b[1;32m{}\x1b[0m", "Добавление валидных маршрутов");
    for (name, (tcp_addr, udp_addr)) in &desired {
        router.add_route(name.clone(), tcp_addr.clone(), udp_addr.clone());
        println!("Добавлен домен '{}'\n> tcp:{}\n> udp:{}", name, tcp_addr, udp_addr);
    }

    Ok(cfg.tcp_port)
}
