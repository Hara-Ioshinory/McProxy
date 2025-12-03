use tokio::net::{TcpListener, UdpSocket};
use std::{sync::Arc, io::Result};
use crate::proto::{Router, ConnectionHandler, UdpProxy};
use crate::configure::load_and_sync;

mod configure;
mod consts;
mod proto;

const CONFIG_PATH: &str = "./proxy.json";

#[tokio::main]
async fn main() -> Result<()> {
    let router = Arc::new(Router::new());

    // Загружаем конфиг и получаем порты
    let (tcp_port, udp_port): (u16, u16) = match load_and_sync(&router, CONFIG_PATH) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Couldn't get the port: {}", e);
            std::process::exit(1);
        }
    };

    let listener = TcpListener::bind(format!("0.0.0.0:{}", tcp_port)).await?;
    println!("TCP proxy listening on 0.0.0.0:{}", tcp_port);

    let udp_socket = UdpSocket::bind(format!("0.0.0.0:{}", udp_port)).await?;
    println!("UDP proxy listening on 0.0.0.0:{}", udp_port);

    // Запускаем UDP прокси в фоне, передав владение сокетом в задачу
    let udp_router = router.clone();
    tokio::spawn(async move {
        let mut proxy = UdpProxy::new(udp_socket, udp_router);
        if let Err(e) = proxy.run().await {
            eprintln!("UDP proxy error: {}", e);
        }
    });

    loop {
        let (inbound, peer) = listener.accept().await?;
        let router = router.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::ConnectionHandler::new(inbound, router).run().await {
                eprintln!("Connection error from {}: {}", peer, e);
            }
        });
    }
}
