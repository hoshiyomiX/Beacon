use crate::config::Config;

use std::pin::Pin;
use std::task::{Context, Poll};
use bytes::{BufMut, BytesMut};
use futures_util::Stream;
use pin_project_lite::pin_project;
use pretty_bytes::converter::convert;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use worker::*;

// Reduced buffer sizes for lower memory usage in Cloudflare Workers
static MAX_WEBSOCKET_SIZE: usize = 16 * 1024; // 16kb
static MAX_BUFFER_SIZE: usize = 128 * 1024; // 128kb

pin_project! {
    pub struct ProxyStream<'a> {
        pub config: Config,
        pub ws: &'a WebSocket,
        pub buffer: BytesMut,
        #[pin]
        pub events: EventStream<'a>,
        pub last_activity: std::time::Instant,
    }
}

impl<'a> ProxyStream<'a> {
    pub fn new(config: Config, ws: &'a WebSocket, events: EventStream<'a>) -> Self {
        let buffer = BytesMut::with_capacity(MAX_BUFFER_SIZE);

        Self {
            config,
            ws,
            buffer,
            events,
            last_activity: std::time::Instant::now(),
        }
    }
    
    /// Send a ping to keep the connection alive
    fn send_keepalive(&mut self) -> std::io::Result<()> {
        // Cloudflare Workers doesn't expose ping() directly, so we send a small control frame
        // by sending an empty binary message which acts as a heartbeat
        self.ws.send_with_bytes(&[])
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        self.last_activity = std::time::Instant::now();
        Ok(())
    }
    
    /// Check if we need to send a keepalive (every 30-45 seconds to stay under Cloudflare's limit)
    fn should_send_keepalive(&self) -> bool {
        self.last_activity.elapsed().as_secs() > 30
    }
    
    pub async fn fill_buffer_until(&mut self, n: usize) -> std::io::Result<()> {
        use futures_util::StreamExt;

        while self.buffer.len() < n {
            // Send keepalive if needed before waiting for data
            if self.should_send_keepalive() {
                self.send_keepalive()?;
            }

            match self.events.next().await {
                Some(Ok(WebsocketEvent::Message(msg))) => {
                    self.last_activity = std::time::Instant::now();
                    if let Some(data) = msg.bytes() {
                        // Ignore empty messages (our keepalives)
                        if data.len() > 0 {
                            self.buffer.put_slice(&data);
                        }
                    }
                }
                Some(Ok(WebsocketEvent::Close(_))) => {
                    console_log!("[WS] Connection closed by peer");
                    break;
                }
                Some(Err(e)) => {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));
                }
                None => {
                    console_log!("[WS] Event stream ended");
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn peek_buffer(&self, n: usize) -> &[u8] {
        let len = self.buffer.len().min(n);
        &self.buffer[..len]
    }

    pub async fn process(&mut self) -> Result<()> {
        let peek_buffer_len = 62;
        self.fill_buffer_until(peek_buffer_len).await?;
        let peeked_buffer = self.peek_buffer(peek_buffer_len);

        if peeked_buffer.len() < (peek_buffer_len/2) {
            return Err(Error::RustError("not enough buffer".to_string()));
        }

        if self.is_vless(peeked_buffer) {
            console_log!("vless detected!");
            self.process_vless().await
        } else if self.is_shadowsocks(peeked_buffer) {
            console_log!("shadowsocks detected!");
            self.process_shadowsocks().await
        } else if self.is_trojan(peeked_buffer) {
            console_log!("trojan detected!");
            self.process_trojan().await
        } else if self.is_vmess(peeked_buffer) {
            console_log!("vmess detected!");
            self.process_vmess().await
        } else {
            Err(Error::RustError("protocol not implemented".to_string()))
        }
    }

    pub fn is_vless(&self, buffer: &[u8]) -> bool {
        buffer[0] == 0
    }

    fn is_shadowsocks(&self, buffer: &[u8]) -> bool {
        match buffer[0] {
            1 => { // IPv4
                if buffer.len() < 7 {
                    return false;
                }
                let remote_port = u16::from_be_bytes([buffer[5], buffer[6]]);
                remote_port != 0
            }
            3 => { // Domain name
                if buffer.len() < 2 {
                    return false;
                }
                let domain_len = buffer[1] as usize;
                if buffer.len() < 2 + domain_len + 2 {
                    return false;
                }
                let remote_port = u16::from_be_bytes([
                    buffer[2 + domain_len],
                    buffer[2 + domain_len + 1],
                ]);
                remote_port != 0
            }
            4 => { // IPv6
                if buffer.len() < 19 {
                    return false;
                }
                let remote_port = u16::from_be_bytes([buffer[17], buffer[18]]);
                remote_port != 0
            }
            _ => false,
        }
    }

    fn is_trojan(&self, buffer: &[u8]) -> bool {
        buffer.len() > 57 && buffer[56] == 13 && buffer[57] == 10
    }

    fn is_vmess(&self, buffer: &[u8]) -> bool {
        buffer.len() > 0 // fallback
    }

    /// Check if the target port is commonly used for HTTP services
    fn is_http_port(port: u16) -> bool {
        port == 80 || port == 443 || port == 8080 || port == 8443
    }

    pub async fn handle_tcp_outbound(&mut self, addr: String, port: u16) -> Result<()> {
        if Self::is_http_port(port) {
            console_log!(
                "[WARN] Connecting to {}:{} - This port is typically used for HTTP services. \
                If connection fails, the target may be an HTTP service.",
                &addr, port
            );
        }

        let mut remote_socket = Socket::builder().connect(&addr, port).map_err(|e| {
            let error_msg = e.to_string();
            
            if error_msg.contains("HTTP") || error_msg.contains("http") {
                console_log!(
                    "[ERROR] Failed to connect to {}:{} - Cloudflare detected an HTTP service. \
                    TCP sockets cannot be used for HTTP services on ports 80/443. \
                    Target should be a raw TCP proxy service, not an HTTP endpoint.",
                    &addr, port
                );
                Error::RustError(format!(
                    "HTTP service detected at {}:{}. Cannot use TCP socket for HTTP services. \
                    Please ensure your proxy backend is running a raw TCP protocol (VLESS/VMess/Trojan), \
                    not HTTP/HTTPS.",
                    &addr, port
                ))
            } else {
                console_log!("[ERROR] Connection failed to {}:{} - {}", &addr, port, error_msg);
                Error::RustError(format!("Connection failed to {}:{}: {}", &addr, port, error_msg))
            }
        })?;

        remote_socket.opened().await.map_err(|e| {
            let error_msg = e.to_string();
            console_log!("[ERROR] Socket open failed for {}:{} - {}", &addr, port, error_msg);
            
            if error_msg.contains("HTTP") || error_msg.contains("http") {
                Error::RustError(format!(
                    "HTTP service detected at {}:{}. This proxy destination appears to be an HTTP service. \
                    Please verify your proxy configuration points to a raw TCP service.",
                    &addr, port
                ))
            } else {
                Error::RustError(format!("Socket open failed for {}:{}: {}", &addr, port, error_msg))
            }
        })?;

        console_log!("[SUCCESS] Connected to {}:{}", &addr, port);

        tokio::io::copy_bidirectional(self, &mut remote_socket)
            .await
            .map(|(a_to_b, b_to_a)| {
                console_log!("[STATS] Data transfer from {}:{} completed - up: {} / dl: {}", 
                    &addr, &port, convert(a_to_b as f64), convert(b_to_a as f64));
            })
            .map_err(|e| {
                console_log!("[ERROR] Data transfer error for {}:{} - {}", &addr, port, e);
                Error::RustError(format!("Transfer error for {}:{}: {}", &addr, port, e.to_string()))
            })?;
        Ok(())
    }

    pub async fn handle_udp_outbound(&mut self) -> Result<()> {
        // Reduced buffer size for DNS to 4096 bytes for memory efficiency
        let mut buff = vec![0u8; 4096];

        let n = self.read(&mut buff).await?;
        let data = &buff[..n];
        if crate::dns::doh(data).await.is_ok() {
            self.write(&data).await?;
        };
        Ok(())
    }
}

impl<'a> AsyncRead for ProxyStream<'a> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<tokio::io::Result<()>> {
        let mut this = self.project();

        loop {
            // Send keepalive if needed
            if this.last_activity.elapsed().as_secs() > 30 {
                if let Err(e) = this.ws.send_with_bytes(&[]) {
                    console_log!("[WS] Keepalive send failed: {}", e);
                } else {
                    *this.last_activity = std::time::Instant::now();
                }
            }

            let size = std::cmp::min(this.buffer.len(), buf.remaining());
            if size > 0 {
                buf.put_slice(&this.buffer.split_to(size));
                return Poll::Ready(Ok(()));
            }

            match this.events.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(WebsocketEvent::Message(msg)))) => {
                    *this.last_activity = std::time::Instant::now();
                    if let Some(data) = msg.bytes() {
                        if data.len() > MAX_WEBSOCKET_SIZE {
                            return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, "websocket buffer too long")))
                        }
                        
                        // Ignore empty messages (keepalives)
                        if data.len() == 0 {
                            continue;
                        }
                        
                        if this.buffer.len() + data.len() > MAX_BUFFER_SIZE {
                            console_log!("buffer full, applying backpressure");
                            return Poll::Pending;
                        }
                        
                        this.buffer.put_slice(&data);
                    }
                }
                Poll::Pending => return Poll::Pending,
                _ => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl<'a> AsyncWrite for ProxyStream<'a> {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<tokio::io::Result<usize>> {
        let this = self.project();
        *this.last_activity = std::time::Instant::now();
        
        return Poll::Ready(
            this.ws
                .send_with_bytes(buf)
                .map(|_| buf.len())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
        );
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        match self.ws.close(Some(1000), Some("shutdown".to_string())) {
            Ok(_) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))),
        }
    }
}