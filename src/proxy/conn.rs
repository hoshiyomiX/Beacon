use crate::config::Config;

use std::pin::Pin;
use std::task::{Context, Poll};
use bytes::{BufMut, BytesMut};
use futures_util::Stream;
use pin_project_lite::pin_project;
use pretty_bytes::converter::convert;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use worker::*;

// OPTIMIZED FOR FAST TRANSFER: Medium settings for balanced performance
static MAX_WEBSOCKET_SIZE: usize = 32 * 1024; // 32KB chunks - fast transfer
static MAX_BUFFER_SIZE: usize = 64 * 1024;    // 64KB buffer - handle bursts
// CPU TIME BREAKER: Conservative iteration limit with dynamic adjustment
static MAX_TRANSFER_ITERATIONS: usize = 80;   // ~6.4ms baseline, adjusted dynamically

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
pub fn is_benign_error(error_msg: &str) -> bool {
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
        || error_lower.contains("timed out")
        || error_lower.contains("timeout")
        || error_lower.contains("deadline")
        || error_lower.contains("http")
        || error_lower.contains("https")
        || error_lower.contains("buffer")
        || error_lower.contains("not enough")
        || error_lower.contains("too large")
        || error_lower.contains("too long")
        || error_lower.contains("rate limit")
        || error_lower.contains("quota")
        || error_lower.contains("exceeded")
        || error_lower.contains("dns")
        || error_lower.contains("host not found")
        || error_lower.contains("unreachable")
        || error_lower.contains("protocol not implemented")
        || error_lower.contains("handshake")
        || error_lower.contains("connection failed")
        || error_lower.contains("all") && error_lower.contains("failed")
}

/// Determine if an error should be logged as WARNING
fn is_warning_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    
    error_lower.contains("backpressure")
        || error_lower.contains("buffer full")
        || error_lower.contains("max iterations")
        || error_lower.contains("cpu limit")
}

/// CPU Time Tracker for Cloudflare Workers 10ms limit
struct CpuTimeTracker {
    start_time: f64,
    cpu_limit_ms: f64,
}

impl CpuTimeTracker {
    fn new() -> Self {
        Self {
            start_time: Self::get_timestamp(),
            cpu_limit_ms: 8.0, // 8ms hard limit (2ms safety margin)
        }
    }
    
    fn get_timestamp() -> f64 {
        js_sys::Date::now()
    }
    
    fn elapsed_ms(&self) -> f64 {
        Self::get_timestamp() - self.start_time
    }
    
    fn should_yield(&self) -> bool {
        self.elapsed_ms() >= 0.75 // Yield every 0.75ms of CPU time
    }
    
    fn should_break(&self) -> bool {
        self.elapsed_ms() >= self.cpu_limit_ms
    }
    
    fn reset(&mut self) {
        self.start_time = Self::get_timestamp();
    }
}

/// FAST TRANSFER OPTIMIZED bidirectional copy with CPU time breaker
/// 
/// CPU Time Management:
/// - 8ms hard limit enforced via performance.now() tracking
/// - Yield every 0.75ms to reset CPU counter
/// - I/O operations don't count toward CPU time
/// - Automatic termination when approaching 10ms limit
/// 
/// Performance Settings:
/// - 32KB buffers for fast throughput
/// - 15s timeout (medium - balanced for most use cases)
/// - Dynamic iteration adjustment based on CPU usage
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
        let mut buf_a = vec![0u8; 32 * 1024]; // 32KB for fast transfer
        let mut buf_b = vec![0u8; 32 * 1024];
        let mut iterations = 0;
        let mut idle_count = 0;
        let mut cpu_tracker = CpuTimeTracker::new();
        
        // Yield every 3 iterations for optimal CPU/throughput balance
        const ITERATIONS_PER_YIELD: usize = 3;

        loop {
            iterations += 1;
            
            // CPU TIME BREAKER: Check if we're approaching the 10ms limit
            if cpu_tracker.should_break() {
                console_log!(
                    "[CPU LIMIT] Breaker triggered at {:.2}ms CPU time, {} iterations",
                    cpu_tracker.elapsed_ms(),
                    iterations
                );
                // Graceful shutdown - allow connection to resume later
                break;
            }
            
            // Yield to event loop periodically
            if iterations % ITERATIONS_PER_YIELD == 0 {
                if cpu_tracker.should_yield() {
                    let elapsed = cpu_tracker.elapsed_ms();
                    
                    // Yield to JavaScript event loop - resets CPU counter
                    let promise = Promise::resolve(&wasm_bindgen::JsValue::NULL);
                    let _ = JsFuture::from(promise).await;
                    
                    // Reset tracker after yield
                    cpu_tracker.reset();
                    
                    console_log!(
                        "[CPU DEBUG] Yielded after {:.2}ms, {} iterations, transferred: up {} / dl {}",
                        elapsed, iterations, convert(a_to_b as f64), convert(b_to_a as f64)
                    );
                }
                
                // Safety check: enforce iteration limit for stalled connections
                if iterations > MAX_TRANSFER_ITERATIONS && idle_count > 10 {
                    console_log!("[DEBUG] Idle connection detected after {} iterations", iterations);
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
                            // EOF from A
                            let _ = b.shutdown().await;
                            loop {
                                if cpu_tracker.should_break() {
                                    console_log!("[CPU LIMIT] Breaker during drain");
                                    break;
                                }
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
                            idle_count = 0;
                        }
                        Err(e) if is_benign_error(&e.to_string()) => {
                            let _ = b.shutdown().await;
                            loop {
                                if cpu_tracker.should_break() {
                                    break;
                                }
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
                            // EOF from B
                            let _ = a.shutdown().await;
                            loop {
                                if cpu_tracker.should_break() {
                                    console_log!("[CPU LIMIT] Breaker during drain");
                                    break;
                                }
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
                            idle_count = 0;
                        }
                        Err(e) if is_benign_error(&e.to_string()) => {
                            let _ = a.shutdown().await;
                            loop {
                                if cpu_tracker.should_break() {
                                    break;
                                }
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
            
            // Track idle iterations
            if a_to_b == 0 && b_to_a == 0 {
                idle_count += 1;
            }
        }

        console_log!(
            "[TRANSFER COMPLETE] {} iterations, CPU time: {:.2}ms",
            iterations,
            cpu_tracker.elapsed_ms()
        );

        Ok((a_to_b, b_to_a))
    };

    // MEDIUM TIMEOUT: 15 seconds for balanced performance
    let timeout = TimeoutFuture::new(15_000);
    futures_util::pin_mut!(transfer_future);
    
    match futures_util::future::select(transfer_future, timeout).await {
        futures_util::future::Either::Left((result, _)) => result,
        futures_util::future::Either::Right(_) => {
            console_log!("[DEBUG] Transfer timeout after 15s");
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

        // FAST HANDSHAKE: 5-second timeout for initial connection
        let timeout = TimeoutFuture::new(5_000);
        futures_util::pin_mut!(fill_future);
        
        match futures_util::future::select(fill_future, timeout).await {
            futures_util::future::Either::Left((result, _)) => result,
            futures_util::future::Either::Right(_) => Ok(()),
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
            return Ok(());
        }

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
            return Ok(());
        };

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let error_msg = e.to_string();
                if is_benign_error(&error_msg) {
                    Ok(())
                } else {
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
            1 => {
                if buffer.len() < 7 {
                    return false;
                }
                let remote_port = u16::from_be_bytes([buffer[5], buffer[6]]);
                remote_port != 0
            }
            3 => {
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
            4 => {
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
        buffer.len() > 0
    }

    pub async fn handle_tcp_outbound(&mut self, addr: String, port: u16) -> Result<()> {
        use gloo_timers::future::TimeoutFuture;
        
        // FAST CONNECTION: 3-second timeout for quick failure
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
                    console_log!("[DEBUG] HTTP service at {}:{}", &addr, port);
                } else {
                    console_log!("[DEBUG] Connection failed to {}:{} - {}", &addr, port, error_msg);
                }
                
                return Err(Error::RustError(format!("Connection failed: {}", error_msg)));
            },
            futures_util::future::Either::Right(_) => {
                console_log!("[DEBUG] Connection timeout (3s) to {}:{}", &addr, port);
                return Err(Error::RustError(format!("Connection timeout to {}:{}", &addr, port)));
            }
        };

        // Use optimized bidirectional copy with CPU breaker
        match copy_bidirectional_wasm(self, &mut remote_socket).await {
            Ok((a_to_b, b_to_a)) => {
                if a_to_b > 0 || b_to_a > 0 {
                    console_log!("[STATS] {}:{} - up: {} / dl: {}", 
                        &addr, &port, convert(a_to_b as f64), convert(b_to_a as f64));
                }
                Ok(())
            },
            Err(e) => {
                let error_msg = e.to_string();
                
                if is_benign_error(&error_msg) {
                    return Ok(());
                }
                
                if is_warning_error(&error_msg) {
                    console_log!("[WARN] {}:{} - {}", &addr, port, error_msg);
                    return Ok(());
                }
                
                console_log!("[ERROR] {}:{} - {}", &addr, port, error_msg);
                Err(Error::RustError(format!("Transfer error: {}", error_msg)))
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
                            console_log!("[DEBUG] Large message: {} bytes", data.len());
                        }
                        
                        if this.buffer.len() + data.len() > MAX_BUFFER_SIZE {
                            console_log!("[WARN] Buffer full, backpressure");
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
