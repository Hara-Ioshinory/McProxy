use tokio::net::UdpSocket;
use std::{io, sync::Arc};
use crate::proto::Router;

pub struct UdpProxy {
    socket: UdpSocket,
    router: Arc<Router>,
}

impl UdpProxy {
    pub fn new(socket: UdpSocket, router: Arc<Router>) -> Self {
        Self { socket, router }
    }

    pub async fn run(&mut self) -> io::Result<()> {
        let mut buf = vec![0u8; 65535];

        loop {
            let (len, src) = self.socket.recv_from(&mut buf).await?;
            let data = &buf[..len];

            if let Some(upstream) = self.router.lookup_udp_for_client(&src) {
                let _ = self.socket.send_to(data, upstream).await;
                continue;
            }

            let clients = self.router.clients_for_upstream(&src);
            if !clients.is_empty() {
                for client in clients {
                    let _ = self.socket.send_to(data, client).await;
                }
                continue;
            }
        }
    }
}
