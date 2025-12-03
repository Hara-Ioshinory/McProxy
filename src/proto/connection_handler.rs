use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use bytes::{BytesMut, BufMut};
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

pub struct ConnectionHandler {
    inbound: TcpStream,
    router: Arc<Router>,
    rl: RateLimiter,
}

impl ConnectionHandler {
    pub fn new(inbound: TcpStream, router: Arc<Router>) -> Self {
        let rl = RateLimiter::new(DEFAULT_BYTES_PER_SEC, DEFAULT_BURST_BYTES);
        Self { inbound, router, rl }
    }

    pub async fn run(mut self) -> Result<()> {
        let _ = self.inbound.set_nodelay(true);

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
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "handshake timeout"));
                }
            };

        if full_packet.is_empty() {
            let _ = self.inbound.shutdown().await;
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "empty packet"));
        }

        let maybe_server_name = match maybe_server_name {
            Some(s) => s,
            None => {
                let _ = self.inbound.shutdown().await;
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "server name missing"));
            }
        };

        let server_name = match maybe_server_name.split('.').next() {
            Some(s) => s.to_string(),
            None => {
                let _ = self.inbound.shutdown().await;
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid server name"));
            }
        };
        println!("Server_name: {}", server_name);

        // Получаем Route (tcp и udp)
        let route = match self.router.lookup_route(&server_name) {
            Some(r) => r,
            None => {
                let _ = self.inbound.shutdown().await;
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "unknown server"));
            }
        };

        let upstream_tcp = route.tcp.clone();
        let upstream_udp = route.udp.clone();

        // Получаем peer_addr до into_split()
        let client_addr = match self.inbound.peer_addr() {
            Ok(a) => Some(a),
            Err(_) => None,
        };

        // Парсим upstream UDP адрес (если он валиден) и регистрируем mapping, если есть client_addr
        // Guard автоматически снимет mapping в Drop
        let mut _udp_guard: Option<UdpMappingGuard> = None;
        if let (Some(client), Ok(up_addr)) = (client_addr, upstream_udp.parse::<SocketAddr>()) {
            _udp_guard = Some(UdpMappingGuard::new(self.router.clone(), client, up_addr));
        }

        // Подключаемся к upstream по TCP
        let mut outbound = TcpStream::connect(upstream_tcp).await?;
        let _ = outbound.set_nodelay(true);

        if !self.rl.allow(full_packet.len()) {
            let _ = self.inbound.shutdown().await;
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "rate limit exceeded"));
        }

        outbound.write_all(&full_packet).await?;
        outbound.flush().await?;

        // Разделяем потоки и проксируем данные
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

        // Убедимся, что mapping будет снят (guard снимет в Drop при выходе из области видимости)
        drop(_udp_guard);

        // Пробрасываем ошибку, если была
        res.map(|_| ())
    }

    /// Read one full length-prefixed packet, parse handshake server name if present.
    async fn read_first_packet_and_parse_servername(&mut self) -> Result<(Vec<u8>, Option<String>)> {
        // Read VarInt length prefix in as few awaits as possible (up to 5 bytes)
        let mut len_prefix = BytesMut::with_capacity(5);
        let mut tmp = [0u8; 5];
        let mut read_total = 0usize;

        loop {
            // read at most remaining bytes of the varint prefix in one op
            let to_read = 5 - read_total;
            if to_read == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "length varint too big"));
            }
            let n = self.inbound.read(&mut tmp[read_total..read_total + to_read]).await?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "stream closed"));
            }
            read_total += n;

            // scan for varint end
            for i in 0..read_total {
                len_prefix.put_u8(tmp[i]);
                if tmp[i] & 0x80 == 0 {
                    // trailing bytes in tmp after varint belong to body
                    let trailing = read_total - (i + 1);
                    let mut body_extra = Vec::new();
                    if trailing > 0 {
                        body_extra.extend_from_slice(&tmp[i + 1..i + 1 + trailing]);
                    }

                    // parse length value from len_prefix
                    let mut tmp_slice: &[u8] = &len_prefix[..];
                    let length = VarInt::read_from_slice(&mut tmp_slice)? as usize;
                    if length > MAX_PACKET_LEN {
                        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "packet too large"));
                    }

                    // read remaining body
                    let mut body = Vec::with_capacity(length);
                    if !body_extra.is_empty() {
                        body.extend_from_slice(&body_extra);
                    }
                    if body.len() < length {
                        let mut to_read_body = length - body.len();
                        while to_read_body > 0 {
                            let chunk = std::cmp::min(to_read_body, 8 * 1024);
                            let mut buf = vec![0u8; chunk];
                            self.inbound.read_exact(&mut buf[..]).await?;
                            body.extend_from_slice(&buf);
                            to_read_body -= chunk;
                        }
                    }

                    // compose full packet (prefix + body) without extra copies where possible
                    let mut full = Vec::with_capacity(len_prefix.len() + body.len());
                    full.extend_from_slice(&len_prefix);
                    full.extend_from_slice(&body);

                    // attempt to parse server_name from handshake
                    let server_name = {
                        let mut body_slice: &[u8] = &body;
                        match VarInt::read_from_slice(&mut body_slice) {
                            Ok(packet_id) if packet_id == 0 => {
                                let _ = VarInt::read_from_slice(&mut body_slice).ok();
                                if let Ok(s) = read_varint_string_from_slice(&mut body_slice) {
                                    Some(s)
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    };

                    return Ok((full, server_name));
                }
            }
        }
    }
}

/// RAII guard для автоматического удаления UDP mapping при выходе из области видимости
struct UdpMappingGuard {
    router: Arc<Router>,
    client: Option<SocketAddr>,
}

impl UdpMappingGuard {
    fn new(router: Arc<Router>, client: SocketAddr, upstream: SocketAddr) -> Self {
        router.register_udp_mapping(client, upstream);
        Self { router, client: Some(client) }
    }
}

impl Drop for UdpMappingGuard {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            self.router.unregister_udp_mapping(&client);
        }
    }
}
