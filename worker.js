import { connect } from 'cloudflare:sockets';

// ============================================================================
// Constants & Configuration
// ============================================================================

const WS_READY_STATE_OPEN = 1;
const WS_READY_STATE_CLOSING = 2;

// Protocol identifiers (base64 encoded)
const TROJAN = atob('dHJvamFu'); // 'trojan'
const VLESS = atob('dm1lc3M=');  // 'vless'
const V2RAY = atob('djJyYXk=');  // 'v2ray'

// DNS Configuration
const DNS_SERVER_ADDRESS = '8.8.8.8';
const DNS_SERVER_PORT = 53;

// UDP Relay Configuration (for DNS and UDP traffic)
const RELAY_SERVER_UDP = {
  host: 'udp-relay.hobihaus.space',
  port: 7300,
};

// CORS headers
const CORS_HEADERS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET,HEAD,POST,OPTIONS',
  'Access-Control-Max-Age': '86400',
};

// Default proxy ports
const PROXY_PORTS = [443, 80];
const PROTOCOLS = [TROJAN, VLESS, 'ss'];

// ============================================================================
// Main Worker Export
// ============================================================================

export default {
  async fetch(request, env, ctx) {
    try {
      const url = new URL(request.url);
      const hostname = url.hostname;
      const upgradeHeader = request.headers.get('Upgrade');

      // WebSocket upgrade for proxy connections
      if (upgradeHeader === 'websocket') {
        // Match proxy patterns: /IP-PORT or /COUNTRY_CODE
        const proxyMatch = url.pathname.match(/^\/(.+[-:=]\d+)$/);
        const countryMatch = url.pathname.match(/^\/([A-Z]{2}(?:,[A-Z]{2})*)$/);

        let proxyIP = '';

        if (countryMatch) {
          // Handle country code routing (e.g., /SG, /ID, /US,SG)
          const countryCodes = countryMatch[1].split(',');
          const selectedCode = countryCodes[Math.floor(Math.random() * countryCodes.length)];
          
          const proxyList = JSON.parse(env.PROXY_LIST || '{}');
          const countryProxies = proxyList[selectedCode];
          
          if (countryProxies && countryProxies.length > 0) {
            const randomProxy = countryProxies[Math.floor(Math.random() * countryProxies.length)];
            proxyIP = randomProxy.replace(':', '-');
          } else {
            return new Response('No proxies available for selected region', { status: 502 });
          }
        } else if (proxyMatch) {
          // Direct proxy IP:PORT specification
          proxyIP = proxyMatch[1];
        }

        if (proxyIP) {
          return await handleWebSocket(request, env, proxyIP);
        }
      }

      // Handle special routes
      if (url.pathname === '/') {
        return await fetchPage(env.MAIN_PAGE_URL);
      } else if (url.pathname === '/sub') {
        return await fetchPage(env.SUB_PAGE_URL);
      } else if (url.pathname === '/link') {
        return await fetchPage(env.LINK_PAGE_URL);
      } else if (url.pathname === '/converter') {
        return await fetchPage(env.CONVERTER_PAGE_URL);
      } else if (url.pathname === '/checker') {
        return await fetchPage(env.CHECKER_PAGE_URL);
      }

      // Default response
      return new Response('Beacon Proxy Server', {
        status: 200,
        headers: { 'Content-Type': 'text/plain' },
      });
    } catch (err) {
      console.error('Worker error:', err);
      return new Response(`Error: ${err.message}`, {
        status: 500,
        headers: CORS_HEADERS,
      });
    }
  },
};

// ============================================================================
// Helper Functions
// ============================================================================

async function fetchPage(url) {
  if (!url) {
    return new Response('Page URL not configured', { status: 500 });
  }
  
  try {
    const response = await fetch(url);
    return new Response(response.body, {
      status: response.status,
      headers: response.headers,
    });
  } catch (err) {
    return new Response(`Failed to fetch page: ${err.message}`, { status: 500 });
  }
}

// ============================================================================
// WebSocket Handler
// ============================================================================

async function handleWebSocket(request, env, proxyIP) {
  const webSocketPair = new WebSocketPair();
  const [client, server] = Object.values(webSocketPair);

  server.accept();

  // Parse proxy address and port
  let proxyAddr = proxyIP.split(/[-:=]/)[0];
  let proxyPort = parseInt(proxyIP.split(/[-:=]/)[1] || '443');

  // Extract early data from header
  const earlyDataHeader = request.headers.get('sec-websocket-protocol') || '';

  // Create readable stream from WebSocket
  const readableStream = makeReadableWebSocketStream(server, earlyDataHeader);

  // State for remote connection
  let remoteSocket = { value: null };
  let isDNS = false;

  // Process incoming WebSocket data
  readableStream
    .pipeTo(
      new WritableStream({
        async write(chunk) {
          // Handle DNS queries through UDP relay
          if (isDNS) {
            return await handleUDPOutbound(
              DNS_SERVER_ADDRESS,
              DNS_SERVER_PORT,
              chunk,
              server,
              null
            );
          }

          // If remote connection exists, forward data
          if (remoteSocket.value) {
            const writer = remoteSocket.value.writable.getWriter();
            await writer.write(chunk);
            writer.releaseLock();
            return;
          }

          // Parse protocol header from first chunk
          const protocol = await detectProtocol(chunk);
          let header;

          if (protocol === TROJAN) {
            header = parseTrojanHeader(chunk);
          } else if (protocol === VLESS) {
            header = parseVlessHeader(chunk);
          } else if (protocol === 'ss') {
            header = parseShadowsocksHeader(chunk);
          } else {
            throw new Error('Unknown protocol');
          }

          if (header.hasError) {
            throw new Error(header.message);
          }

          // Handle UDP traffic
          if (header.isUDP) {
            if (header.portRemote === 53) {
              isDNS = true;
            }
            return await handleUDPOutbound(
              header.addressRemote,
              header.portRemote,
              chunk,
              server,
              header.version
            );
          }

          // Handle TCP traffic
          await handleTCPOutbound(
            remoteSocket,
            header.addressRemote || proxyAddr,
            header.portRemote || proxyPort,
            header.rawClientData,
            server,
            header.version,
            proxyAddr,
            proxyPort
          );
        },
        close() {
          console.log('WebSocket stream closed');
        },
        abort(reason) {
          console.log('WebSocket stream aborted:', reason);
        },
      })
    )
    .catch((err) => {
      console.error('Stream error:', err);
    });

  return new Response(null, {
    status: 101,
    webSocket: client,
  });
}

// ============================================================================
// Protocol Detection & Parsing
// ============================================================================

async function detectProtocol(buffer) {
  // Detect Trojan protocol (check delimiter at byte 56-60)
  if (buffer.byteLength >= 62) {
    const trojanDelimiter = new Uint8Array(buffer.slice(56, 60));
    if (
      trojanDelimiter[0] === 0x0d &&
      trojanDelimiter[1] === 0x0a &&
      (trojanDelimiter[2] === 0x01 || trojanDelimiter[2] === 0x03 || trojanDelimiter[2] === 0x7f) &&
      (trojanDelimiter[3] === 0x01 || trojanDelimiter[3] === 0x03 || trojanDelimiter[3] === 0x04)
    ) {
      return TROJAN;
    }
  }

  // Detect VLESS protocol (check UUID v4 pattern at bytes 1-17)
  const vlessDelimiter = new Uint8Array(buffer.slice(1, 17));
  const hexString = arrayBufferToHex(vlessDelimiter);
  if (hexString.match(/^[0-9a-f]{8}[0-9a-f]{4}4[0-9a-f]{3}[89ab][0-9a-f]{3}[0-9a-f]{12}$/i)) {
    return VLESS;
  }

  // Default to Shadowsocks
  return 'ss';
}

function parseTrojanHeader(buffer) {
  const dataBuffer = buffer.slice(58);
  if (dataBuffer.byteLength < 6) {
    return { hasError: true, message: 'Invalid Trojan request' };
  }

  const view = new DataView(dataBuffer);
  const cmd = view.getUint8(0);
  const isUDP = cmd === 3;

  if (cmd !== 1 && cmd !== 3) {
    return { hasError: true, message: 'Unsupported Trojan command' };
  }

  const addressType = view.getUint8(1);
  let addressLength = 0;
  let addressIndex = 2;
  let addressValue = '';

  switch (addressType) {
    case 1: // IPv4
      addressLength = 4;
      addressValue = new Uint8Array(dataBuffer.slice(addressIndex, addressIndex + addressLength)).join('.');
      break;
    case 3: // Domain
      addressLength = view.getUint8(addressIndex);
      addressIndex += 1;
      addressValue = new TextDecoder().decode(dataBuffer.slice(addressIndex, addressIndex + addressLength));
      break;
    case 4: // IPv6
      addressLength = 16;
      const ipv6View = new DataView(dataBuffer.slice(addressIndex, addressIndex + addressLength));
      const ipv6 = [];
      for (let i = 0; i < 8; i++) {
        ipv6.push(ipv6View.getUint16(i * 2).toString(16));
      }
      addressValue = ipv6.join(':');
      break;
    default:
      return { hasError: true, message: `Invalid address type: ${addressType}` };
  }

  const portIndex = addressIndex + addressLength;
  const portRemote = new DataView(dataBuffer.slice(portIndex, portIndex + 2)).getUint16(0);

  return {
    hasError: false,
    addressRemote: addressValue,
    portRemote: portRemote,
    rawClientData: dataBuffer.slice(portIndex + 4),
    version: null,
    isUDP: isUDP,
  };
}

function parseVlessHeader(buffer) {
  const version = new Uint8Array(buffer.slice(0, 1))[0];
  const optLength = new Uint8Array(buffer.slice(17, 18))[0];
  const cmd = new Uint8Array(buffer.slice(18 + optLength, 18 + optLength + 1))[0];

  const isUDP = cmd === 2;
  if (cmd !== 1 && cmd !== 2) {
    return { hasError: true, message: `Unsupported VLESS command: ${cmd}` };
  }

  const portIndex = 18 + optLength + 1;
  const portRemote = new DataView(buffer.slice(portIndex, portIndex + 2)).getUint16(0);

  const addressIndex = portIndex + 2;
  const addressType = new Uint8Array(buffer.slice(addressIndex, addressIndex + 1))[0];
  let addressLength = 0;
  let addressValueIndex = addressIndex + 1;
  let addressValue = '';

  switch (addressType) {
    case 1: // IPv4
      addressLength = 4;
      addressValue = new Uint8Array(buffer.slice(addressValueIndex, addressValueIndex + addressLength)).join('.');
      break;
    case 2: // Domain
      addressLength = new Uint8Array(buffer.slice(addressValueIndex, addressValueIndex + 1))[0];
      addressValueIndex += 1;
      addressValue = new TextDecoder().decode(buffer.slice(addressValueIndex, addressValueIndex + addressLength));
      break;
    case 3: // IPv6
      addressLength = 16;
      const ipv6View = new DataView(buffer.slice(addressValueIndex, addressValueIndex + addressLength));
      const ipv6 = [];
      for (let i = 0; i < 8; i++) {
        ipv6.push(ipv6View.getUint16(i * 2).toString(16));
      }
      addressValue = ipv6.join(':');
      break;
    default:
      return { hasError: true, message: `Invalid address type: ${addressType}` };
  }

  return {
    hasError: false,
    addressRemote: addressValue,
    portRemote: portRemote,
    rawClientData: buffer.slice(addressValueIndex + addressLength),
    version: new Uint8Array([version, 0]),
    isUDP: isUDP,
  };
}

function parseShadowsocksHeader(buffer) {
  const view = new DataView(buffer);
  const addressType = view.getUint8(0);
  let addressLength = 0;
  let addressIndex = 1;
  let addressValue = '';

  switch (addressType) {
    case 1: // IPv4
      addressLength = 4;
      addressValue = new Uint8Array(buffer.slice(addressIndex, addressIndex + addressLength)).join('.');
      break;
    case 3: // Domain
      addressLength = view.getUint8(addressIndex);
      addressIndex += 1;
      addressValue = new TextDecoder().decode(buffer.slice(addressIndex, addressIndex + addressLength));
      break;
    case 4: // IPv6
      addressLength = 16;
      const ipv6View = new DataView(buffer.slice(addressIndex, addressIndex + addressLength));
      const ipv6 = [];
      for (let i = 0; i < 8; i++) {
        ipv6.push(ipv6View.getUint16(i * 2).toString(16));
      }
      addressValue = ipv6.join(':');
      break;
    default:
      return { hasError: true, message: `Invalid address type: ${addressType}` };
  }

  const portIndex = addressIndex + addressLength;
  const portRemote = new DataView(buffer.slice(portIndex, portIndex + 2)).getUint16(0);

  return {
    hasError: false,
    addressRemote: addressValue,
    portRemote: portRemote,
    rawClientData: buffer.slice(portIndex + 2),
    version: null,
    isUDP: portRemote === 53,
  };
}

// ============================================================================
// TCP & UDP Handlers
// ============================================================================

async function handleTCPOutbound(
  remoteSocket,
  addressRemote,
  portRemote,
  rawClientData,
  webSocket,
  responseHeader,
  proxyAddr,
  proxyPort
) {
  async function connectAndWrite(address, port) {
    const tcpSocket = connect({
      hostname: address,
      port: port,
    });
    remoteSocket.value = tcpSocket;
    console.log(`Connected to ${address}:${port}`);

    const writer = tcpSocket.writable.getWriter();
    await writer.write(rawClientData);
    writer.releaseLock();

    return tcpSocket;
  }

  async function retry() {
    console.log('Retrying connection through proxy...');
    const tcpSocket = await connectAndWrite(proxyAddr, proxyPort);
    tcpSocket.closed
      .catch((error) => {
        console.error('Retry connection error:', error);
      })
      .finally(() => {
        safeCloseWebSocket(webSocket);
      });
    remoteSocketToWS(tcpSocket, webSocket, responseHeader, null);
  }

  // Initial connection attempt
  const tcpSocket = await connectAndWrite(addressRemote, portRemote);
  remoteSocketToWS(tcpSocket, webSocket, responseHeader, retry);
}

async function handleUDPOutbound(targetAddress, targetPort, dataChunk, webSocket, responseHeader) {
  try {
    const tcpSocket = connect({
      hostname: RELAY_SERVER_UDP.host,
      port: RELAY_SERVER_UDP.port,
    });

    // Format: udp:address:port|data
    const header = `udp:${targetAddress}:${targetPort}`;
    const headerBuffer = new TextEncoder().encode(header);
    const separator = new Uint8Array([0x7c]); // '|'
    const relayMessage = new Uint8Array(
      headerBuffer.length + separator.length + dataChunk.byteLength
    );
    relayMessage.set(headerBuffer, 0);
    relayMessage.set(separator, headerBuffer.length);
    relayMessage.set(new Uint8Array(dataChunk), headerBuffer.length + separator.length);

    const writer = tcpSocket.writable.getWriter();
    await writer.write(relayMessage);
    writer.releaseLock();

    // Pipe response back to WebSocket
    let header = responseHeader;
    await tcpSocket.readable.pipeTo(
      new WritableStream({
        async write(chunk) {
          if (webSocket.readyState === WS_READY_STATE_OPEN) {
            if (header) {
              webSocket.send(await new Blob([header, chunk]).arrayBuffer());
              header = null;
            } else {
              webSocket.send(chunk);
            }
          }
        },
        close() {
          console.log(`UDP connection to ${targetAddress}:${targetPort} closed`);
        },
        abort(reason) {
          console.error('UDP connection aborted:', reason);
        },
      })
    );
  } catch (err) {
    console.error('UDP outbound error:', err);
  }
}

async function remoteSocketToWS(remoteSocket, webSocket, responseHeader, retry) {
  let header = responseHeader;
  let hasIncomingData = false;

  await remoteSocket.readable
    .pipeTo(
      new WritableStream({
        async write(chunk) {
          hasIncomingData = true;
          if (webSocket.readyState !== WS_READY_STATE_OPEN) {
            throw new Error('WebSocket not open');
          }
          if (header) {
            webSocket.send(await new Blob([header, chunk]).arrayBuffer());
            header = null;
          } else {
            webSocket.send(chunk);
          }
        },
        close() {
          console.log('Remote socket closed');
        },
        abort(reason) {
          console.error('Remote socket aborted:', reason);
        },
      })
    )
    .catch((error) => {
      console.error('remoteSocketToWS error:', error);
      safeCloseWebSocket(webSocket);
    });

  // Retry if no data received
  if (!hasIncomingData && retry) {
    console.log('No data received, retrying...');
    retry();
  }
}

// ============================================================================
// Utility Functions
// ============================================================================

function makeReadableWebSocketStream(webSocket, earlyDataHeader) {
  let readableStreamCancel = false;
  const stream = new ReadableStream({
    start(controller) {
      webSocket.addEventListener('message', (event) => {
        if (readableStreamCancel) return;
        controller.enqueue(event.data);
      });

      webSocket.addEventListener('close', () => {
        safeCloseWebSocket(webSocket);
        if (readableStreamCancel) return;
        controller.close();
      });

      webSocket.addEventListener('error', (err) => {
        console.error('WebSocket error:', err);
        controller.error(err);
      });

      // Handle early data
      const { earlyData, error } = base64ToArrayBuffer(earlyDataHeader);
      if (error) {
        controller.error(error);
      } else if (earlyData) {
        controller.enqueue(earlyData);
      }
    },
    cancel(reason) {
      if (readableStreamCancel) return;
      console.log('ReadableStream canceled:', reason);
      readableStreamCancel = true;
      safeCloseWebSocket(webSocket);
    },
  });

  return stream;
}

function safeCloseWebSocket(socket) {
  try {
    if (socket.readyState === WS_READY_STATE_OPEN || socket.readyState === WS_READY_STATE_CLOSING) {
      socket.close();
    }
  } catch (error) {
    console.error('Error closing WebSocket:', error);
  }
}

function base64ToArrayBuffer(base64Str) {
  if (!base64Str) {
    return { error: null };
  }
  try {
    base64Str = base64Str.replace(/-/g, '+').replace(/_/g, '/');
    const decode = atob(base64Str);
    const arrayBuffer = Uint8Array.from(decode, (c) => c.charCodeAt(0));
    return { earlyData: arrayBuffer.buffer, error: null };
  } catch (error) {
    return { error };
  }
}

function arrayBufferToHex(buffer) {
  return [...new Uint8Array(buffer)]
    .map((x) => x.toString(16).padStart(2, '0'))
    .join('');
}
