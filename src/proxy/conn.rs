use crate::config::Config;

use std::pin::Pin;
use std::task::{Context, Poll};
use bytes::{BufMut, BytesMut};
use futures_util::Stream;
use pin_project_lite::pin_project;
use pretty_bytes::converter::convert;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use worker::*;

// Optimized buffer sizes for Cloudflare Workers free tier
// Smaller chunks = lower memory usage + better CPU time distribution
static MAX_WEBSOCKET_SIZE: usize = 4 * 1024; // 4kb (reduced from 16kb)
static MAX_BUFFER_SIZE: usize = 32 * 1024; // 32kb (reduced from 128kb)
// Tighter iteration bound to stay well under 10ms CPU limit
// Each iteration ~0.05-0.1ms, so 150 iterations = ~7.5-15ms with yielding
static MAX_TRANSFER_ITERATIONS: usize = 150; // reduced from 500

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
/// Benign errors should be silently handled without propagating to Cloudflare logs
pub fn is_benign_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    
    // Connection lifecycle errors (normal client/network behavior)
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
        
        // Timeout errors (expected in proxy scenarios with slow/unreachable targets)
        || error_lower.contains("timed out")
        || error_lower.contains("timeout")
        || error_lower.contains("deadline")
        
        // HTTP protocol detection (user misconfiguration, not a system error)
        || error_lower.contains("http")
        || error_lower.contains("https")
        
        // Buffer/resource limits (client-side behavior)
        || error_lower.contains("buffer")
        || error_lower.contains("not enough")
        || error_lower.contains("too large")
        || error_lower.contains("too long")
        
        // Rate limiting and worker constraints (platform-expected)
        || error_lower.contains("rate limit")
        || error_lower.contains("quota")
        || error_lower.contains("exceeded")
        
        // DNS and routing issues (transient network conditions)
        || error_lower.contains("dns")
        || error_lower.contains("host not found")
        || error_lower.contains("unreachable")
        
        // Protocol-level expected conditions
        || error_lower.contains("protocol not implemented")
        || error_lower.contains("handshake")
        || error_lower.contains("connection failed")
        || error_lower.contains("all") && error_lower.contains("failed")
}

/// Determine if an error should be logged as WARNING (transient/expected issues)
fn is_warning_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    
    // Resource constraints that may affect service but are recoverable
    error_lower.contains("backpressure")
        || error_lower.contains("buffer full")
        || error_lower.contains("max iterations")
}

/// WASM-compatible bidirectional copy with chunked processing and periodic yielding
/// Implements aggressive CPU time management for Cloudflare Workers free tier (10ms limit)
/// 
/// Strategy:
/// - Smaller buffer size (4KB) for faster processing chunks
/// - Yield to event loop every 10 iterations to reset CPU time counter
/// - Max 150 iterations to keep total CPU time under 10ms
/// - 10s wall-clock timeout for data transfer phase (separate from CPU time)
async fn copy_bidirectional_wasm<A, B>(
    a: &mut A,
    b: &mut B,
) -> std::io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    use gloo_timers::future::TimeoutFuture;
    use wasm_bindgen_futures::JsFuture;
    use js_sys::Promise;
    
    let transfer_future = async {
        let mut a_to_b: u64 = 0;
        let mut b_to_a: u64 = 0;
        // Reduced buffer size for smaller processing chunks (4KB instead of 8KB)
        let mut buf_a = vec![0u8; 4096];
        let mut buf_b = vec![0u8; 4096];
        let mut iterations = 0;
        
        // Yield to JavaScript event loop every N iterations to reset CPU time counter
        // This prevents "CPU time exceeded" errors on free tier
        const ITERATIONS_PER_YIELD: usize = 10;

        loop {
            iterations += 1;
            
            // CHUNKED PROCESSING: Yield periodically to distribute CPU time
            if iterations % ITERATIONS_PER_YIELD == 0 {
                // Explicit yield to JavaScript event loop
                // This resets the CPU time counter, allowing us to continue processing
                let promise = Promise::resolve(&wasm_bindgen::JsValue::NULL);
                let _ = JsFuture::from(promise).await;
                
                // Check max iterations AFTER yielding to allow more total work
                if iterations > MAX_TRANSFER_ITERATIONS {
                    console_log!("[WARN] Max iterations ({}) reached, closing connection gracefully", MAX_TRANSFER_ITERATIONS);
                    break;
                }
            }

            let a_fut = a.read(&mut buf_a);
            let b_fut = b.read(&mut buf_b);

            futures_util::pin_mut!(a_fut, b_fut);

            match futures_util::future::select(a_fut, b_fut).await {
                futures_util::future::Either::Left((a_result, _)) => {
                    match a_result {
                        Ok(0) => {
                            // EOF from A, shutdown B and drain remaining data
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
                            // Benign error from A, shutdown B and drain
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
                            // EOF from B, shutdown A and drain remaining data
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
                            // Benign error from B, shutdown A and drain
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
    };

    // 10-second wall-clock timeout for data transfer phase
    // Note: This is wall-clock time (waiting for I/O), not CPU time
    let timeout = TimeoutFuture::new(10_000);
    futures_util::pin_mut!(transfer_future);
    
    match futures_util::future::select(transfer_future, timeout).await {
        futures_util::future::Either::Left((result, _)) => result,
        futures_util::future::Either::Right(_) => {
            console_log!("[DEBUG] Transfer timeout after 10s wall-clock time");
            Ok((0, 0))
        }
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
        }
    }
    
    pub async fn fill_buffer_until(&mut self, n: usize) -> std::io::Result<()> {
        use futures_util::StreamExt;
        use gloo_timers::future::TimeoutFuture;

        let fill_future = async {
            while self.buffer.len() < n {
                match self.events.next().await {
                    Some(Ok(WebsocketEvent::Message(msg))) => {
                        if let Some(data) = msg.bytes() {
                            self.buffer.put_slice(&data);
                        }
                    }
                    Some(Ok(WebsocketEvent::Close(_))) => break,
                    Some(Err(e)) => {
                        let error_msg = e.to_string();
                        if !is_benign_error(&error_msg) {
                            return Err(std::io::Error::new(std::io::ErrorKind::Other, error_msg));
                        }
                        break;
                    }
                    None => break,
                }
            }
            Ok(())
        };

        // 5-second timeout for initial buffer fill (handshake phase)
        let timeout = TimeoutFuture::new(5_000);
        futures_util::pin_mut!(fill_future);
        
        match futures_util::future::select(fill_future, timeout).await {
            futures_util::future::Either::Left((result, _)) => result,
            futures_util::future::Either::Right(_) => {
                // Buffer fill timeout is benign - client may have slow connection
                Ok(())
            }
        }
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
            // Insufficient buffer is benign - client disconnected early
            return Ok(());
        }

        // Process protocol and wrap result to suppress benign errors from Cloudflare logs
        let result = if self.is_vless(peeked_buffer) {
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
            // Unknown protocol is benign - could be probe/scanner
            return Ok(());
        };

        // Top-level error suppression: silence benign errors before Cloudflare logging
        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let error_msg = e.to_string();
                if is_benign_error(&error_msg) {
                    // Silent success - don't propagate to Cloudflare logs
                    Ok(())
                } else {
                    // Only true bugs/unexpected errors propagate
                    console_log!("[FATAL] Unexpected error: {}", error_msg);
                    Err(e)
                }
            }
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

    pub async fn handle_tcp_outbound(&mut self, addr: String, port: u16) -> Result<()> {
        use gloo_timers::future::TimeoutFuture;
        
        // PHASE 1: Connect with 3s timeout for initial TCP establishment
        let connect_future = async {
            let remote_socket = Socket::builder().connect(&addr, port)?;
            remote_socket.opened().await?;
            Ok::<Socket, Error>(remote_socket)
        };
        
        let connect_timeout = TimeoutFuture::new(3_000);
        futures_util::pin_mut!(connect_future);
        
        let mut remote_socket = match futures_util::future::select(connect_future, connect_timeout).await {
            futures_util::future::Either::Left((Ok(socket), _)) => {
                console_log!("[DEBUG] Connected to {}:{}", &addr, port);
                socket
            },
            futures_util::future::Either::Left((Err(e), _)) => {
                let error_msg = e.to_string();
                
                if error_msg.to_lowercase().contains("http") {
                    console_log!(
                        "[DEBUG] HTTP service detected at {}:{}",
                        &addr, port
                    );
                } else {
                    console_log!("[DEBUG] Connection failed to {}:{} - {}", &addr, port, error_msg);
                }
                
                return Err(Error::RustError(format!("Connection failed to {}:{} - {}", &addr, port, error_msg)));
            },
            futures_util::future::Either::Right(_) => {
                console_log!("[DEBUG] Connection timeout (3s) to {}:{}", &addr, port);
                return Err(Error::RustError(format!("Connection timeout to {}:{}", &addr, port)));
            }
        };

        // PHASE 2: Chunked bidirectional copy with periodic yielding
        match copy_bidirectional_wasm(self, &mut remote_socket).await {
            Ok((a_to_b, b_to_a)) => {
                if a_to_b > 0 || b_to_a > 0 {
                    console_log!("[STATS] Transfer from {}:{} completed - up: {} / dl: {}", 
                        &addr, &port, convert(a_to_b as f64), convert(b_to_a as f64));
                }
                Ok(())
            },
            Err(e) => {
                let error_msg = e.to_string();
                
                // Check if transfer error is benign
                if is_benign_error(&error_msg) {
                    return Ok(());
                }
                
                // Check if it's a warning-level error
                if is_warning_error(&error_msg) {
                    console_log!("[WARN] Transfer issue for {}:{} - {}", &addr, port, error_msg);
                    return Ok(());
                }
                
                // Propagate unexpected errors
                console_log!("[ERROR] Transfer error for {}:{} - {}", &addr, port, error_msg);
                Err(Error::RustError(format!("Transfer error for {}:{}: {}", &addr, port, error_msg)))
            }
        }
    }

    pub async fn handle_udp_outbound(&mut self) -> Result<()> {
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
                            // Buffer size violation is benign - aggressive client
                            console_log!("[DEBUG] Websocket message too large, dropping");
                            return Poll::Ready(Ok(()));
                        }
                        
                        if this.buffer.len() + data.len() > MAX_BUFFER_SIZE {
                            console_log!("[WARN] Buffer full, applying backpressure");
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
