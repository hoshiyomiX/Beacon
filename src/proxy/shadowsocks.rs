use super::ProxyStream;
use crate::common::{parse_addr, parse_port};
use worker::*;

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
        || error_lower.contains("transfer error")
}

impl <'a> ProxyStream<'a> {
    pub async fn process_shadowsocks(&mut self) -> Result<()> {
        // read port and address
        let remote_addr = parse_addr(self).await?;
        let remote_port = parse_port(self).await?;
        
        let is_tcp = true; // difficult to detect udp packet from shadowsocks
        
        if is_tcp {
            let addr_pool = [
                (remote_addr.clone(), remote_port),
                (self.config.proxy_addr.clone(), self.config.proxy_port)
            ];

            for (target_addr, target_port) in addr_pool {
                if let Err(e) = self.handle_tcp_outbound(target_addr, target_port).await {
                    let error_msg = e.to_string();
                    if !is_benign_error(&error_msg) && !error_msg.contains("HTTP service detected") {
                        console_error!("[FATAL] TCP error: {}", error_msg);
                    }
                }
            }
        } else {
            if let Err(e) = self.handle_udp_outbound().await {
                let error_msg = e.to_string();
                if !is_benign_error(&error_msg) {
                    console_error!("[FATAL] UDP error: {}", error_msg);
                }
            }
        }

        Ok(())
    }
}