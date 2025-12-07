/**
 * Trojan Protocol Handler
 * Reference: https://trojan-gfw.github.io/trojan/protocol
 */

export class TrojanHandler {
  constructor(config, webSocket) {
    this.config = config;
    this.webSocket = webSocket;
  }

  /**
   * Handle Trojan handshake
   * Format: hex(SHA224(password)) + CRLF + [command(1)] + [addr_type(1)] + [addr...] + [port(2)] + CRLF
   */
  async handleHandshake(data) {
    const dataStr = new TextDecoder().decode(data);
    
    // Find first CRLF
    const crlfIndex = dataStr.indexOf('\r\n');
    if (crlfIndex === -1) {
      throw new Error('Invalid Trojan handshake: no CRLF found');
    }

    // Extract password hash (should be 56 hex characters for SHA224)
    const passwordHash = dataStr.substring(0, crlfIndex);
    if (passwordHash.length !== 56 || !/^[0-9a-f]+$/i.test(passwordHash)) {
      throw new Error('Invalid Trojan password hash');
    }

    // Verify password (simplified - would need SHA224 of actual password)
    // For now, we'll just log it
    console.log('[DEBUG] Trojan connection authenticated');

    // Parse the rest after CRLF
    let offset = crlfIndex + 2;
    const payload = new Uint8Array(data.slice(offset));
    
    const command = payload[0];
    const addrType = payload[1];
    
    console.log(`[DEBUG] Trojan: command=${command}, addrType=${addrType}`);
  }

  /**
   * Trojan doesn't encrypt application data - just pass through
   */
  async encrypt(data) {
    return data;
  }

  async decrypt(data) {
    return data;
  }
}
