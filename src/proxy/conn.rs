    pub async fn handle_udp_outbound(&mut self) -> Result<()> {
        // Use a 4KB buffer for DNS queries (typical DNS packet is much smaller; max UDP payload is 65535, but not needed for DNS)
        let mut buff = vec![0u8; 4096];

        let n = self.read(&mut buff).await?;
        let data = &buff[..n];
        if crate::dns::doh(data).await.is_ok() {
            self.write(&data).await?;
        };
        Ok(())
    }
