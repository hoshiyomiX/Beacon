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
   * Handle Trojan handshake and return header info
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

    console.log('[DEBUG] Trojan connection authenticated');

    // Parse the rest after CRLF
    let offset = crlfIndex + 2;
    const payload = new Uint8Array(data.slice(offset));
    
    const command = payload[0];
    const addrType = payload[1];
    
    let addrOffset = 2;
    let address;
    
    if (addrType === 1) {
      // IPv4
      address = `${payload[addrOffset]}.${payload[addrOffset + 1]}.${payload[addrOffset + 2]}.${payload[addrOffset + 3]}`;
      addrOffset += 4;
    } else if (addrType === 3) {
      // Domain
      const domainLength = payload[addrOffset++];
      address = new TextDecoder().decode(payload.slice(addrOffset, addrOffset + domainLength));
      addrOffset += domainLength;
    } else if (addrType === 4) {
      // IPv6
      const ipv6Parts = [];
      for (let i = 0; i < 16; i += 2) {
        ipv6Parts.push(((payload[addrOffset + i] << 8) | payload[addrOffset + i + 1]).toString(16));
      }
      address = ipv6Parts.join(':');
      addrOffset += 16;
    } else {
      throw new Error(`Invalid address type: ${addrType}`);
    }
    
    const port = (payload[addrOffset] << 8) | payload[addrOffset + 1];
    addrOffset += 2;
    
    console.log(`[DEBUG] Trojan: ${address}:${port}, command=${command}`);
    
    // Find second CRLF to get raw data
    const secondCrlfIndex = dataStr.indexOf('\r\n', crlfIndex + 2);
    const rawDataOffset = secondCrlfIndex !== -1 ? secondCrlfIndex + 2 : offset + addrOffset;

    return {
      addressRemote: address,
      portRemote: port,
      rawClientData: data.slice(rawDataOffset),
      version: null,
    };
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
