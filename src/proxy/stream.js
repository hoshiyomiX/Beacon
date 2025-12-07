/**
 * ProxyStream - Handles WebSocket proxy connections
 * Supports multiple protocols: VLESS, VMess, Trojan, Shadowsocks
 */

import { connect } from 'cloudflare:sockets';
import { VlessHandler } from './vless';
import { VmessHandler } from './vmess';
import { TrojanHandler } from './trojan';
import { ShadowsocksHandler } from './shadowsocks';

export class ProxyStream {
  constructor(config, webSocket) {
    this.config = config;
    this.webSocket = webSocket;
    this.remoteSocket = null;
    this.remoteReader = null;
    this.remoteWriter = null;
    this.protocol = null;
    this.isClosing = false;
    console.log('[DEBUG] ProxyStream created');
  }

  /**
   * Process the proxy stream
   */
  async process() {
    try {
      console.log('[DEBUG] Starting proxy stream processing');
      
      // Listen for messages from the client
      this.webSocket.addEventListener('message', async (event) => {
        try {
          console.log(`[DEBUG] Received message, size: ${event.data?.byteLength || event.data?.length || 0} bytes`);
          await this.handleMessage(event.data);
        } catch (error) {
          if (!this.isBenignError(error.message)) {
            console.error('[ERROR] Message handling failed:', error.message);
            console.error('[ERROR] Stack:', error.stack);
          }
        }
      });

      // Listen for close events
      this.webSocket.addEventListener('close', (event) => {
        console.log(`[DEBUG] WebSocket closed: code=${event.code}, reason=${event.reason}`);
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
        console.error('[ERROR] Stack:', error.stack);
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
      console.error('[ERROR] Unexpected message type:', typeof data);
      return;
    }

    const uint8Data = new Uint8Array(buffer);
    console.log(`[DEBUG] Processing ${uint8Data.length} bytes, first byte: 0x${uint8Data[0]?.toString(16).padStart(2, '0')}`);

    // If protocol not yet determined, parse the first message
    if (!this.protocol) {
      console.log('[DEBUG] Determining protocol from first message');
      await this.determineProtocol(uint8Data);
    } else {
      // Forward data to remote socket
      console.log(`[DEBUG] Forwarding ${uint8Data.length} bytes to remote`);
      await this.forwardToRemote(uint8Data);
    }
  }

  /**
   * Determine the protocol from the first message
   */
  async determineProtocol(data) {
    try {
      console.log(`[DEBUG] First byte: 0x${data[0]?.toString(16).padStart(2, '0')}, checking protocols...`);
      
      // Try VLESS first (version byte = 0)
      if (data[0] === 0) {
        console.log('[DEBUG] Detected VLESS protocol (version byte = 0)');
        this.protocol = new VlessHandler(this.config, this.webSocket);
        await this.protocol.handleHandshake(data);
        await this.connectRemote();
        return;
      }

      // Try VMess (authenticated data cipher)
      try {
        console.log('[DEBUG] Trying VMess protocol');
        this.protocol = new VmessHandler(this.config, this.webSocket);
        await this.protocol.handleHandshake(data);
        await this.connectRemote();
        return;
      } catch (e) {
        console.log('[DEBUG] Not VMess, trying next protocol');
      }

      // Try Trojan (hex string followed by \r\n)
      const dataStr = new TextDecoder().decode(data.slice(0, Math.min(60, data.length)));
      if (dataStr.includes('\r\n')) {
        console.log('[DEBUG] Detected Trojan protocol (found CRLF)');
        this.protocol = new TrojanHandler(this.config, this.webSocket);
        await this.protocol.handleHandshake(data);
        await this.connectRemote();
        return;
      }

      // Try Shadowsocks
      console.log('[DEBUG] Trying Shadowsocks protocol');
      this.protocol = new ShadowsocksHandler(this.config, this.webSocket);
      await this.protocol.handleHandshake(data);
      await this.connectRemote();
    } catch (error) {
      console.error('[ERROR] Protocol determination failed:', error.message);
      console.error('[ERROR] Stack:', error.stack);
      this.webSocket.close(1002, 'Protocol error');
    }
  }

  /**
   * Connect to remote proxy server using Cloudflare Workers Socket API
   */
  async connectRemote() {
    try {
      const { proxyAddr, proxyPort } = this.config;
      
      console.log(`[DEBUG] Initiating TCP connection to ${proxyAddr}:${proxyPort}`);
      console.log(`[DEBUG] connect function available: ${typeof connect !== 'undefined'}`);
      
      const socket = connect({
        hostname: proxyAddr,
        port: proxyPort
      });

      console.log('[DEBUG] TCP socket created successfully');
      this.remoteSocket = socket;

      // Get writer for sending data
      this.remoteWriter = socket.writable.getWriter();
      console.log('[DEBUG] Got writable stream writer');
      
      // Read from remote and send to client
      this.remoteReader = socket.readable.getReader();
      console.log('[DEBUG] Got readable stream reader, starting read loop');
      this.readFromRemote();
      
      console.log(`[DEBUG] Successfully connected to ${proxyAddr}:${proxyPort}`);
      
    } catch (error) {
      console.error(`[ERROR] Remote connection to ${this.config.proxyAddr}:${this.config.proxyPort} failed:`, error.message);
      console.error('[ERROR] Stack:', error.stack);
      console.error('[ERROR] Error name:', error.name);
      console.error('[ERROR] Error code:', error.code);
      
      // Send error to client
      if (this.webSocket.readyState === 1) { // OPEN
        this.webSocket.close(1002, `Connection failed: ${error.message}`);
      }
    }
  }

  /**
   * Read data from remote socket and forward to client
   */
  async readFromRemote() {
    try {
      let bytesRead = 0;
      while (!this.isClosing) {
        const { done, value } = await this.remoteReader.read();
        
        if (done) {
          console.log(`[DEBUG] Remote stream closed. Total bytes read: ${bytesRead}`);
          break;
        }

        bytesRead += value.length;
        console.log(`[DEBUG] Read ${value.length} bytes from remote (total: ${bytesRead})`);

        // Decrypt if protocol requires it
        let decrypted = value;
        if (this.protocol && this.protocol.decrypt) {
          decrypted = await this.protocol.decrypt(value);
          console.log(`[DEBUG] Decrypted ${decrypted.length} bytes`);
        }

        // Send to client WebSocket
        if (this.webSocket.readyState === 1 && !this.isClosing) { // OPEN
          this.webSocket.send(decrypted);
          console.log(`[DEBUG] Sent ${decrypted.length} bytes to client`);
        } else {
          console.log(`[DEBUG] WebSocket state: ${this.webSocket.readyState}, stopping read`);
          break;
        }
      }
    } catch (error) {
      if (!this.isBenignError(error.message)) {
        console.error('[ERROR] Remote read failed:', error.message);
        console.error('[ERROR] Stack:', error.stack);
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
      if (!this.remoteWriter || this.isClosing) {
        console.error('[ERROR] Remote writer not ready or closing');
        return;
      }

      // Encrypt if protocol requires it
      let encrypted = data;
      if (this.protocol && this.protocol.encrypt) {
        encrypted = await this.protocol.encrypt(data);
        console.log(`[DEBUG] Encrypted ${encrypted.length} bytes`);
      }

      await this.remoteWriter.write(encrypted);
      console.log(`[DEBUG] Wrote ${encrypted.length} bytes to remote`);
    } catch (error) {
      if (!this.isBenignError(error.message)) {
        console.error('[ERROR] Remote write failed:', error.message);
        console.error('[ERROR] Stack:', error.stack);
      }
    }
  }

  /**
   * Clean up connections
   */
  cleanup() {
    if (this.isClosing) {
      return; // Already cleaning up
    }
    
    this.isClosing = true;
    console.log('[DEBUG] Cleaning up connections');
    
    try {
      // Cancel the reader first to stop the read loop
      if (this.remoteReader) {
        this.remoteReader.cancel().catch(() => {});
        this.remoteReader = null;
        console.log('[DEBUG] Cancelled remote reader');
      }
      
      if (this.remoteWriter) {
        this.remoteWriter.close().catch(() => {});
        this.remoteWriter = null;
        console.log('[DEBUG] Closed remote writer');
      }
      
      if (this.remoteSocket) {
        this.remoteSocket.close().catch(() => {});
        this.remoteSocket = null;
        console.log('[DEBUG] Closed remote socket');
      }
    } catch (error) {
      console.log('[DEBUG] Cleanup error (ignored):', error.message);
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
      errorLower.includes('cancelled') ||
      errorLower.includes('websocket');
  }
}
