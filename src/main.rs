use tokio::net::{TcpListener, UdpSocket};
use std::{sync::Arc, io::Result};
use crate::proto::{Router, TcpProxy, UdpProxy};
use crate::configure::load_and_sync;

mod configure;
mod consts;
mod proto;

const CONFIG_PATH: &str = "./proxy.json";

#[tokio::main]
async fn main() -> Result<()> {
    let router = Arc::new(Router::new());

    // Загружаем конфиг и получаем порты
    let tcp_port: u16 = match load_and_sync(&router, CONFIG_PATH) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Couldn't get the port: {}", e);
            std::process::exit(1);
        }
    };

    println!("\x1b[1;32m{}\x1b[0m", "Запуск прокси");

    let udp_socket = UdpSocket::bind(format!("0.0.0.0:{}", 24454)).await?;
    println!("UDP proxy listening on 0.0.0.0:{}", 24454);
    
    // Запускаем UDP прокси в фоне, передав владение сокетом в задачу
    let udp_router = router.clone();
    tokio::spawn(async move {
        let mut proxy = UdpProxy::new(udp_socket, udp_router);
        if let Err(e) = proxy.run().await {
            eprintln!("UDP proxy error: {}", e);
        }
    });

    let listener = TcpListener::bind(format!("0.0.0.0:{}", tcp_port)).await?;
    println!("TCP proxy listening on 0.0.0.0:{}", tcp_port);

    loop {
        let (inbound, peer) = listener.accept().await?;
        let router = router.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::TcpProxy::new(inbound, router).run().await {
                eprintln!("{} соединение разорвано: {}", peer, e);
            }
        });
    }
}

// 172.31.123.61