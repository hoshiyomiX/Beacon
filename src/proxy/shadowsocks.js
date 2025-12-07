/**
 * Shadowsocks Protocol Handler (Simplified)
 * Reference: https://shadowsocks.org/en/spec/Protocol.html
 */

export class ShadowsocksHandler {
  constructor(config, webSocket) {
    this.config = config;
    this.webSocket = webSocket;
  }

  /**
   * Handle Shadowsocks handshake and return header info
   * Format: [addr_type(1)] + [addr...] + [port(2)] + [data...]
   */
  async handleHandshake(data) {
    const view = new DataView(data.buffer || data);
    
    const addressType = view.getUint8(0);
    let addressLength = 0;
    let addressValueIndex = 1;
    let addressValue = '';

    switch (addressType) {
      case 1: // IPv4
        addressLength = 4;
        addressValue = new Uint8Array(data.slice(addressValueIndex, addressValueIndex + addressLength)).join('.');
        break;
      case 3: // Domain
        addressLength = new Uint8Array(data.slice(addressValueIndex, addressValueIndex + 1))[0];
        addressValueIndex += 1;
        addressValue = new TextDecoder().decode(data.slice(addressValueIndex, addressValueIndex + addressLength));
        break;
      case 4: // IPv6
        addressLength = 16;
        const dataView = new DataView(data.slice(addressValueIndex, addressValueIndex + addressLength).buffer);
        const ipv6 = [];
        for (let i = 0; i < 8; i++) {
          ipv6.push(dataView.getUint16(i * 2).toString(16));
        }
        addressValue = ipv6.join(':');
        break;
      default:
        throw new Error(`Invalid addressType for Shadowsocks: ${addressType}`);
    }

    if (!addressValue) {
      throw new Error(`Destination address empty, address type is: ${addressType}`);
    }

    const portIndex = addressValueIndex + addressLength;
    const portBuffer = data.slice(portIndex, portIndex + 2);
    const portRemote = new DataView(portBuffer.buffer || portBuffer).getUint16(0);
    
    console.log(`[DEBUG] Shadowsocks: ${addressValue}:${portRemote}`);

    // Return complete data to proxy server
    return {
      addressRemote: addressValue,
      portRemote: portRemote,
      rawClientData: data, // Send COMPLETE data to proxy
      version: null,
    };
  }

  /**
   * Shadowsocks encryption (simplified - would need actual cipher)
   */
  async encrypt(data) {
    // TODO: Implement actual encryption
    return data;
  }

  async decrypt(data) {
    // TODO: Implement actual decryption
    return data;
  }
}
