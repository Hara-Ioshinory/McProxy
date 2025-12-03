use std::{
    collections::HashMap, fs, net::{SocketAddr, TcpStream, ToSocketAddrs}, time::Duration
};
use serde::Deserialize;

use crate::Router;

#[derive(Deserialize)]
struct Config {
    tcp_port: u16,
    udp_port: u16,
    // endpoints: { "185.24.55.33": { "fractal": [54636, 24454] } }
    endpoints: HashMap<String, HashMap<String, (u16, u16)>>,
}

pub fn load_and_sync(router: &Router, path: &str) -> Result<(u16, u16), String> {
    let data = fs::read_to_string(path).map_err(|e| format!("can't read {}: {}", path, e))?;
    let cfg: Config = serde_json::from_str(&data).map_err(|e| format!("bad json: {}", e))?;

    let mut desired: HashMap<String, (String, String)> = HashMap::new();

    for (host, map) in cfg.endpoints {
        for (domain, ports) in map {
            let tcp_addr = format!("{}:{}", host, ports.0);
            let udp_addr = format!("{}:{}", host, ports.1);
            if port_is_open(&tcp_addr, Duration::from_secs(2)) {
                desired.insert(domain.clone(), (tcp_addr, udp_addr));
            } else {
                println!("Skipped (tcp port closed): {} -> {}", domain, tcp_addr);
            }
        }
    }

    let current = router.all_routes();

    for (name, (tcp_addr, udp_addr)) in &desired {
        match current.get(name) {
            None => {
                router.add_route(name.clone(), tcp_addr.clone(), udp_addr.clone());
                println!("Added route: {} -> tcp:{} udp:{}", name, tcp_addr, udp_addr);
            }
            Some(existing) if existing.tcp != *tcp_addr || existing.udp != *udp_addr => {
                router.add_route(name.clone(), tcp_addr.clone(), udp_addr.clone());
                println!("Updated route: {}: {} -> tcp:{} udp:{}", name, existing.tcp, tcp_addr, udp_addr);
            }
            Some(_) => {
                println!("Unchanged route: {} -> tcp:{} udp:{}", name, tcp_addr, udp_addr);
            }
        }
    }

    for (name, existing_route) in current {
        if !desired.contains_key(&name) {
            router.remove_route(&name);
            println!("Removed route: {} -> tcp:{} udp:{}", name, existing_route.tcp, existing_route.udp);
        }
    }

    Ok((cfg.tcp_port, cfg.udp_port))
}

fn port_is_open(addr: &str, timeout: Duration) -> bool {
    let sock: Option<SocketAddr> = addr.to_socket_addrs().ok().and_then(|mut it| it.next());
    if let Some(sock_addr) = sock {
        TcpStream::connect_timeout(&sock_addr, timeout).is_ok()
    } else {
        false
    }
}
