use super::ProxyStream;
use tokio::io::AsyncReadExt;
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
    pub async fn process_trojan(&mut self) -> Result<()> {
        // ignore user_id
        let mut _user_id = [0u8; 56];
        self.read_exact(&mut _user_id).await?;

        // remove crlf
        self.read_u16().await?;
        
        // read instruction
        let network_type = self.read_u8().await?;
        let is_tcp = network_type == 1;

        // read port and address
        let remote_addr = parse_addr(self).await?;
        let remote_port = parse_port(self).await?;

        // remove crlf
        self.read_u16().await?;

        if is_tcp {
            let addr_pool = [
                (remote_addr.clone(), remote_port),
                (self.config.proxy_addr.clone(), self.config.proxy_port)
            ];

            // Try each address in pool, break on first success
            let mut last_error = None;
            for (target_addr, target_port) in addr_pool {
                match self.handle_tcp_outbound(target_addr.clone(), target_port).await {
                    Ok(_) => {
                        console_log!("[SUCCESS] Trojan TCP connection successful to {}:{}", target_addr, target_port);
                        return Ok(()); // Break on first successful connection
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        if !is_benign_error(&error_msg) && !error_msg.contains("HTTP service detected") {
                            console_log!("[WARN] Trojan TCP failed for {}:{} - {}, trying next...", target_addr, target_port, error_msg);
                        }
                        last_error = Some(e);
                        // Continue to next address in pool
                    }
                }
            }
            
            // All addresses failed, return the last error
            if let Some(err) = last_error {
                Err(err)
            } else {
                Err(Error::RustError("All Trojan TCP connections failed".to_string()))
            }
        } else {
            // UDP handling
            if let Err(e) = self.handle_udp_outbound().await {
                let error_msg = e.to_string();
                if !is_benign_error(&error_msg) {
                    console_error!("[FATAL] Trojan UDP error: {}", error_msg);
                }
                return Err(e);
            }
            Ok(())
        }
    }
}
