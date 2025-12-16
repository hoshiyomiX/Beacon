use crate::config::Config;

use std::pin::Pin;
use std::task::{Context, Poll};
use bytes::{BufMut, BytesMut};
use futures_util::Stream;
use pin_project_lite::pin_project;
use pretty_bytes::converter::convert;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use worker::*;

// OPTIMIZED: Reduced buffer sizes to minimize memory allocation overhead
static MAX_WEBSOCKET_SIZE: usize = 16 * 1024; // 16KB - sweet spot for Workers
static MAX_BUFFER_SIZE: usize = 32 * 1024;    // 32KB - reduced CPU pressure
static MAX_TRANSFER_ITERATIONS: usize = 300;  // Lower limit for CPU time
static MAX_IDLE_ITERATIONS: usize = 20;

pin_project! {
    pub struct ProxyStream<'a> {
        pub config: Config,
        pub ws: &'a WebSocket,
        pub buffer: BytesMut,
        #[pin]
        pub events: EventStream<'a>,
    }
}

/// OPTIMIZED: Inline benign error check without string allocation
#[inline(always)]
pub fn is_benign_error(error_msg: &str) -> bool {
    // Fast path: check most common patterns first
    error_msg.contains("closed") 
        || error_msg.contains("pipe")
        || error_msg.contains("reset")
        || error_msg.contains("eof")
        || error_msg.contains("aborted")
}

/// OPTIMIZED: Simplified warning check
#[inline(always)]
fn is_warning_error(error_msg: &str) -> bool {
    error_msg.contains("backpressure") || error_msg.contains("buffer full")
}

/// MAXIMUM PERFORMANCE bidirectional copy
/// 
/// Key optimizations:
/// - NO yielding (removed spawn_local overhead)
/// - Smaller buffers (16KB) for faster CPU operations
/// - Reduced iteration limits
/// - Minimal error allocations
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
    
    // OPTIMIZED: Smaller buffers = less memory allocation = faster CPU
    let mut buf_a = vec![0u8; 16 * 1024];
    let mut buf_b = vec![0u8; 16 * 1024];
    
    let mut iterations = 0;
    let mut idle_count = 0;

    loop {
        iterations += 1;
        
        // Early exit on iteration limits
        if iterations > MAX_TRANSFER_ITERATIONS {
            if idle_count > MAX_IDLE_ITERATIONS {
                break;
            }
        }
        
        // CRITICAL: Removed all yielding to eliminate promise allocation overhead

        let a_fut = a.read(&mut buf_a);
        let b_fut = b.read(&mut buf_b);

        futures_util::pin_mut!(a_fut, b_fut);

        match futures_util::future::select(a_fut, b_fut).await {
            futures_util::future::Either::Left((a_result, _)) => {
                match a_result {
                    Ok(0) => {
                        // EOF from A - drain B silently
                        let _ = b.shutdown().await;
                        while let Ok(n) = b.read(&mut buf_b).await {
                            if n == 0 { break; }
                            if a.write_all(&buf_b[..n]).await.is_err() { break; }
                            b_to_a += n as u64;
                        }
                        break;
                    }
                    Ok(n) => {
                        if b.write_all(&buf_a[..n]).await.is_err() {
                            break;
                        }
                        a_to_b += n as u64;
                        idle_count = 0;
                    }
                    Err(_) => {
                        // Simplified error handling - no string allocation
                        let _ = b.shutdown().await;
                        while let Ok(n) = b.read(&mut buf_b).await {
                            if n == 0 { break; }
                            if a.write_all(&buf_b[..n]).await.is_err() { break; }
                            b_to_a += n as u64;
                        }
                        break;
                    }
                }
            }
            futures_util::future::Either::Right((b_result, _)) => {
                match b_result {
                    Ok(0) => {
                        // EOF from B - drain A silently
                        let _ = a.shutdown().await;
                        while let Ok(n) = a.read(&mut buf_a).await {
                            if n == 0 { break; }
                            if b.write_all(&buf_a[..n]).await.is_err() { break; }
                            a_to_b += n as u64;
                        }
                        break;
                    }
                    Ok(n) => {
                        if a.write_all(&buf_b[..n]).await.is_err() {
                            break;
                        }
                        b_to_a += n as u64;
                        idle_count = 0;
                    }
                    Err(_) => {
                        // Simplified error handling
                        let _ = a.shutdown().await;
                        while let Ok(n) = a.read(&mut buf_a).await {
                            if n == 0 { break; }
                            if b.write_all(&buf_a[..n]).await.is_err() { break; }
                            a_to_b += n as u64;
                        }
                        break;
                    }
                }
            }
        }
        
        // Track idle iterations
        if a_to_b == 0 && b_to_a == 0 {
            idle_count += 1;
        }
    }

    // OPTIMIZED: Only log significant transfers
    if a_to_b > 1024 || b_to_a > 1024 {
        console_log!("[OK] {} iterations", iterations);
    }
    Ok((a_to_b, b_to_a))
}

impl<'a> ProxyStream<'a> {
    pub fn new(config: Config, ws: &'a WebSocket, events: EventStream<'a>) -> Self {
        // OPTIMIZED: Pre-allocate exact capacity
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
                    Some(Err(_)) => break, // No string allocation
                    None => break,
                }
            }
            Ok(())
        };

        // OPTIMIZED: Reduced handshake timeout to 3s
        let timeout = TimeoutFuture::new(3_000);
        futures_util::pin_mut!(fill_future);
        
        match futures_util::future::select(fill_future, timeout).await {
            futures_util::future::Either::Left((result, _)) => result,
            futures_util::future::Either::Right(_) => Ok(()),
        }
    }

    #[inline(always)]
    pub fn peek_buffer(&self, n: usize) -> &[u8] {
        let len = self.buffer.len().min(n);
        &self.buffer[..len]
    }

    pub async fn process(&mut self) -> Result<()> {
        let peek_buffer_len = 62;
        
        // Simplified error handling
        if self.fill_buffer_until(peek_buffer_len).await.is_err() {
            return Ok(());
        }
        
        let peeked_buffer = self.peek_buffer(peek_buffer_len);

        if peeked_buffer.len() < 31 {
            return Ok(());
        }

        let result = if self.is_vless(peeked_buffer) {
            console_log!("vless");
            self.process_vless().await
        } else if self.is_shadowsocks(peeked_buffer) {
            console_log!("ss");
            self.process_shadowsocks().await
        } else if self.is_trojan(peeked_buffer) {
            console_log!("trojan");
            self.process_trojan().await
        } else if self.is_vmess(peeked_buffer) {
            console_log!("vmess");
            self.process_vmess().await
        } else {
            return Ok(());
        };

        // Simplified error handling - no logging overhead
        result.or(Ok(()))
    }

    #[inline(always)]
    pub fn is_vless(&self, buffer: &[u8]) -> bool {
        buffer[0] == 0
    }

    #[inline(always)]
    fn is_shadowsocks(&self, buffer: &[u8]) -> bool {
        match buffer[0] {
            1 => buffer.len() >= 7 && u16::from_be_bytes([buffer[5], buffer[6]]) != 0,
            3 => {
                if buffer.len() < 2 { return false; }
                let domain_len = buffer[1] as usize;
                buffer.len() >= 2 + domain_len + 2 
                    && u16::from_be_bytes([buffer[2 + domain_len], buffer[2 + domain_len + 1]]) != 0
            }
            4 => buffer.len() >= 19 && u16::from_be_bytes([buffer[17], buffer[18]]) != 0,
            _ => false,
        }
    }

    #[inline(always)]
    fn is_trojan(&self, buffer: &[u8]) -> bool {
        buffer.len() > 57 && buffer[56] == 13 && buffer[57] == 10
    }

    #[inline(always)]
    fn is_vmess(&self, buffer: &[u8]) -> bool {
        buffer.len() > 0
    }

    pub async fn handle_tcp_outbound(&mut self, addr: String, port: u16) -> Result<()> {
        use gloo_timers::future::TimeoutFuture;
        
        // OPTIMIZED: Reduced connection timeout to 3s
        let connect_future = async {
            let remote_socket = Socket::builder().connect(&addr, port)?;
            remote_socket.opened().await?;
            Ok::<Socket, Error>(remote_socket)
        };
        
        let connect_timeout = TimeoutFuture::new(3_000);
        futures_util::pin_mut!(connect_future);
        
        let mut remote_socket = match futures_util::future::select(connect_future, connect_timeout).await {
            futures_util::future::Either::Left((Ok(socket), _)) => socket,
            _ => return Err(Error::RustError("Connection failed".to_string())),
        };

        // Use optimized copy with minimal logging
        match copy_bidirectional_wasm(self, &mut remote_socket).await {
            Ok((a_to_b, b_to_a)) => {
                // Only log significant transfers (> 1MB)
                if a_to_b > 1_000_000 || b_to_a > 1_000_000 {
                    console_log!("[STATS] {}:{} - up: {} / dl: {}", 
                        &addr, &port, convert(a_to_b as f64), convert(b_to_a as f64));
                }
                Ok(())
            },
            Err(_) => Ok(()), // Silent failure
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
                        // OPTIMIZED: Enforce strict buffer limit
                        if this.buffer.len() + data.len() > MAX_BUFFER_SIZE {
                            // Drop message to prevent CPU spike
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
        Poll::Ready(
            self.ws
                .send_with_bytes(buf)
                .map(|_| buf.len())
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "ws error")),
        )
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        let _ = self.ws.close(Some(1000), Some("shutdown".to_string()));
        Poll::Ready(Ok(()))
    }
}
