/**
 * VLESS Protocol Handler
 * Reference: https://github.com/XTLS/Xray-core
 */

export class VlessHandler {
  constructor(config, webSocket) {
    this.config = config;
    this.webSocket = webSocket;
    this.hasResponse = false;
  }

  /**
   * Handle VLESS handshake
   */
  async handleHandshake(data) {
    // VLESS protocol structure:
    // [version(1)] [uuid(16)] [addon_length(1)] [addons...] [command(1)] [port(2)] [addr_type(1)] [addr...]
    
    if (data.length < 1 + 16) {
      throw new Error('Invalid VLESS handshake: too short');
    }

    // Check version
    if (data[0] !== 0) {
      throw new Error(`Invalid VLESS version: ${data[0]}`);
    }

    // Verify UUID
    const uuid = new Uint8Array(data.slice(1, 17));
    const expectedUuid = this.uuidToBytes(this.config.uuid);
    
    if (!this.compareBytes(uuid, expectedUuid)) {
      throw new Error('Invalid UUID');
    }

    // Parse the rest of the handshake
    let offset = 17;
    const addonLength = data[offset++];
    offset += addonLength; // Skip addons

    // Extract command, port, and address
    const command = data[offset++];
    const port = (data[offset] << 8) | data[offset + 1];
    offset += 2;

    const addrType = data[offset++];
    let address;

    if (addrType === 1) {
      // IPv4
      address = `${data[offset]}.${data[offset + 1]}.${data[offset + 2]}.${data[offset + 3]}`;
      offset += 4;
    } else if (addrType === 2) {
      // Domain
      const domainLength = data[offset++];
      address = new TextDecoder().decode(data.slice(offset, offset + domainLength));
      offset += domainLength;
    } else if (addrType === 3) {
      // IPv6
      const ipv6Parts = [];
      for (let i = 0; i < 16; i += 2) {
        ipv6Parts.push(((data[offset + i] << 8) | data[offset + i + 1]).toString(16));
      }
      address = ipv6Parts.join(':');
      offset += 16;
    } else {
      throw new Error(`Invalid address type: ${addrType}`);
    }

    console.log(`[DEBUG] VLESS: ${address}:${port}, command: ${command}`);

    // For proxy mode, we don't actually use the target address
    // The data will be forwarded to the configured proxy
    this.hasResponse = true;
  }

  /**
   * VLESS doesn't encrypt - just pass through
   */
  async encrypt(data) {
    return data;
  }

  /**
   * VLESS doesn't decrypt - just pass through
   */
  async decrypt(data) {
    if (!this.hasResponse) {
      // First response should include VLESS header: [version(1)] [addon_length(1)] [addons...]
      const response = new Uint8Array(2);
      response[0] = 0; // version
      response[1] = 0; // no addons
      
      this.hasResponse = true;
      
      // Combine response header with actual data
      const combined = new Uint8Array(response.length + data.length);
      combined.set(response, 0);
      combined.set(data, response.length);
      return combined;
    }
    return data;
  }

  /**
   * Convert UUID string to bytes
   */
  uuidToBytes(uuid) {
    const hex = uuid.replace(/-/g, '');
    const bytes = new Uint8Array(16);
    for (let i = 0; i < 16; i++) {
      bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
    }
    return bytes;
  }

  /**
   * Compare two byte arrays
   */
  compareBytes(a, b) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      if (a[i] !== b[i]) return false;
    }
    return true;
  }
}
