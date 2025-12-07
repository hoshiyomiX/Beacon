//! Centralized error classification for proxy operations
//!
//! Benign errors are expected during normal operation (client disconnects, timeouts, etc.)
//! and should be silently handled without propagating to Cloudflare logs.

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
        || error_lower.contains("transfer error")
        || error_lower.contains("canceled")
        || error_lower.contains("cancelled")
        
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
        || error_lower.contains("websocket")
        || error_lower.contains("hung")
        || error_lower.contains("never generate")
        || (error_lower.contains("all") && error_lower.contains("failed"))
}

/// Determine if an error should be logged as WARNING (transient/expected issues)
pub fn is_warning_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    
    // Resource constraints that may affect service but are recoverable
    error_lower.contains("backpressure")
        || error_lower.contains("buffer full")
        || error_lower.contains("max iterations")
}