/**
 * VLESS Protocol Handler
 * Reference: https://github.com/XTLS/Xray-core
 */

export class VlessHandler {
  constructor(config, webSocket) {
    this.config = config;
    this.webSocket = webSocket;
  }

  /**
   * Handle VLESS handshake and return header info
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
    console.log(`[DEBUG] VLESS: Header ends at byte ${offset}, total data: ${data.length} bytes`);

    // CRITICAL FIX: Return only the payload data AFTER the VLESS header
    // The target server should receive raw data, NOT VLESS protocol headers
    const payloadData = data.slice(offset);
    console.log(`[DEBUG] VLESS: Sending ${payloadData.length} bytes of payload to target`);

    return {
      addressRemote: address,
      portRemote: port,
      rawDataAfterHandshake: payloadData, // FIXED: Only send payload, not full handshake
      version: null, // FIXED: No response header - direct connection mode
    };
  }

  /**
   * VLESS doesn't encrypt - just pass through
   */
  async encrypt(data) {
    return data;
  }

  /**
   * VLESS doesn't decrypt - just pass through
   * 
   * In direct connection mode, data from target is already raw HTTP/HTTPS response.
   * No VLESS protocol wrapping needed.
   */
  async decrypt(data) {
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
