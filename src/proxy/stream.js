/**
 * ProxyStream - Handles WebSocket proxy connections
 * Supports multiple protocols: VLESS, VMess, Trojan, Shadowsocks
 * 
 * Based on FoolVPN-ID/Nautica implementation pattern
 * 
 * FIXED Issues:
 * - Issue #B: Stream lifecycle - await pipeTo()
 * - Issue #A: Proxy routing - connect to proxy, not target
 * - Issue #D: Error classification - expanded benign error list
 * - Issue #E: Write validation - proper socket state checks
 * - Issue #C: Protocol detection - improved error handling
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
   * Convert any data type to ArrayBuffer for consistent WebSocket sends
   * This prevents TransformStream type errors
   */
  toArrayBuffer(data) {
    if (data instanceof ArrayBuffer) {
      return data;
    }
    if (data instanceof Uint8Array) {
      // Create a proper ArrayBuffer slice (not a view)
      return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
    }
    if (ArrayBuffer.isView(data)) {
      // Handle other TypedArray types
      return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
    }
    // Shouldn't reach here, but handle as fallback
    console.warn('[WARN] Unexpected data type in toArrayBuffer:', typeof data);
    return data;
  }

  /**
   * Process the proxy stream using ReadableStream pattern
   * FIXED Issue #B: Now awaits the pipeTo() to prevent worker exit before stream completion
   */
  async process() {
    try {
      console.log('[DEBUG] Starting proxy stream processing');
      
      const readableWebSocketStream = this.makeReadableWebSocketStream();

      // ✅ FIXED Issue #B: AWAIT the pipe - don't return immediately!
      // This keeps the worker alive while processing stream data
      await readableWebSocketStream
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
            console.error('[ERROR] readableWebSocketStream pipeTo error:', err);
          }
        });
      
      console.log('[DEBUG] Stream processing completed');
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
   * FIXED: Normalize all WebSocket message types to ArrayBuffer (Issue #2)
   */
  makeReadableWebSocketStream() {
    let readableStreamCancel = false;
    
    return new ReadableStream({
      start: (controller) => {
        this.webSocket.addEventListener('message', async (event) => {
          if (readableStreamCancel) {
            return;
          }
          
          try {
            let arrayBuffer;
            const data = event.data;
            
            // ✅ FIXED Issue #2: NORMALIZE ALL DATA TO ARRAYBUFFER
            if (data instanceof ArrayBuffer) {
              arrayBuffer = data;
            } else if (data instanceof Blob) {
              arrayBuffer = await data.arrayBuffer();
            } else if (data instanceof Uint8Array || ArrayBuffer.isView(data)) {
              // Create proper ArrayBuffer slice
              arrayBuffer = data.buffer.slice(
                data.byteOffset, 
                data.byteOffset + data.byteLength
              );
            } else {
              console.warn('[WARN] Unknown message type:', typeof data);
              return;
            }
            
            controller.enqueue(arrayBuffer);
          } catch (err) {
            controller.error(err);
          }
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
   * FIXED Issue #C: Improved error handling for protocol detection
   */
  async handleFirstMessage(data) {
    try {
      console.log(`[DEBUG] First byte: 0x${data[0]?.toString(16).padStart(2, '0')}`);
      console.log(`[DEBUG] Total data length: ${data.length} bytes`);
      
      // Minimum data validation
      if (data.length < 20) {
        throw new Error(`Insufficient data for protocol detection: ${data.length} bytes`);
      }
      
      // Try VLESS first (version byte = 0)
      if (data[0] === 0) {
        try {
          console.log('[DEBUG] Attempting VLESS protocol');
          this.protocol = new VlessHandler(this.config, this.webSocket);
          const header = await this.protocol.handleHandshake(data);
          this.addressLog = header.addressRemote;
          this.portLog = header.portRemote;
          console.log(`[DEBUG] VLESS detected: ${header.addressRemote}:${header.portRemote}`);
          await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, header.version);
          return;
        } catch (e) {
          console.log(`[DEBUG] VLESS validation failed: ${e.message}`);
          // Continue to try other protocols
        }
      }

      // Try Trojan (second most specific)
      if (data.length >= 58) {
        try {
          console.log('[DEBUG] Attempting Trojan protocol');
          this.protocol = new TrojanHandler(this.config, this.webSocket);
          const header = await this.protocol.handleHandshake(data);
          this.addressLog = header.addressRemote;
          this.portLog = header.portRemote;
          console.log(`[DEBUG] Trojan detected: ${header.addressRemote}:${header.portRemote}`);
          await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, null);
          return;
        } catch (e) {
          console.log(`[DEBUG] Trojan validation failed: ${e.message}`);
          // Continue to try other protocols
        }
      }

      // Try VMess
      try {
        console.log('[DEBUG] Attempting VMess protocol');
        this.protocol = new VmessHandler(this.config, this.webSocket);
        const header = await this.protocol.handleHandshake(data);
        this.addressLog = header.addressRemote;
        this.portLog = header.portRemote;
        console.log(`[DEBUG] VMess detected: ${header.addressRemote}:${header.portRemote}`);
        await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, header.version);
        return;
      } catch (e) {
        console.log(`[DEBUG] VMess validation failed: ${e.message}`);
      }

      // Try Shadowsocks (fallback)
      try {
        console.log('[DEBUG] Attempting Shadowsocks protocol');
        this.protocol = new ShadowsocksHandler(this.config, this.webSocket);
        const header = await this.protocol.handleHandshake(data);
        this.addressLog = header.addressRemote;
        this.portLog = header.portRemote;
        console.log(`[DEBUG] Shadowsocks detected: ${header.addressRemote}:${header.portRemote}`);
        await this.connectAndWrite(header.addressRemote, header.portRemote, header.rawDataAfterHandshake, null);
        return;
      } catch (e) {
        console.log(`[DEBUG] Shadowsocks validation failed: ${e.message}`);
      }

      // None of the protocols matched
      throw new Error('Unable to determine protocol from client data');

    } catch (error) {
      console.error('[ERROR] Protocol determination failed:', error.message);
      try {
        if (this.webSocket.readyState <= 1) {
          this.webSocket.close(1002, `Protocol error: ${error.message}`);
        }
      } catch (closeError) {
        console.error('[ERROR] Failed to close WebSocket:', closeError.message);
      }
    }
  }

  /**
   * Connect to remote and write initial data
   * FIXED Issue #A: Connect to proxy server (config.proxyAddr), not target website
   * FIXED Issue #9: Use try-finally for writer lock safety
   * FIXED Issue #13: Await remoteSocketToWS to prevent WebSocket close during setup
   */
  async connectAndWrite(address, port, rawDataAfterHandshake, responseHeader) {
    let tcpSocket = null;
    
    try {
      // Validate inputs
      if (!address || !port) {
        throw new Error(`Invalid address/port: ${address}:${port}`);
      }
      
      if (!rawDataAfterHandshake || rawDataAfterHandshake.length === 0) {
        throw new Error('No handshake data to send');
      }

      // ✅ FIXED Issue #A: Connect to PROXY server, not target website!
      // The target address comes from VLESS/Trojan header, but we relay through proxy
      console.log(`[DEBUG] Connecting to proxy ${this.config.proxyAddr}:${this.config.proxyPort}`);
      console.log(`[DEBUG] Target destination (will be relayed by proxy): ${address}:${port}`);
      
      tcpSocket = await connect({
        hostname: this.config.proxyAddr,    // ✅ Connect to PROXY
        port: this.config.proxyPort,       // ✅ PROXY port
      });
      
      if (!tcpSocket) {
        throw new Error('Socket creation failed - returned null');
      }
      
      this.remoteSocketWrapper.value = tcpSocket;
      this.log(`connected to ${this.config.proxyAddr}:${this.config.proxyPort}`);
      
      // Validate socket is writable
      if (!tcpSocket.writable) {
        throw new Error('Socket writable is not available');
      }
      
      // ✅ FIXED Issue #9: Use try-finally for writer lock safety
      let writer = null;
      try {
        writer = tcpSocket.writable.getWriter();
        
        console.log(`[DEBUG] Writing ${rawDataAfterHandshake.length} bytes of handshake`);
        await writer.write(rawDataAfterHandshake);
        console.log(`[DEBUG] Successfully sent handshake`);
        
      } catch (writeError) {
        const errorMsg = writeError.message || writeError.toString();
        console.error(`[ERROR] Write failed: ${errorMsg}`);
        throw new Error(`Failed to write handshake: ${errorMsg}`);
        
      } finally {
        if (writer) {
          try {
            writer.releaseLock();  // ✅ Always release lock
            console.log(`[DEBUG] Writer lock released`);
          } catch (unlockError) {
            console.warn(`[WARN] Failed to release writer lock: ${unlockError.message}`);
          }
        }
      }

      // ✅ FIXED Issue #13: AWAIT piping setup
      console.log(`[DEBUG] Starting remote→WebSocket pipe`);
      await this.remoteSocketToWS(tcpSocket, responseHeader);
      console.log(`[DEBUG] Remote socket pipe completed`);
      
    } catch (error) {
      const errorMsg = error.message || error.toString();
      console.error(`[ERROR] Connection failed: ${errorMsg}`);
      console.error(`[ERROR] Stack trace:`, error.stack);
      
      // Clean up socket if still open
      if (tcpSocket && tcpSocket.writable) {
        try {
          tcpSocket.writable.close();
        } catch (closeError) {
          console.error(`[WARN] Failed to close socket: ${closeError.message}`);
        }
      }
      
      // Close WebSocket with error details
      try {
        if (this.webSocket && this.webSocket.readyState <= 1) {
          this.webSocket.close(1002, `Connection failed: ${errorMsg}`);
        }
      } catch (wsCloseError) {
        console.error(`[WARN] Failed to close WebSocket: ${wsCloseError.message}`);
      }
    }
  }

  /**
   * Pipe remote socket to WebSocket (f55692c fix: capture 'this' context)
   * FIXED: All WebSocket sends now use consistent ArrayBuffer type
   * FIXED: Writer lock safety with try-finally (Issue #9)
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
    const toArrayBuffer = this.toArrayBuffer.bind(this);
    
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
            
            // FIXED: Ensure ALL WebSocket sends use ArrayBuffer type for consistency
            // This prevents TransformStream type mismatch errors
            if (header) {
              // First send includes response header
              const combined = await new Blob([header, decrypted]).arrayBuffer();
              webSocket.send(combined);
              header = null;
            } else {
              // Subsequent sends: convert to ArrayBuffer if needed
              const buffer = toArrayBuffer(decrypted);
              webSocket.send(buffer);
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
   * FIXED: Writer lock safety with try-finally (Issue #9)
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

      // ✅ FIXED Issue #9: Use try-finally for writer lock safety
      const writer = this.remoteSocketWrapper.value.writable.getWriter();
      try {
        await writer.write(encrypted);
      } finally {
        writer.releaseLock();  // ✅ Always release lock
      }
      
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
   * FIXED Issue #D: Expanded error classification for better logging
   */
  isBenignError(errorMsg) {
    if (!errorMsg) return true;  // No error = benign
    
    const errorLower = errorMsg.toLowerCase();
    
    // Expected disconnection/timeout scenarios - these are NORMAL
    const benignPatterns = [
      // Stream/Socket closure
      'writablestream has been closed',
      'stream closed',
      'socket closed',
      'connection closed',
      
      // Connection errors (temporary)
      'broken pipe',
      'connection reset',
      'connection refused',
      'connection timeout',
      'connection timed out',
      
      // I/O timeouts
      'read timed out',
      'read timeout',
      'write timed out',
      'write timeout',
      'timeout',
      
      // WebSocket specific
      'websocket',
      'not open',
      'closed',
      
      // Stream end
      'end of stream',
      'eof',
      'stream ended',
      
      // Async cancellation
      'cancelled',
      'canceled',
      'cancel',
      'aborted',
      'abort',
      
      // Network level
      'network error',
      'network unreachable',
      'host unreachable',
      'no route to host',
      'network down',
      
      // DNS
      'dns resolution failed',
      'getaddrinfo',
      'enotfound',
      'unknown host',
      
      // System level (errno equivalents)
      'epipe',
      'econnreset',
      'econnrefused',
      'etimedout',
      'ehostunreach',
      'enetunreach',
    ];
    
    return benignPatterns.some(pattern => errorLower.includes(pattern));
  }
}
