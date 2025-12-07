/**
 * ProxyStream - Handles WebSocket proxy connections
 * Supports multiple protocols: VLESS, VMess, Trojan, Shadowsocks
 */

import { VlessHandler } from './vless';
import { VmessHandler } from './vmess';
import { TrojanHandler } from './trojan';
import { ShadowsocksHandler } from './shadowsocks';

export class ProxyStream {
  constructor(config, webSocket) {
    this.config = config;
    this.webSocket = webSocket;
    this.remoteSocket = null;
    this.protocol = null;
  }

  /**
   * Process the proxy stream
   */
  async process() {
    try {
      // Listen for messages from the client
      this.webSocket.addEventListener('message', async (event) => {
        try {
          await this.handleMessage(event.data);
        } catch (error) {
          if (!this.isBenignError(error.message)) {
            console.error('[ERROR] Message handling failed:', error.message);
          }
        }
      });

      // Listen for close events
      this.webSocket.addEventListener('close', () => {
        this.cleanup();
      });

      // Listen for error events
      this.webSocket.addEventListener('error', (error) => {
        if (!this.isBenignError(error.message || 'unknown')) {
          console.error('[ERROR] WebSocket error:', error.message || error);
        }
        this.cleanup();
      });
    } catch (error) {
      if (!this.isBenignError(error.message)) {
        console.error('[ERROR] Stream processing failed:', error.message);
      }
      throw error;
    }
  }

  /**
   * Handle incoming messages from the client
   */
  async handleMessage(data) {
    // Convert data to ArrayBuffer if needed
    let buffer;
    if (data instanceof ArrayBuffer) {
      buffer = data;
    } else if (data instanceof Blob) {
      buffer = await data.arrayBuffer();
    } else if (typeof data === 'string') {
      const encoder = new TextEncoder();
      buffer = encoder.encode(data).buffer;
    } else {
      console.error('[ERROR] Unexpected message type');
      return;
    }

    const uint8Data = new Uint8Array(buffer);

    // If protocol not yet determined, parse the first message
    if (!this.protocol) {
      await this.determineProtocol(uint8Data);
    } else {
      // Forward data to remote socket
      await this.forwardToRemote(uint8Data);
    }
  }

  /**
   * Determine the protocol from the first message
   */
  async determineProtocol(data) {
    try {
      // Try VLESS first (version byte = 0)
      if (data[0] === 0) {
        this.protocol = new VlessHandler(this.config, this.webSocket);
        await this.protocol.handleHandshake(data);
        await this.connectRemote();
        return;
      }

      // Try VMess (authenticated data cipher)
      try {
        this.protocol = new VmessHandler(this.config, this.webSocket);
        await this.protocol.handleHandshake(data);
        await this.connectRemote();
        return;
      } catch (e) {
        // Not VMess, try next
      }

      // Try Trojan (hex string followed by \r\n)
      const dataStr = new TextDecoder().decode(data.slice(0, 60));
      if (dataStr.includes('\r\n')) {
        this.protocol = new TrojanHandler(this.config, this.webSocket);
        await this.protocol.handleHandshake(data);
        await this.connectRemote();
        return;
      }

      // Try Shadowsocks
      this.protocol = new ShadowsocksHandler(this.config, this.webSocket);
      await this.protocol.handleHandshake(data);
      await this.connectRemote();
    } catch (error) {
      console.error('[ERROR] Protocol determination failed:', error.message);
      this.webSocket.close(1002, 'Protocol error');
    }
  }

  /**
   * Connect to remote proxy server using Cloudflare Workers Socket API
   * Requires compatibility_date >= 2021-11-03 and proper network configuration
   */
  async connectRemote() {
    try {
      const { proxyAddr, proxyPort } = this.config;
      
      console.log(`[DEBUG] Connecting to ${proxyAddr}:${proxyPort}`);
      
      // FIXED: Use Cloudflare Workers Socket API (available in Workers with TCP enabled)
      // The connect() function is available globally in Workers runtime
      // Docs: https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/
      
      // Check if connect is available (newer Workers runtime)
      if (typeof connect === 'undefined') {
        throw new Error('TCP Socket API not available. Enable in wrangler.toml with compatibility_flags = ["nodejs_compat"]');
      }
      
      const socket = connect({
        hostname: proxyAddr,
        port: proxyPort
      });

      this.remoteSocket = socket;

      // Get writer for sending data
      this.remoteWriter = socket.writable.getWriter();
      
      // Read from remote and send to client
      const reader = socket.readable.getReader();
      this.readFromRemote(reader);
      
    } catch (error) {
      console.error('[ERROR] Remote connection failed:', error.message);
      console.error('[ERROR] Stack:', error.stack);
      
      // Send error to client
      if (this.webSocket.readyState === 1) { // OPEN
        this.webSocket.close(1002, `Connection failed: ${error.message}`);
      }
    }
  }

  /**
   * Read data from remote socket and forward to client
   */
  async readFromRemote(reader) {
    try {
      while (true) {
        const { done, value } = await reader.read();
        
        if (done) {
          console.log('[DEBUG] Remote stream closed');
          break;
        }

        // Decrypt if protocol requires it
        let decrypted = value;
        if (this.protocol && this.protocol.decrypt) {
          decrypted = await this.protocol.decrypt(value);
        }

        // Send to client WebSocket
        if (this.webSocket.readyState === 1) { // OPEN
          this.webSocket.send(decrypted);
        } else {
          console.log('[DEBUG] WebSocket not open, stopping read');
          break;
        }
      }
    } catch (error) {
      if (!this.isBenignError(error.message)) {
        console.error('[ERROR] Remote read failed:', error.message);
      }
    } finally {
      this.cleanup();
    }
  }

  /**
   * Forward data to remote socket
   */
  async forwardToRemote(data) {
    try {
      if (!this.remoteWriter) {
        console.error('[ERROR] Remote writer not ready');
        return;
      }

      // Encrypt if protocol requires it
      let encrypted = data;
      if (this.protocol && this.protocol.encrypt) {
        encrypted = await this.protocol.encrypt(data);
      }

      await this.remoteWriter.write(encrypted);
    } catch (error) {
      if (!this.isBenignError(error.message)) {
        console.error('[ERROR] Remote write failed:', error.message);
      }
    }
  }

  /**
   * Clean up connections
   */
  cleanup() {
    try {
      if (this.remoteWriter) {
        this.remoteWriter.close().catch(() => {});
        this.remoteWriter = null;
      }
      if (this.remoteSocket) {
        this.remoteSocket.close().catch(() => {});
        this.remoteSocket = null;
      }
    } catch (error) {
      // Ignore cleanup errors
    }
  }

  /**
   * Check if error is benign
   */
  isBenignError(errorMsg) {
    const errorLower = errorMsg.toLowerCase();
    return errorLower.includes('writablestream has been closed') ||
      errorLower.includes('broken pipe') ||
      errorLower.includes('connection reset') ||
      errorLower.includes('connection closed') ||
      errorLower.includes('stream closed') ||
      errorLower.includes('websocket');
  }
}
