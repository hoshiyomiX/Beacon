/**
 * Beacon - Cloudflare Workers Proxy (JavaScript)
 * Migrated from Rust/WASM implementation
 */

import { Config } from './config';
import { ProxyStream } from './proxy/stream';

// Regex patterns for proxy validation
const PROXYIP_PATTERN = /^.+-\d+$/;
const PROXYKV_PATTERN = /^([A-Z]{2})/;

/**
 * Check if an error is benign (expected during normal operation)
 */
function isBenignError(errorMsg) {
  const errorLower = errorMsg.toLowerCase();
  return errorLower.includes('writablestream has been closed') ||
    errorLower.includes('broken pipe') ||
    errorLower.includes('connection reset') ||
    errorLower.includes('connection closed') ||
    errorLower.includes('network connection lost') ||
    errorLower.includes('stream closed') ||
    errorLower.includes('eof') ||
    errorLower.includes('connection aborted') ||
    errorLower.includes('transfer error') ||
    errorLower.includes('canceled') ||
    errorLower.includes('cancelled') ||
    errorLower.includes('benign') ||
    errorLower.includes('not enough buffer') ||
    errorLower.includes('websocket') ||
    errorLower.includes('handshake') ||
    errorLower.includes('hung') ||
    errorLower.includes('never generate');
}

/**
 * Fetch and return HTML response from URL
 */
async function getResponseFromUrl(url) {
  try {
    const response = await fetch(url);
    const html = await response.text();
    return new Response(html, {
      headers: { 'Content-Type': 'text/html' }
    });
  } catch (error) {
    console.error(`[ERROR] Failed to fetch ${url}:`, error.message);
    return new Response('Error fetching page', { status: 502 });
  }
}

/**
 * Route handlers
 */
const handlers = {
  fe: (request, config) => getResponseFromUrl(config.mainPageUrl),
  sub: (request, config) => getResponseFromUrl(config.subPageUrl),
  link: (request, config) => getResponseFromUrl(config.linkPageUrl),
  converter: (request, config) => getResponseFromUrl(config.converterPageUrl),
  checker: (request, config) => getResponseFromUrl(config.checkerPageUrl)
};

/**
 * Main tunnel handler for proxy connections
 */
async function handleTunnel(request, config, env, proxyipParam) {
  console.log(`[DEBUG] handleTunnel called with proxyip: ${proxyipParam}`);
  let proxyip = proxyipParam;

  // Handle proxy selection from bundled list
  if (PROXYKV_PATTERN.test(proxyip)) {
    console.log(`[DEBUG] Country code detected: ${proxyip}`);
    const kvidList = proxyip.split(',');
    
    // Get bundled proxy list from environment variables
    const proxyListJson = env.PROXY_LIST || '{}';
    
    try {
      const proxyKv = JSON.parse(proxyListJson);
      
      // Random selection logic
      const randBytes = crypto.getRandomValues(new Uint8Array(2));
      const kvIndex = randBytes[0] % kvidList.length;
      proxyip = kvidList[kvIndex];
      
      // Select random proxy from the country list
      const proxyList = proxyKv[proxyip];
      if (proxyList && proxyList.length > 0) {
        const proxyipIndex = randBytes[0] % proxyList.length;
        proxyip = proxyList[proxyipIndex].replace(':', '-');
        console.log(`[DEBUG] Selected random proxy: ${proxyip}`);
      } else {
        console.error(`[ERROR] No proxies available for country: ${proxyip}`);
        return new Response('No proxies available for selected region', { status: 502 });
      }
    } catch (error) {
      console.error('[ERROR] Invalid PROXY_LIST configuration:', error.message);
      return new Response('Invalid server configuration: PROXY_LIST', { status: 502 });
    }
  }

  // Parse proxy address and port
  if (PROXYIP_PATTERN.test(proxyip)) {
    const parts = proxyip.split('-');
    if (parts.length === 2) {
      config.proxyAddr = parts[0];
      config.proxyPort = parseInt(parts[1], 10);
      console.log(`[DEBUG] Parsed proxy: ${config.proxyAddr}:${config.proxyPort}`);
    }
  }

  // Check for WebSocket upgrade
  const upgrade = request.headers.get('Upgrade');
  console.log(`[DEBUG] Upgrade header: ${upgrade}`);
  console.log(`[DEBUG] All request headers:`, Object.fromEntries(request.headers));
  
  if (upgrade === 'websocket') {
    console.log('[DEBUG] WebSocket upgrade detected, creating WebSocket pair');
    try {
      // Create WebSocket pair
      const pair = new WebSocketPair();
      const [client, server] = [pair[0], pair[1]];
      
      console.log('[DEBUG] WebSocket pair created successfully');
      
      // Accept the WebSocket connection
      server.accept();
      console.log('[DEBUG] Server WebSocket accepted');
      
      // Add connection monitoring
      server.addEventListener('open', () => {
        console.log('[DEBUG] Server WebSocket OPEN event');
      });
      
      server.addEventListener('close', (event) => {
        console.log(`[DEBUG] Server WebSocket CLOSE event: code=${event.code}, reason=${event.reason}, wasClean=${event.wasClean}`);
      });
      
      server.addEventListener('error', (event) => {
        console.error('[ERROR] Server WebSocket ERROR event:', event);
      });
      
      // Handle WebSocket proxy stream
      const proxyStream = new ProxyStream(config, server);
      
      // Process stream with timeout
      const processPromise = proxyStream.process();
      const timeoutPromise = new Promise((resolve) => 
        setTimeout(() => resolve('timeout'), 8000)
      );
      
      Promise.race([processPromise, timeoutPromise])
        .then((result) => {
          if (result === 'timeout') {
            console.log('[DEBUG] Connection timeout (8 seconds)');
          }
          server.close(1000, 'Connection closed');
        })
        .catch((error) => {
          if (!isBenignError(error.message)) {
            console.error('[FATAL] Unexpected tunnel error:', error.message);
            console.error('[FATAL] Stack:', error.stack);
          }
          server.close(1000, 'Connection closed');
        });
      
      console.log('[DEBUG] Returning WebSocket upgrade response (101)');
      // Return the client WebSocket in the response
      return new Response(null, {
        status: 101,
        webSocket: client
      });
    } catch (error) {
      console.error('[ERROR] WebSocket response creation failed:', error.message);
      console.error('[ERROR] Stack:', error.stack);
      return new Response('WebSocket handshake failed', { status: 400 });
    }
  } else {
    console.log('[DEBUG] No WebSocket upgrade, returning plain response');
    return new Response('hi from JavaScript!', {
      headers: { 'Content-Type': 'text/html' }
    });
  }
}

/**
 * Main request handler
 */
export default {
  async fetch(request, env, ctx) {
    try {
      const url = new URL(request.url);
      console.log(`[DEBUG] === New Request: ${request.method} ${url.pathname} ===`);
      console.log(`[DEBUG] Host: ${url.hostname}`);
      
      // Parse UUID
      const uuid = env.UUID;
      if (!uuid) {
        console.error('[ERROR] UUID environment variable not found');
        return new Response('Server configuration error: Missing UUID', { status: 502 });
      }
      
      // Validate UUID format (basic check)
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(uuid)) {
        console.error('[ERROR] Invalid UUID format in environment variable');
        return new Response('Invalid server configuration: UUID', { status: 502 });
      }
      
      const host = url.hostname;
      
      // Get environment variables
      const mainPageUrl = env.MAIN_PAGE_URL;
      const subPageUrl = env.SUB_PAGE_URL;
      const linkPageUrl = env.LINK_PAGE_URL;
      const converterPageUrl = env.CONVERTER_PAGE_URL;
      const checkerPageUrl = env.CHECKER_PAGE_URL;
      
      // Validate required environment variables
      if (!mainPageUrl || !subPageUrl || !linkPageUrl || !converterPageUrl || !checkerPageUrl) {
        console.error('[ERROR] Missing required environment variables');
        return new Response('Server configuration error: Missing page URLs', { status: 502 });
      }
      
      // Create config
      const config = new Config({
        uuid,
        host,
        proxyAddr: host,
        proxyPort: 443,
        mainPageUrl,
        subPageUrl,
        linkPageUrl,
        converterPageUrl,
        checkerPageUrl
      });
      
      // Route the request
      const pathname = url.pathname;
      console.log(`[DEBUG] Routing pathname: ${pathname}`);
      
      if (pathname === '/') {
        return handlers.fe(request, config);
      } else if (pathname === '/sub') {
        return handlers.sub(request, config);
      } else if (pathname === '/link') {
        return handlers.link(request, config);
      } else if (pathname === '/converter') {
        return handlers.converter(request, config);
      } else if (pathname === '/checker') {
        return handlers.checker(request, config);
      } else {
        // Extract proxy parameter
        const proxyipMatch = pathname.match(/^\/(?:Geo-Project\/)?(.*?)$/);
        if (proxyipMatch && proxyipMatch[1]) {
          console.log(`[DEBUG] Matched proxy path, proxyip: ${proxyipMatch[1]}`);
          return handleTunnel(request, config, env, proxyipMatch[1]);
        }
      }
      
      console.log('[DEBUG] No route matched, returning 404');
      return new Response('Not Found', { status: 404 });
    } catch (error) {
      console.error('[ERROR] Request handling failed:', error.message);
      console.error('[ERROR] Stack:', error.stack);
      return new Response('Internal Server Error', { status: 500 });
    }
  }
};
