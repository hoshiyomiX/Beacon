use super::ProxyStream;
use crate::common::{
    hash, parse_port, parse_addr, KDFSALT_CONST_AEAD_RESP_HEADER_IV, KDFSALT_CONST_AEAD_RESP_HEADER_KEY, KDFSALT_CONST_AEAD_RESP_HEADER_LEN_IV, KDFSALT_CONST_AEAD_RESP_HEADER_LEN_KEY, KDFSALT_CONST_VMESS_HEADER_PAYLOAD_AEAD_IV, KDFSALT_CONST_VMESS_HEADER_PAYLOAD_AEAD_KEY, KDFSALT_CONST_VMESS_HEADER_PAYLOAD_LENGTH_AEAD_IV, KDFSALT_CONST_VMESS_HEADER_PAYLOAD_LENGTH_AEAD_KEY
};
use std::io::Cursor;
use aes::cipher::KeyInit;
use aes_gcm::{
    aead::{Aead, Payload},
    Aes128Gcm,
};
use md5::{Digest, Md5};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    async fn aead_decrypt(&mut self) -> Result<Vec<u8>> {
        let key = crate::md5!(
            &self.config.uuid.as_bytes(),
            b"c48619fe-8f02-49e0-b9e9-edf763e17e21"
        );

        // +-------------------+-------------------+-------------------+
        // |     Auth ID       |   Header Length   |       Nonce       |
        // +-------------------+-------------------+-------------------+
        // |     16 Bytes      |     18 Bytes      |      8 Bytes      |
        // +-------------------+-------------------+-------------------+
        let mut auth_id = [0u8; 16];
        self.read_exact(&mut auth_id).await?;
        let mut len = [0u8; 18];
        self.read_exact(&mut len).await?;
        let mut nonce = [0u8; 8];
        self.read_exact(&mut nonce).await?;

        // https://github.com/v2fly/v2ray-core/blob/master/proxy/vmess/aead/kdf.go
        let header_length = {
            let header_length_key = &hash::kdf(
                &key,
                &[
                    KDFSALT_CONST_VMESS_HEADER_PAYLOAD_LENGTH_AEAD_KEY,
                    &auth_id,
                    &nonce,
                ],
            )[..16];
            let header_length_nonce = &hash::kdf(
                &key,
                &[
                    KDFSALT_CONST_VMESS_HEADER_PAYLOAD_LENGTH_AEAD_IV,
                    &auth_id,
                    &nonce,
                ],
            )[..12];

            let payload = Payload {
                msg: &len,
                aad: &auth_id,
            };

            let len = Aes128Gcm::new(header_length_key.into())
                .decrypt(header_length_nonce.into(), payload)
                .map_err(|e| Error::RustError(e.to_string()))?;

            ((len[0] as u16) << 8) | (len[1] as u16)
        };

        // 16 bytes padding
        let mut cmd = vec![0u8; (header_length + 16) as _];
        self.read_exact(&mut cmd).await?;

        let header_payload = {
            let payload_key = &hash::kdf(
                &key,
                &[
                    KDFSALT_CONST_VMESS_HEADER_PAYLOAD_AEAD_KEY,
                    &auth_id,
                    &nonce,
                ],
            )[..16];
            let payload_nonce = &hash::kdf(
                &key,
                &[KDFSALT_CONST_VMESS_HEADER_PAYLOAD_AEAD_IV, &auth_id, &nonce],
            )[..12];

            let payload = Payload {
                msg: &cmd,
                aad: &auth_id,
            };

            Aes128Gcm::new(payload_key.into())
                .decrypt(payload_nonce.into(), payload)
                .map_err(|e| Error::RustError(e.to_string()))?
        };

        Ok(header_payload)
    }

    pub async fn process_vmess(&mut self) -> Result<()> {
        let mut buf = Cursor::new(self.aead_decrypt().await?);

        // https://xtls.github.io/en/development/protocols/vmess.html#command-section
        //
        // +---------+--------------------+---------------------+-------------------------------+---------+----------+-------------------+----------+---------+---------+--------------+---------+--------------+----------+
        // | 1 Byte  |      16 Bytes      |      16 Bytes       |            1 Byte             | 1 Byte  |  4 bits  |      4 bits       |  1 Byte  | 1 Byte  | 2 Bytes |    1 Byte    | N Bytes |   P Bytes    | 4 Bytes  |
        // +---------+--------------------+---------------------+-------------------------------+---------+----------+-------------------+----------+---------+---------+--------------+---------+--------------+----------+
        // | Version | Data Encryption IV | Data Encryption Key | Response Authentication Value | Options | Reserved | Encryption Method | Reserved | Command | Port    | Address Type | Address | Random Value | Checksum |
        // +---------+--------------------+---------------------+-------------------------------+---------+----------+-------------------+----------+---------+---------+--------------+---------+--------------+----------+

        let version = buf.read_u8().await?;
        if version != 1 {
            return Err(Error::RustError("invalid version".to_string()));
        }

        let mut iv = [0u8; 16];
        buf.read_exact(&mut iv).await?;
        let mut key = [0u8; 16];
        buf.read_exact(&mut key).await?;

        // ignore options for now
        let mut options = [0u8; 4];
        buf.read_exact(&mut options).await?;

        let cmd = buf.read_u8().await?;
        let is_tcp = cmd == 0x1;

        let remote_port = parse_port(&mut buf).await?;
        let remote_addr = parse_addr(&mut buf).await?;

        // encrypt payload
        let key = &crate::sha256!(&key)[..16];
        let iv = &crate::sha256!(&iv)[..16];

        // https://github.com/v2ray/v2ray-core/blob/master/proxy/vmess/encoding/client.go#L196
        let length_key = &hash::kdf(&key, &[KDFSALT_CONST_AEAD_RESP_HEADER_LEN_KEY])[..16];
        let length_iv = &hash::kdf(&iv, &[KDFSALT_CONST_AEAD_RESP_HEADER_LEN_IV])[..12];
        let length = Aes128Gcm::new(length_key.into())
            // 4 bytes header: https://github.com/v2ray/v2ray-core/blob/master/proxy/vmess/encoding/client.go#L238
            .encrypt(length_iv.into(), &4u16.to_be_bytes()[..])
            .map_err(|e| Error::RustError(e.to_string()))?;
        self.write(&length).await?;

        let payload_key = &hash::kdf(&key, &[KDFSALT_CONST_AEAD_RESP_HEADER_KEY])[..16];
        let payload_iv = &hash::kdf(&iv, &[KDFSALT_CONST_AEAD_RESP_HEADER_IV])[..12];
        let header = {
            let header = [
                options[0], // https://github.com/v2ray/v2ray-core/blob/master/proxy/vmess/encoding/client.go#L242
                0x00, 0x00, 0x00,
            ];
            Aes128Gcm::new(payload_key.into())
                .encrypt(payload_iv.into(), &header[..])
                .map_err(|e| Error::RustError(e.to_string()))?
        };
        self.write(&header).await?;

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
                        console_log!("[SUCCESS] VMess TCP connection successful to {}:{}", target_addr, target_port);
                        return Ok(()); // Break on first successful connection
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        if !is_benign_error(&error_msg) && !error_msg.contains("HTTP service detected") {
                            console_log!("[WARN] VMess TCP failed for {}:{} - {}, trying next...", target_addr, target_port, error_msg);
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
                Err(Error::RustError("All VMess TCP connections failed".to_string()))
            }
        } else {
            // UDP handling
            if let Err(e) = self.handle_udp_outbound().await {
                let error_msg = e.to_string();
                if !is_benign_error(&error_msg) {
                    console_error!("[FATAL] VMess UDP error: {}", error_msg);
                }
                return Err(e);
            }
            Ok(())
        }
    }
}
