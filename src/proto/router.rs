use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub struct Route {
    pub tcp: String, // "ip:port"
    pub udp: String, // "ip:port"
}

#[derive(Clone)]
pub struct Router {
    routes: Arc<Mutex<HashMap<String, Route>>>,
    // таблица соответствий клиент -> upstream udp (например 1.2.3.4:54321 -> 185.24.55.33:24454)
    client_udp_map: Arc<Mutex<HashMap<SocketAddr, SocketAddr>>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(HashMap::new())),
            client_udp_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Добавить/обновить маршрут с указанием tcp и udp адресов
    pub fn add_route(&self, name: String, tcp_addr: String, udp_addr: String) {
        let mut guard = self.routes.lock().unwrap();
        guard.insert(name, Route { tcp: tcp_addr, udp: udp_addr });
    }

    pub fn remove_route(&self, name: &str) -> Option<Route> {
        let mut guard = self.routes.lock().unwrap();
        guard.remove(name)
    }

    pub fn lookup_route(&self, server_name: &str) -> Option<Route> {
        let guard = self.routes.lock().unwrap();
        guard.get(server_name).cloned()
    }

    pub fn all_routes(&self) -> HashMap<String, Route> {
        let guard = self.routes.lock().unwrap();
        guard.clone()
    }

    /// Регистрация соответствия клиент -> upstream udp
    pub fn register_udp_mapping(&self, client: SocketAddr, upstream: SocketAddr) {
        let mut guard = self.client_udp_map.lock().unwrap();
        guard.insert(client, upstream);
    }

    /// Удалить соответствие клиента
    pub fn unregister_udp_mapping(&self, client: &SocketAddr) {
        let mut guard = self.client_udp_map.lock().unwrap();
        guard.remove(client);
    }

    /// Получить upstream для клиента
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
}
