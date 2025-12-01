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
    }
}

/// Check if an error is benign (expected during normal operation)
fn is_benign_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    error_lower.contains("writablestream has been closed")
        || error_lower.contains("broken pipe")
        || error_lower.contains("connection reset")
        || error_lower.contains("connection closed")
        || error_lower.contains("network connection lost")
        || error_lower.contains("stream closed")
        || error_lower.contains("eof")
        || error_lower.contains("connection aborted")
        || error_lower.contains("network error")
        || error_lower.contains("socket closed")
}

/// WASM-compatible bidirectional copy implementation
/// Replaces tokio::io::copy_bidirectional for Cloudflare Workers compatibility
async fn copy_bidirectional_wasm<A, B>(
    a: &mut A,
    b: &mut B,
) -> std::io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let mut a_to_b: u64 = 0;
    let mut b_to_a: u64 = 0;
    let mut buf_a = vec![0u8; 8192];
    let mut buf_b = vec![0u8; 8192];

    loop {
        // Create fresh futures each iteration to avoid borrow checker issues
        let a_fut = a.read(&mut buf_a);
        let b_fut = b.read(&mut buf_b);

        futures_util::pin_mut!(a_fut, b_fut);

        match futures_util::future::select(a_fut, b_fut).await {
            futures_util::future::Either::Left((a_result, _)) => {
                match a_result {
                    Ok(0) => {
                        // A reached EOF, shutdown write side of B and drain remaining data from B
                        let _ = b.shutdown().await;
                        loop {
                            match b.read(&mut buf_b).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    a.write_all(&buf_b[..n]).await?;
                                    b_to_a += n as u64;
                                }
                                Err(e) if is_benign_error(&e.to_string()) => break,
                                Err(e) => return Err(e),
                            }
                        }
                        break;
                    }
                    Ok(n) => {
                        b.write_all(&buf_a[..n]).await?;
                        a_to_b += n as u64;
                    }
                    Err(e) if is_benign_error(&e.to_string()) => {
                        // A closed with benign error, drain B
                        let _ = b.shutdown().await;
                        loop {
                            match b.read(&mut buf_b).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    a.write_all(&buf_b[..n]).await?;
                                    b_to_a += n as u64;
                                }
                                Err(e) if is_benign_error(&e.to_string()) => break,
                                Err(e) => return Err(e),
                            }
                        }
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            futures_util::future::Either::Right((b_result, _)) => {
                match b_result {
                    Ok(0) => {
                        // B reached EOF, shutdown write side of A and drain remaining data from A
                        let _ = a.shutdown().await;
                        loop {
                            match a.read(&mut buf_a).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    b.write_all(&buf_a[..n]).await?;
                                    a_to_b += n as u64;
                                }
                                Err(e) if is_benign_error(&e.to_string()) => break,
                                Err(e) => return Err(e),
                            }
                        }
                        break;
                    }
                    Ok(n) => {
                        a.write_all(&buf_b[..n]).await?;
                        b_to_a += n as u64;
                    }
                    Err(e) if is_benign_error(&e.to_string()) => {
                        // B closed with benign error, drain A
                        let _ = a.shutdown().await;
                        loop {
                            match a.read(&mut buf_a).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    b.write_all(&buf_a[..n]).await?;
                                    a_to_b += n as u64;
                                }
                                Err(e) if is_benign_error(&e.to_string()) => break,
                                Err(e) => return Err(e),
                            }
                        }
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    Ok((a_to_b, b_to_a))
}

impl<'a> ProxyStream<'a> {
    pub fn new(config: Config, ws: &'a WebSocket, events: EventStream<'a>) -> Self {
        let buffer = BytesMut::with_capacity(MAX_BUFFER_SIZE);

        Self {
            config,
            ws,
            buffer,
            events,
        }
    }
    
    pub async fn fill_buffer_until(&mut self, n: usize) -> std::io::Result<()> {
        use futures_util::StreamExt;

        while self.buffer.len() < n {
            match self.events.next().await {
                Some(Ok(WebsocketEvent::Message(msg))) => {
                    if let Some(data) = msg.bytes() {
                        self.buffer.put_slice(&data);
                    }
                }
                Some(Ok(WebsocketEvent::Close(_))) => {
                    break;
                }
                Some(Err(e)) => {
                    let error_msg = e.to_string();
                    if !is_benign_error(&error_msg) {
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, error_msg));
                    }
                    break;
                }
                None => {
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

        // Connect with error handling
        let mut remote_socket = match Socket::builder().connect(&addr, port) {
            Ok(socket) => socket,
            Err(e) => {
                let error_msg = e.to_string();
                
                // Benign connection errors - return Ok to suppress CF logging
                if is_benign_error(&error_msg) {
                    return Ok(()); // Silently drop
                }
                
                if error_msg.contains("HTTP") || error_msg.contains("http") {
                    console_log!(
                        "[FATAL] Failed to connect to {}:{} - Cloudflare detected an HTTP service. \
                        TCP sockets cannot be used for HTTP services on ports 80/443. \
                        Target should be a raw TCP proxy service, not an HTTP endpoint.",
                        &addr, port
                    );
                    return Err(Error::RustError(format!(
                        "HTTP service detected at {}:{}. Cannot use TCP socket for HTTP services. \
                        Please ensure your proxy backend is running a raw TCP protocol (VLESS/VMess/Trojan), \
                        not HTTP/HTTPS.",
                        &addr, port
                    )));
                } else {
                    console_log!("[FATAL] Connection failed to {}:{} - {}", &addr, port, error_msg);
                    return Err(Error::RustError(format!("Connection failed to {}:{}: {}", &addr, port, error_msg)));
                }
            }
        };

        // Wait for socket to open
        match remote_socket.opened().await {
            Ok(_) => {},
            Err(e) => {
                let error_msg = e.to_string();
                
                // Benign socket open errors - return Ok to suppress CF logging
                if is_benign_error(&error_msg) {
                    return Ok(()); // Silently drop
                }
                
                console_log!("[FATAL] Socket open failed for {}:{} - {}", &addr, port, error_msg);
                
                if error_msg.contains("HTTP") || error_msg.contains("http") {
                    return Err(Error::RustError(format!(
                        "HTTP service detected at {}:{}. This proxy destination appears to be an HTTP service. \
                        Please verify your proxy configuration points to a raw TCP service.",
                        &addr, port
                    )));
                } else {
                    return Err(Error::RustError(format!("Socket open failed for {}:{}: {}", &addr, port, error_msg)));
                }
            }
        }

        console_log!("[SUCCESS] Connected to {}:{}", &addr, port);

        // WASM-compatible bidirectional copy - replaces tokio::io::copy_bidirectional
        match copy_bidirectional_wasm(self, &mut remote_socket).await {
            Ok((a_to_b, b_to_a)) => {
                console_log!("[STATS] Data transfer from {}:{} completed - up: {} / dl: {}", 
                    &addr, &port, convert(a_to_b as f64), convert(b_to_a as f64));
                Ok(())
            },
            Err(e) => {
                let error_msg = e.to_string();
                
                // THIS IS THE KEY FIX: Benign transfer errors return Ok() to suppress CF error logs
                if is_benign_error(&error_msg) {
                    // Connection naturally closed - not an error
                    return Ok(());
                }
                
                console_log!("[FATAL] Transfer error for {}:{} - {}", &addr, port, error_msg);
                Err(Error::RustError(format!("Transfer error for {}:{}: {}", &addr, port, error_msg)))
            }
        }
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
            let size = std::cmp::min(this.buffer.len(), buf.remaining());
            if size > 0 {
                buf.put_slice(&this.buffer.split_to(size));
                return Poll::Ready(Ok(()));
            }

            match this.events.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(WebsocketEvent::Message(msg)))) => {
                    if let Some(data) = msg.bytes() {
                        if data.len() > MAX_WEBSOCKET_SIZE {
                            return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, "websocket buffer too long")))
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
        return Poll::Ready(
            self.ws
                .send_with_bytes(buf)
                .map(|_| buf.len())
                .map_err(|e| {
                    let error_msg = e.to_string();
                    if is_benign_error(&error_msg) {
                        // Silently handle benign write errors - return BrokenPipe for normal handling
                        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "connection closed")
                    } else {
                        std::io::Error::new(std::io::ErrorKind::Other, error_msg)
                    }
                }),
        );
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        match self.ws.close(Some(1000), Some("shutdown".to_string())) {
            Ok(_) => Poll::Ready(Ok(())),
            Err(e) => {
                let error_msg = e.to_string();
                if is_benign_error(&error_msg) {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        error_msg,
                    )))
                }
            }
        }
    }
}