use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use worker::*;

// UDP Relay Protocol Constants
const UDP_RELAY_HOST: &str = "udp-relay.hobihaus.space";
const UDP_RELAY_PORT: u16 = 80;
const HEADER_SIZE: usize = 6; // Target address length marker + port

/// UDP Relay handler for proxying UDP traffic through TCP gateway
pub struct UdpRelayHandler {
    target_addr: String,
    target_port: u16,
}

impl UdpRelayHandler {
    /// Create a new UDP relay handler for the specified target
    pub fn new(target: &str) -> Result<Self> {
        let (addr, port_str) = target
            .rsplit_once(':')
            .ok_or_else(|| Error::RustError("Invalid target format, expected host:port".to_string()))?;

        let port = port_str
            .parse::<u16>()
            .map_err(|_| Error::RustError("Invalid port number".to_string()))?;

        Ok(Self {
            target_addr: addr.to_string(),
            target_port: port,
        })
    }

    /// Process UDP relay through TCP connection
    pub async fn process<S>(&self, stream: &mut S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        console_log!(
            "[UDP-RELAY] Establishing relay to {}:{} via {}",
            self.target_addr,
            self.target_port,
            UDP_RELAY_HOST
        );

        // Connect to UDP relay gateway
        let mut relay_socket = Socket::builder()
            .connect(UDP_RELAY_HOST, UDP_RELAY_PORT)
            .map_err(|e| {
                console_log!("[UDP-RELAY] Failed to connect to relay gateway: {}", e);
                Error::RustError(format!("Relay gateway connection failed: {}", e))
            })?;

        relay_socket.opened().await.map_err(|e| {
            console_log!("[UDP-RELAY] Relay socket open failed: {}", e);
            Error::RustError(format!("Relay socket open failed: {}", e))
        })?;

        console_log!("[UDP-RELAY] Connected to relay gateway");

        // Send handshake with target address
        self.send_handshake(&mut relay_socket).await?;

        console_log!(
            "[UDP-RELAY] Handshake sent, starting bidirectional relay"
        );

        // Bidirectional data relay
        tokio::io::copy_bidirectional(stream, &mut relay_socket)
            .await
            .map(|(client_to_relay, relay_to_client)| {
                console_log!(
                    "[UDP-RELAY] Transfer complete - sent: {} bytes, received: {} bytes",
                    client_to_relay,
                    relay_to_client
                );
            })
            .map_err(|e| {
                console_log!("[UDP-RELAY] Transfer error: {}", e);
                Error::RustError(format!("UDP relay transfer error: {}", e))
            })?;

        Ok(())
    }

    /// Send handshake to relay gateway with target address
    /// 
    /// Protocol format (to be adjusted based on actual relay protocol):
    /// [1 byte: address type] [variable: address] [2 bytes: port (big-endian)]
    async fn send_handshake<S>(&self, socket: &mut S) -> Result<()>
    where
        S: AsyncWrite + Unpin,
    {
        let mut handshake = Vec::new();

        // Try to parse as IP address, otherwise treat as domain
        if let Ok(ip) = self.target_addr.parse::<std::net::IpAddr>() {
            match ip {
                std::net::IpAddr::V4(ipv4) => {
                    // Type 0x01 = IPv4
                    handshake.push(0x01);
                    handshake.extend_from_slice(&ipv4.octets());
                }
                std::net::IpAddr::V6(ipv6) => {
                    // Type 0x04 = IPv6
                    handshake.push(0x04);
                    handshake.extend_from_slice(&ipv6.octets());
                }
            }
        } else {
            // Type 0x03 = Domain name
            handshake.push(0x03);
            let addr_bytes = self.target_addr.as_bytes();
            
            // Add domain length (max 255)
            if addr_bytes.len() > 255 {
                return Err(Error::RustError("Domain name too long".to_string()));
            }
            handshake.push(addr_bytes.len() as u8);
            handshake.extend_from_slice(addr_bytes);
        }

        // Add port (big-endian)
        handshake.extend_from_slice(&self.target_port.to_be_bytes());

        console_log!(
            "[UDP-RELAY] Sending handshake: type={}, addr={}, port={}",
            handshake[0],
            self.target_addr,
            self.target_port
        );

        socket
            .write_all(&handshake)
            .await
            .map_err(|e| Error::RustError(format!("Handshake send failed: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_target() {
        let handler = UdpRelayHandler::new("example.com:53").unwrap();
        assert_eq!(handler.target_addr, "example.com");
        assert_eq!(handler.target_port, 53);

        let handler = UdpRelayHandler::new("8.8.8.8:443").unwrap();
        assert_eq!(handler.target_addr, "8.8.8.8");
        assert_eq!(handler.target_port, 443);

        assert!(UdpRelayHandler::new("invalid").is_err());
        assert!(UdpRelayHandler::new("example.com:abc").is_err());
    }
}
