use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use bytes::BytesMut;
use std::sync::Arc;
use std::io::Result;
use std::net::SocketAddr;

use crate::proto::{Router, RateLimiter, VarInt, read_varint_string_from_slice};
use crate::consts::{
    DEFAULT_BYTES_PER_SEC,
    DEFAULT_BURST_BYTES,
    HANDSHAKE_READ_TIMEOUT,
    MAX_PACKET_LEN
};

pub struct TcpProxy {
    inbound: TcpStream,
    router: Arc<Router>,
    rl: RateLimiter,
}

impl TcpProxy {
    pub fn new(inbound: TcpStream, router: Arc<Router>) -> Self {
        let rl = RateLimiter::new(DEFAULT_BYTES_PER_SEC, DEFAULT_BURST_BYTES);
        Self { inbound, router, rl }
    }

    pub async fn run(mut self) -> Result<()> {
        let _ = self.inbound.set_nodelay(true);

        // Получаем peer_addr до дальнейших действий, чтобы корректно логировать попытку подключения
        let client_addr = match self.inbound.peer_addr() {
            Ok(a) => Some(a),
            Err(_) => None,
        };
        let client_str = client_addr
            .map(|a| a.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());

        // Лог: попытка подключения
        println!("{} запрашивает соединение", client_str);

        // Read first packet with timeout
        let (full_packet, maybe_server_name) =
            match timeout(HANDSHAKE_READ_TIMEOUT, self.read_first_packet_and_parse_servername()).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    let _ = self.inbound.shutdown().await;
                    return Err(e);
                }
                Err(_) => {
                    let _ = self.inbound.shutdown().await;
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Превышено время ожидания рукопожатия"));
                }
            };

        if full_packet.is_empty() {
            let _ = self.inbound.shutdown().await;
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Отправлен пустой пакет"));
        }

        let maybe_server_name = match maybe_server_name {
            Some(s) => s,
            None => {
                let _ = self.inbound.shutdown().await;
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Не удалось получить имя сервера"));
            }
        };

        let server_name = match maybe_server_name.split('.').next() {
            Some(s) => s.to_string(),
            None => {
                let _ = self.inbound.shutdown().await;
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Неверное имя сервера '{}'", maybe_server_name)));
            }
        };

        // Получаем Route (tcp и udp)
        let route = match self.router.lookup_route(&server_name) {
            Some(r) => r,
            None => {
                let _ = self.inbound.shutdown().await;
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("Неизхвестное имя сервера '{}'", maybe_server_name)));
            }
        };

        let upstream_tcp = route.tcp.clone();
        let upstream_udp = route.udp.clone();

        // Получаем peer_addr до into_split()
        let client_addr = match self.inbound.peer_addr() {
            Ok(a) => Some(a),
            Err(_) => None,
        };

        // Регистрируем IP->IP сопоставление (client_ip -> upstream_ip) для UDP
        if let (Some(client), Ok(up_addr)) = (client_addr, upstream_udp.parse::<SocketAddr>()) {
            let client_ip = client.ip();
            let upstream_ip = up_addr.ip();
            self.router.register_udp_ip_mapping(client_ip, upstream_ip);
            println!("REGISTER UDP IP mapping (from TCP handshake): {} -> {}", client_ip, upstream_ip);
        }

        // Подключаемся к upstream по TCP
        let mut outbound = TcpStream::connect(upstream_tcp).await?;
        let _ = outbound.set_nodelay(true);

        // Лог о подключении
        println!("{} установил соединение с {}", client_str, maybe_server_name);

        if !self.rl.allow(full_packet.len()) {
            let _ = self.inbound.shutdown().await;
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "rate limit exceeded"));
        }

        outbound.write_all(&full_packet).await?;
        outbound.flush().await?;

        // Разделяем потоки и проксируем данные (чистый TCP proxy)
        let (mut ri, mut wi) = self.inbound.into_split();
        let (mut ro, mut wo) = outbound.into_split();

        let c2s = async move {
            io::copy(&mut ri, &mut wo).await?;
            wo.shutdown().await
        };

        let s2c = async move {
            io::copy(&mut ro, &mut wi).await?;
            wi.shutdown().await
        };

        // Ждём завершения обеих задач
        let res = tokio::try_join!(c2s, s2c);

        // Пробрасываем ошибку, если была
        res.map(|_| ())
    }

    /// Read one full length-prefixed packet, parse handshake server name if present.
    async fn read_first_packet_and_parse_servername(&mut self) -> Result<(Vec<u8>, Option<String>)> {
        // Read up to 5 bytes of varint prefix, appending only newly read bytes
        let mut len_prefix = BytesMut::with_capacity(5);
        let mut tmp = [0u8; 5];
        let mut read_total = 0usize;

        loop {
            let to_read = 5 - read_total;
            if to_read == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "length varint too big"));
            }
            let n = self.inbound.read(&mut tmp[read_total..read_total + to_read]).await?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "stream closed"));
            }

            // append only newly read bytes
            len_prefix.extend_from_slice(&tmp[read_total..read_total + n]);
            read_total += n;

            // scan len_prefix for varint end
            let mut varint_end = None;
            for (i, &b) in len_prefix.iter().enumerate() {
                if b & 0x80 == 0 {
                    varint_end = Some(i);
                    break;
                }
            }

            if let Some(i) = varint_end {
                // trailing bytes after varint are part of body
                let trailing = len_prefix.len() - (i + 1);
                let mut body_extra = Vec::new();
                if trailing > 0 {
                    body_extra.extend_from_slice(&len_prefix[i + 1..]);
                }

                // parse length from prefix slice
                let mut tmp_slice: &[u8] = &len_prefix[..=i];
                let length = VarInt::read_from_slice(&mut tmp_slice)? as usize;
                if length > MAX_PACKET_LEN {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "packet too large"));
                }

                // read remaining body (use read_exact into BytesMut to avoid many allocations)
                let mut body = Vec::with_capacity(length);
                if !body_extra.is_empty() {
                    body.extend_from_slice(&body_extra);
                }
                while body.len() < length {
                    let mut chunk = vec![0u8; std::cmp::min(length - body.len(), 8 * 1024)];
                    self.inbound.read_exact(&mut chunk[..]).await?;
                    body.extend_from_slice(&chunk);
                }

                // compose full packet
                let mut full = Vec::with_capacity(i + 1 + body.len());
                full.extend_from_slice(&len_prefix[..=i]);
                full.extend_from_slice(&body);

                // parse server name from body
                let server_name = {
                    let mut body_slice: &[u8] = &body;
                    match VarInt::read_from_slice(&mut body_slice) {
                        Ok(packet_id) if packet_id == 0 => {
                            let _ = VarInt::read_from_slice(&mut body_slice).ok();
                            read_varint_string_from_slice(&mut body_slice).ok()
                        }
                        _ => None,
                    }
                };

                return Ok((full, server_name));
            }
        }
    }
}
