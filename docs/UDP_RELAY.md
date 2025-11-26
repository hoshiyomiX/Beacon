# UDP Relay Feature

## Overview

Beacon now supports UDP traffic relaying through a TCP-to-UDP gateway. This feature allows you to proxy UDP packets through Cloudflare Workers by leveraging an external UDP relay service.

## How It Works

```
Client (WebSocket) → Beacon (Workers) → UDP Relay Gateway (TCP) → Target (UDP)
```

1. **Client connects** to Beacon via WebSocket
2. **Beacon establishes** TCP connection to `udp-relay.hobihaus.space:80`
3. **Handshake sent** with target UDP server address
4. **Bidirectional relay** between client and target UDP server

## Usage

### Endpoint Format

```
wss://your-beacon.workers.dev/udp/{target_host}:{target_port}
```

### Examples

**DNS Query:**
```
wss://your-beacon.workers.dev/udp/8.8.8.8:53
```

**Game Server:**
```
wss://your-beacon.workers.dev/udp/mc.hypixel.net:25565
```

**VoIP Service:**
```
wss://your-beacon.workers.dev/udp/voip.example.com:5060
```

## Protocol Details

### Handshake Format

Beacon sends a SOCKS5-like handshake to the UDP relay gateway:

```
[1 byte: address type] [variable: address] [2 bytes: port (big-endian)]
```

**Address Types:**
- `0x01` - IPv4 (4 bytes)
- `0x03` - Domain name (1 byte length + domain)
- `0x04` - IPv6 (16 bytes)

**Example Handshake for `example.com:53`:**
```
0x03 0x0B 65 78 61 6D 70 6C 65 2E 63 6F 6D 0x00 0x35
│    │    └─────────────────────────────┘    └──────┘
│    │              domain                    port 53
│    └─ length (11)
└─ type (domain)
```

## Client Implementation

### JavaScript/Browser

```javascript
const ws = new WebSocket('wss://your-beacon.workers.dev/udp/8.8.8.8:53');

ws.binaryType = 'arraybuffer';

ws.onopen = () => {
  console.log('UDP relay connected');
  
  // Send DNS query
  const dnsQuery = new Uint8Array([/* DNS packet */]);
  ws.send(dnsQuery);
};

ws.onmessage = (event) => {
  const response = new Uint8Array(event.data);
  console.log('UDP response:', response);
};
```

### Python

```python
import asyncio
import websockets

async def udp_relay():
    uri = "wss://your-beacon.workers.dev/udp/8.8.8.8:53"
    
    async with websockets.connect(uri) as ws:
        # Send UDP packet
        await ws.send(b"\x00\x00...")
        
        # Receive response
        response = await ws.recv()
        print(f"Received: {response}")

asyncio.run(udp_relay())
```

## Limitations

### Cloudflare Workers Constraints

- **30-second CPU time limit** - Long-running connections may timeout
- **No direct UDP** - All UDP traffic must go through TCP relay
- **Egress bandwidth** - Subject to Cloudflare Workers limits

### UDP Relay Gateway

- **Public relay** at `udp-relay.hobihaus.space:80` is shared
- **Rate limiting** may apply
- **Protocol compatibility** - Ensure target supports UDP

## Common Use Cases

### ✅ Supported

- **DNS queries** (port 53)
- **NTP time sync** (port 123)
- **Game server queries** (various ports)
- **VoIP signaling** (SIP, etc.)
- **Short-lived UDP communications**

### ❌ Not Recommended

- **Video/audio streaming** (high bandwidth, long duration)
- **P2P protocols** (NAT traversal complexity)
- **Long-running stateful connections** (Workers timeout)

## Troubleshooting

### Connection Fails

**Check target format:**
```
✅ wss://beacon.workers.dev/udp/example.com:53
❌ wss://beacon.workers.dev/udp/example.com
❌ wss://beacon.workers.dev/udp/example.com:abc
```

**Verify WebSocket upgrade:**
```bash
curl -i -N \
  -H "Connection: Upgrade" \
  -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Key: $(openssl rand -base64 16)" \
  https://your-beacon.workers.dev/udp/8.8.8.8:53
```

### Timeout Issues

- **Keep connections short** (<30 seconds)
- **Use connection pooling** for repeated queries
- **Implement retry logic** on client side

### Debug Logs

Check Cloudflare Workers logs:
```bash
wrangler tail
```

Look for:
- `[UDP-RELAY] Establishing relay to...`
- `[UDP-RELAY] Connected to relay gateway`
- `[UDP-RELAY] Transfer complete`

## Advanced Configuration

### Custom Relay Gateway

Modify `src/proxy/udp_relay.rs`:

```rust
const UDP_RELAY_HOST: &str = "your-relay.example.com";
const UDP_RELAY_PORT: u16 = 7300;
```

### Protocol Modifications

If using a different relay protocol, update the `send_handshake` method in `udp_relay.rs`.

## Security Considerations

⚠️ **Warning**: UDP relay can be used to proxy arbitrary traffic. Consider:

- **Access control** - Implement authentication/authorization
- **Rate limiting** - Prevent abuse of relay service  
- **Allowlist targets** - Restrict destination addresses
- **Monitoring** - Log and alert on suspicious patterns

### Example Access Control

```rust
// In udp_relay handler
const ALLOWED_PORTS: &[u16] = &[53, 123, 5060];

if !ALLOWED_PORTS.contains(&target_port) {
    return Response::error("Port not allowed", 403);
}
```

## Performance Tips

1. **Minimize round trips** - Batch queries when possible
2. **Use connection reuse** - Keep WebSocket open for multiple requests
3. **Implement client-side caching** - Cache DNS/query results
4. **Monitor latency** - UDP relay adds ~50-200ms overhead

## References

- [UDP Relay Service](https://hub.docker.com/r/kelvinzer0/udp-relay)
- [TURN Protocol (RFC 8656)](https://datatracker.ietf.org/doc/html/rfc8656)
- [Cloudflare Workers Limits](https://developers.cloudflare.com/workers/platform/limits/)
- [SOCKS5 Protocol (RFC 1928)](https://datatracker.ietf.org/doc/html/rfc1928)

## Support

For issues or questions:
- Open an issue on GitHub
- Check Worker logs with `wrangler tail`
- Test connectivity to `udp-relay.hobihaus.space:80`
