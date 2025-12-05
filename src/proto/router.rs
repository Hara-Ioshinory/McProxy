use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::net::{SocketAddr, IpAddr};

#[derive(Clone, Debug)]
pub struct Route {
    pub tcp: String, // "ip:port"
    pub udp: String, // "ip:port"
}

#[derive(Clone)]
pub struct Router {
    routes: Arc<Mutex<HashMap<String, Route>>>,
    /// Точные сопоставления client SocketAddr -> upstream SocketAddr (IP+порт)
    client_udp_map: Arc<Mutex<HashMap<SocketAddr, SocketAddr>>>,
    /// Сопоставления по IP (без портов): client_ip -> upstream_ip
    client_upstream_ip_map: Arc<Mutex<HashMap<IpAddr, IpAddr>>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
            client_udp_map: Arc::new(Mutex::new(HashMap::new())),
            client_upstream_ip_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Добавить/обновить маршрут с указанием tcp и udp адресов
    pub fn add_route(&self, name: String, tcp_addr: String, udp_addr: String) {
        let mut guard = self.routes.lock().unwrap();
        guard.insert(name, Route { tcp: tcp_addr, udp: udp_addr });
    }

    pub fn lookup_route(&self, server_name: &str) -> Option<Route> {
        let guard = self.routes.lock().unwrap();
        guard.get(server_name).cloned()
    }

    /// Получить все upstream UDP адреса (ip:port) для заданного upstream IP
    pub fn upstream_addrs_for_ip(&self, ip: &IpAddr) -> Vec<SocketAddr> {
        let guard = self.routes.lock().unwrap();
        guard.values()
            .filter_map(|r| r.udp.parse::<SocketAddr>().ok())
            .filter(|sa| &sa.ip() == ip)
            .collect()
    }

    /// Регистрация соответствия client -> upstream udp (точное по SocketAddr)
    pub fn register_udp_mapping(&self, client: SocketAddr, upstream: SocketAddr) {
        let mut guard = self.client_udp_map.lock().unwrap();
        guard.insert(client, upstream);
        println!("REGISTER UDP mapping: {} -> {}", client, upstream);
    }

    /// Удалить соответствие клиента (точное по SocketAddr)
    pub fn unregister_udp_mapping(&self, client: &SocketAddr) {
        let mut guard = self.client_udp_map.lock().unwrap();
        if guard.remove(client).is_some() {
            println!("UNREGISTER UDP mapping: {}", client);
        }
    }

    /// Получить upstream для клиента (точное совпадение по SocketAddr)
    pub fn lookup_udp_for_client(&self, client: &SocketAddr) -> Option<SocketAddr> {
        let guard = self.client_udp_map.lock().unwrap();
        guard.get(client).cloned()
    }

    /// Получить всех клиентов, у которых upstream == server_addr
    pub fn clients_for_upstream(&self, server_addr: &SocketAddr) -> Vec<SocketAddr> {
        let guard = self.client_udp_map.lock().unwrap();
        guard.iter()
            .filter_map(|(client, up)| if up == server_addr { Some(*client) } else { None })
            .collect()
    }

    /// Регистрация сопоставления по IP (без портов): client_ip -> upstream_ip
    pub fn register_udp_ip_mapping(&self, client_ip: IpAddr, upstream_ip: IpAddr) {
        let mut guard = self.client_upstream_ip_map.lock().unwrap();
        guard.insert(client_ip, upstream_ip);
        println!("REGISTER UDP IP mapping: {} -> {}", client_ip, upstream_ip);
    }

    /// Получить upstream IP для client_ip (без портов)
    pub fn lookup_udp_ip_for_client(&self, client_ip: &IpAddr) -> Option<IpAddr> {
        let guard = self.client_upstream_ip_map.lock().unwrap();
        guard.get(client_ip).cloned()
    }
}
