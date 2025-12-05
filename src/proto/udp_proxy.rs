use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use std::{io, sync::Arc, collections::HashMap};
use std::net::{SocketAddr, IpAddr};
use tokio::sync::Mutex as AsyncMutex;

use crate::proto::Router;

/// UDP proxy: broadcast по портам upstream IP, попытка отправить с исходного client_port,
/// создание точных mapping'ов при ответе сервера, аккуратная работа с pending.
pub struct UdpProxy {
    socket: UdpSocket,
    router: Arc<Router>,
    pending_clients: Arc<AsyncMutex<HashMap<IpAddr, Vec<SocketAddr>>>>,
    pending_ttl_secs: u64,
}

impl UdpProxy {
    pub fn new(socket: UdpSocket, router: Arc<Router>) -> Self {
        Self {
            socket,
            router,
            pending_clients: Arc::new(AsyncMutex::new(HashMap::new())),
            pending_ttl_secs: 10,
        }
    }

    pub async fn run(&mut self) -> io::Result<()> {
        let mut buf = vec![0u8; 65535];

        loop {
            let recv = self.socket.recv_from(&mut buf).await;
            let (len, src) = match recv {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("UDP recv_from error: {}", e);
                    continue;
                }
            };
            let data = &buf[..len];

            println!("Получен udp пакет от {} ({} байт)", src, len);

            // 1) Точный lookup по SocketAddr (IP+порт)
            if let Some(upstream) = self.router.lookup_udp_for_client(&src) {
                println!("  точное сопоставление: {} -> upstream {}", src, upstream);
                match self.socket.send_to(data, upstream).await {
                    Ok(sent) => println!("    отправлено {} байт на {}", sent, upstream),
                    Err(e) => {
                        eprintln!("    ошибка при отправке на {}: {}", upstream, e);
                        if e.kind() == std::io::ErrorKind::ConnectionReset {
                            self.router.unregister_udp_mapping(&src);
                            println!("    удалён mapping {} из-за ConnectionReset", src);
                        }
                    }
                }
                continue;
            }

            // 2) Возможно это ответ от upstream сервера — сначала проверяем clients_for_upstream
            let clients_for_up = self.router.clients_for_upstream(&src);
            if !clients_for_up.is_empty() {
                println!("  пакет от сервера {} пересылаю клиентам: {:?}", src, clients_for_up);
                // дедупликация
                let mut clients = clients_for_up;
                clients.sort_unstable();
                clients.dedup();
                for client in clients.iter() {
                    match self.socket.send_to(data, *client).await {
                        Ok(sent) => println!("    отправлено {} байт клиенту {}", sent, client),
                        Err(e) => {
                            eprintln!("    ошибка при отправке клиенту {}: {}", client, e);
                            if e.kind() == std::io::ErrorKind::ConnectionReset {
                                self.router.unregister_udp_mapping(client);
                                println!("    удалён mapping {} из-за ConnectionReset", client);
                            }
                        }
                    }
                }

                // Если были pending клиенты для этого upstream IP — создаём точные mapping'и
                let up_ip = src.ip();
                let mut pending = self.pending_clients.lock().await;
                if let Some(pending_list) = pending.remove(&up_ip) {
                    for client in pending_list {
                        self.router.register_udp_mapping(client, src);
                        println!("  CREATED mapping from pending: {} -> {}", client, src);
                    }
                }
                continue;
            }

            // 2b) Если clients_for_upstream пуст, но есть pending для этого upstream IP — это тоже ответ сервера
            {
                let up_ip = src.ip();
                let mut pending = self.pending_clients.lock().await;
                if let Some(pending_list) = pending.remove(&up_ip) {
                    println!("  ответ от upstream {} для pending клиентов: {:?}", src, pending_list);
                    // пересылаем ответ каждому pending клиенту и создаём точный mapping
                    for client in pending_list.iter() {
                        // регистрируем mapping client -> src
                        self.router.register_udp_mapping(*client, src);
                        println!("  CREATED mapping from pending: {} -> {}", client, src);

                        match self.socket.send_to(data, *client).await {
                            Ok(sent) => println!("    отправлено {} байт клиенту {}", sent, client),
                            Err(e) => {
                                eprintln!("    ошибка при отправке клиенту {}: {}", client, e);
                                if e.kind() == std::io::ErrorKind::ConnectionReset {
                                    self.router.unregister_udp_mapping(client);
                                    println!("    удалён mapping {} из-за ConnectionReset", client);
                                }
                            }
                        }
                    }
                    // уже обработали как ответ сервера
                    continue;
                }
            }

            // 3) Нет точного mapping и это не ответ от upstream — пробуем ip->ip mapping
            let client_ip = src.ip();
            if let Some(up_ip) = self.router.lookup_udp_ip_for_client(&client_ip) {
                println!("  найден IP->IP mapping: {} -> {}", client_ip, up_ip);

                // Получаем все upstream ip:port для up_ip (в конфиге может быть несколько серверов на одном IP)
                let upstream_addrs = self.router.upstream_addrs_for_ip(&up_ip);
                if upstream_addrs.is_empty() {
                    println!("  upstream_addrs_for_ip({}) пустой — нет портов для рассылки", up_ip);
                    continue;
                }

                // Broadcast: отправляем пакет на каждый порт upstream_addrs.
                // Попытаемся отправить с исходного client.port() — создаём временный сокет, привязанный к этому порту.
                let client_port = src.port();
                for up_addr in upstream_addrs.iter() {
                    let bind_addr = SocketAddr::new(std::net::IpAddr::from([0,0,0,0]), client_port);
                    match UdpSocket::bind(bind_addr).await {
                        Ok(temp_sock) => {
                            match temp_sock.send_to(data, *up_addr).await {
                                Ok(sent) => println!("    отправлено {} байт на {} (source port {})", sent, up_addr, client_port),
                                Err(e) => eprintln!("    ошибка при отправке через temp socket на {}: {}", up_addr, e),
                            }
                        }
                        Err(e) => {
                            eprintln!("    не удалось bind 0.0.0.0:{} ({}) — fallback на главный сокет", client_port, e);
                            match self.socket.send_to(data, *up_addr).await {
                                Ok(sent) => println!("    отправлено {} байт на {} (fallback)", sent, up_addr),
                                Err(e) => eprintln!("    ошибка при отправке на {}: {}", up_addr, e),
                            }
                        }
                    }
                }

                // Добавляем клиента в pending для этого upstream IP (чтобы при ответе сервера создать mapping)
                {
                    let mut pending = self.pending_clients.lock().await;
                    let entry = pending.entry(up_ip).or_insert_with(Vec::new);
                    if !entry.contains(&src) {
                        entry.push(src);
                        println!("  добавлен в pending_clients[{}]: {}", up_ip, src);

                        // spawn TTL task только если мы действительно добавили клиента
                        let pending_clone = self.pending_clients.clone();
                        let up_ip_clone = up_ip;
                        let client_clone = src;
                        let ttl = self.pending_ttl_secs;
                        tokio::spawn(async move {
                            sleep(Duration::from_secs(ttl)).await;
                            let mut pending = pending_clone.lock().await;
                            if let Some(vec) = pending.get_mut(&up_ip_clone) {
                                vec.retain(|&c| c != client_clone);
                                if vec.is_empty() {
                                    pending.remove(&up_ip_clone);
                                }
                                println!("  pending TTL expired: removed {} from pending[{}]", client_clone, up_ip_clone);
                            }
                        });
                    } else {
                        println!("  клиент {} уже в pending_clients[{}], TTL не перезапускается", src, up_ip);
                    }
                }

                continue;
            }

            // 4) Ничего не найдено — логируем
            println!("  нет сопоставления для {}", src);
        }
    }
}
