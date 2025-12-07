/**
 * ProxyStream - Handles WebSocket proxy connections
 * Supports multiple protocols: VLESS, VMess, Trojan, Shadowsocks
 * 
 * Based on FoolVPN-ID/Nautica implementation pattern
 */

import { connect } from 'cloudflare:sockets';
import { VlessHandler } from './vless';
import { VmessHandler } from './vmess';
import { TrojanHandler } from './trojan';
import { ShadowsocksHandler } from './shadowsocks';

const WS_READY_STATE_OPEN = 1;
const WS_READY_STATE_CLOSING = 2;

export class ProxyStream {
  constructor(config, webSocket) {
    this.config = config;
    this.webSocket = webSocket;
    this.remoteSocketWrapper = { value: null };
    this.protocol = null;
    this.addressLog = '';
    this.portLog = '';
    console.log('[DEBUG] ProxyStream created');
  }

  log(info, event) {
    console.log(`[${this.addressLog}:${this.portLog}] ${info}`, event || '');
  }

  /**
   * Process the proxy stream using ReadableStream pattern
   */
  async process() {
    try {
      console.log('[DEBUG] Starting proxy stream processing');
      
      const readableWebSocketStream = this.makeReadableWebSocketStream();

      // Pipe WebSocket data through WritableStream processor
      readableWebSocketStream
        .pipeTo(
          new WritableStream({
            write: async (chunk, controller) => {
              await this.handleChunk(chunk, controller);
            },
            close: () => {
              this.log('readableWebSocketStream is closed');
            },
            abort: (reason) => {
              this.log('readableWebSocketStream is aborted', JSON.stringify(reason));
            },
          })
        )
        .catch((err) => {
          if (!this.isBenignError(err.message || err.toString())) {
            this.log('readableWebSocketStream pipeTo error', err);
          }
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
   * Create ReadableStream from WebSocket (Nautica pattern)
   */
  makeReadableWebSocketStream() {
    let readableStreamCancel = false;
    
    return new ReadableStream({
      start: (controller) => {
        this.webSocket.addEventListener('message', (event) => {
          if (readableStreamCancel) {
            return;
          }
          const message = event.data;
          controller.enqueue(message);
        });

        this.webSocket.addEventListener('close', () => {
          this.safeCloseWebSocket();
          if (readableStreamCancel) {
            return;
          }
          controller.close();
        });

        this.webSocket.addEventListener('error', (err) => {
          this.log('webSocket has error');
          controller.error(err);
        });
      },

      pull: (controller) => {
        // Not needed for WebSocket
      },

      cancel: (reason) => {
        if (readableStreamCancel) {
          return;
        }
        this.log(`ReadableStream was canceled, due to ${reason}`);
        readableStreamCancel = true;
        this.safeCloseWebSocket();
      },
    });
  }

  /**
   * Handle incoming data chunks
   */
  async handleChunk(chunk, controller) {
    try {
      // Convert to ArrayBuffer
      let buffer;
      if (chunk instanceof ArrayBuffer) {
        buffer = chunk;
      } else if (chunk instanceof Blob) {
        buffer = await chunk.arrayBuffer();
      } else if (typeof chunk === 'string') {
        const encoder = new TextEncoder();
        buffer = encoder.encode(chunk).buffer;
      } else {
        console.error('[ERROR] Unexpected chunk type:', typeof chunk);
        return;
      }

      const uint8Data = new Uint8Array(buffer);
      console.log(`[DEBUG] Processing ${uint8Data.length} bytes`);

      // First message: determine protocol and connect
      if (!this.remoteSocketWrapper.value) {
        console.log('[DEBUG] First chunk, determining protocol');
        await this.handleFirstMessage(uint8Data);
      } else {
        // Subsequent messages: forward to remote
        await this.forwardToRemote(uint8Data);
      }
    } catch (error) {
      if (!this.isBenignError(error.message)) {
        console.error('[ERROR] Chunk handling failed:', error.message);
      }
    }
  }

  /**
   * Handle first message to determine protocol and connect
   */
  async handleFirstMessage(data) {
    try {
      console.log(`[DEBUG] First byte: 0x${data[0]?.toString(16).padStart(2, '0')}`);
      
      // Try VLESS first (version byte = 0)
      if (data[0] === 0) {
        console.log('[DEBUG] Detected VLESS protocol');
        this.protocol = new VlessHandler(this.config, this.webSocket);
        const header = await this.protocol.handleHandshake(data);
        this.addressLog = header.addressRemote;
        this.portLog = header.portRemote;
        await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, header.version);
        return;
      }

      // Try VMess
      try {
        console.log('[DEBUG] Trying VMess protocol');
        this.protocol = new VmessHandler(this.config, this.webSocket);
        const header = await this.protocol.handleHandshake(data);
        this.addressLog = header.addressRemote;
        this.portLog = header.portRemote;
        await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, header.version);
        return;
      } catch (e) {
        console.log('[DEBUG] Not VMess');
      }

      // Try Trojan
      const dataStr = new TextDecoder().decode(data.slice(0, Math.min(60, data.length)));
      if (dataStr.includes('\r\n')) {
        console.log('[DEBUG] Detected Trojan protocol');
        this.protocol = new TrojanHandler(this.config, this.webSocket);
        const header = await this.protocol.handleHandshake(data);
        this.addressLog = header.addressRemote;
        this.portLog = header.portRemote;
        await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, null);
        return;
      }

      // Default to Shadowsocks
      console.log('[DEBUG] Defaulting to Shadowsocks');
      this.protocol = new ShadowsocksHandler(this.config, this.webSocket);
      const header = await this.protocol.handleHandshake(data);
      this.addressLog = header.addressRemote;
      this.portLog = header.portRemote;
      await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, null);
    } catch (error) {
      console.error('[ERROR] Protocol determination failed:', error.message);
      this.webSocket.close(1002, 'Protocol error');
    }
  }

  /**
   * Connect to remote and write initial data (Nautica pattern)
   */
  async connectAndWrite(address, port, rawDataAfterHandshake, responseHeader) {
    try {
      console.log(`[DEBUG] Connecting to ${address}:${port}`);
      
      const tcpSocket = connect({
        hostname: address,
        port: port,
      });
      
      this.remoteSocketWrapper.value = tcpSocket;
      this.log(`connected to ${address}:${port}`);
      
      const writer = tcpSocket.writable.getWriter();
      await writer.write(rawDataAfterHandshake);
      writer.releaseLock();
      console.log(`[DEBUG] Sent ${rawDataAfterHandshake.length} bytes`);

      // Pipe remote socket to WebSocket
      this.remoteSocketToWS(tcpSocket, responseHeader);
      
    } catch (error) {
      console.error(`[ERROR] Connection failed:`, error.message);
      this.webSocket.close(1002, `Connection failed: ${error.message}`);
    }
  }

  /**
   * Pipe remote socket to WebSocket (f55692c fix: capture 'this' context)
   */
  async remoteSocketToWS(remoteSocket, responseHeader) {
    let header = responseHeader;
    let hasIncomingData = false;
    
    // f55692c FIX: Capture references before WritableStream to avoid 'this' binding issues
    const webSocket = this.webSocket;
    const protocol = this.protocol;
    const log = this.log.bind(this);
    const isBenignError = this.isBenignError.bind(this);
    const safeCloseWebSocket = this.safeCloseWebSocket.bind(this);
    
    await remoteSocket.readable
      .pipeTo(
        new WritableStream({
          start() {},
          async write(chunk, controller) {
            hasIncomingData = true;
            
            if (webSocket.readyState !== WS_READY_STATE_OPEN) {
              controller.error('webSocket.readyState is not open, maybe closed');
              return;
            }
            
            // Decrypt if needed
            let decrypted = chunk;
            if (protocol && protocol.decrypt) {
              decrypted = await protocol.decrypt(chunk);
            }
            
            // Send to WebSocket (exactly as Nautica)
            if (header) {
              webSocket.send(await new Blob([header, decrypted]).arrayBuffer());
              header = null;
            } else {
              webSocket.send(decrypted);
            }
          },
          close() {
            log(`remoteConnection readable is closed with hasIncomingData=${hasIncomingData}`);
          },
          abort(reason) {
            console.error('remoteConnection readable abort', reason);
          },
        })
      )
      .catch((error) => {
        if (!isBenignError(error.message || error.toString())) {
          console.error('remoteSocketToWS has exception', error.stack || error);
        }
        safeCloseWebSocket();
      });
  }

  /**
   * Forward data to remote socket
   */
  async forwardToRemote(data) {
    try {
      if (!this.remoteSocketWrapper.value) {
        console.error('[ERROR] Remote socket not ready');
        return;
      }

      // Encrypt if protocol requires it
      let encrypted = data;
      if (this.protocol && this.protocol.encrypt) {
        encrypted = await this.protocol.encrypt(data);
      }

      const writer = this.remoteSocketWrapper.value.writable.getWriter();
      await writer.write(encrypted);
      writer.releaseLock();
      
      console.log(`[DEBUG] Forwarded ${encrypted.length} bytes`);
    } catch (error) {
      if (!this.isBenignError(error.message)) {
        console.error('[ERROR] Remote write failed:', error.message);
      }
    }
  }

  /**
   * Safe close WebSocket (Nautica pattern)
   */
  safeCloseWebSocket() {
    try {
      if (
        this.webSocket.readyState === WS_READY_STATE_OPEN ||
        this.webSocket.readyState === WS_READY_STATE_CLOSING
      ) {
        this.webSocket.close();
      }
    } catch (error) {
      console.error('safeCloseWebSocket error', error);
    }
  }

  /**
   * Check if error is benign
   */
  isBenignError(errorMsg) {
    const errorLower = errorMsg.toLowerCase();
    return (
      errorLower.includes('writablestream has been closed') ||
      errorLower.includes('broken pipe') ||
      errorLower.includes('connection reset') ||
      errorLower.includes('connection closed') ||
      errorLower.includes('stream closed') ||
      errorLower.includes('cancelled') ||
      errorLower.includes('canceled') ||
      errorLower.includes('websocket') ||
      errorLower.includes('not open')
    );
  }
}
