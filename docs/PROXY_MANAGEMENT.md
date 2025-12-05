# Proxy List Management

## Overview

Beacon now uses a **bundled proxy list** stored in `wrangler.toml` instead of fetching from external sources. This eliminates:
- External GitHub dependency (FoolVPN-ID/Nautica)
- KV storage read/write operations (~365 writes/year)
- Network latency and failure risks
- Race conditions during cache misses

## How to Update Proxy List

### 1. Edit wrangler.toml

Locate the `PROXY_LIST` variable in `wrangler.toml`:

```toml
[vars]
PROXY_LIST = '{"US":["1.2.3.4:443","5.6.7.8:443"],"SG":["10.0.0.1:443"]}'
```

### 2. JSON Format

The proxy list uses this structure:

```json
{
  "COUNTRY_CODE": ["ip:port", "ip:port"],
  "COUNTRY_CODE": ["ip:port"]
}
```

**Country Codes** (ISO 3166-1 alpha-2):
- `US` - United States
- `SG` - Singapore
- `JP` - Japan
- `ID` - Indonesia
- `GB` - United Kingdom
- `DE` - Germany
- `FR` - France
- `AU` - Australia
- `CA` - Canada
- `IN` - India

### 3. Example Configuration

```toml
PROXY_LIST = '{"US":["104.21.45.67:443","172.67.89.12:443"],"SG":["203.0.113.45:443","198.51.100.89:443"],"JP":["192.0.2.123:443"]}'
```

### 4. Deploy Changes

After editing `wrangler.toml`, deploy the worker:

```bash
# Deploy to production
wrangler deploy

# Or deploy to testing environment
wrangler deploy --env testing
```

## Important Notes

### ✅ Benefits
- **No external dependencies**: Works even if GitHub is down
- **Zero KV writes**: Stays within free tier limits (1,000 writes/day)
- **Instant availability**: No cache warming needed
- **No race conditions**: No concurrent fetch issues
- **Lower latency**: No external HTTP calls

### ⚠️ Considerations
- **Requires redeployment**: Proxy list changes need `wrangler deploy`
- **Worker size limit**: Each proxy entry adds ~20-30 bytes
  - Free tier: 3 MB worker size limit
  - Can store ~10,000+ proxy entries comfortably
- **Single quotes in TOML**: Use `'` around JSON string in `wrangler.toml`

### 📝 JSON Validation

Before deploying, validate your JSON:

```bash
# Extract and validate PROXY_LIST
grep 'PROXY_LIST' wrangler.toml | cut -d"'" -f2 | jq .
```

### 🔍 Testing Locally

```bash
# Test with wrangler dev
wrangler dev

# Test proxy selection (replace with your domain)
curl -H "Upgrade: websocket" https://your-worker.workers.dev/US
```

## Migration from Old System

If you were using the old GitHub fetch method:

1. Get your current proxy list from:
   - KV storage: `wrangler kv:key get proxy_kv --binding=library`
   - Or from: `https://raw.githubusercontent.com/FoolVPN-ID/Nautica/refs/heads/main/kvProxyList.json`

2. Format it as a single-line JSON string

3. Add it to `wrangler.toml` under `PROXY_LIST`

4. Deploy: `wrangler deploy`

5. Old KV data will be ignored (can be deleted to save storage)

## Troubleshooting

### "Country code not found" error
- Check that country code exists in `PROXY_LIST`
- Verify JSON syntax is valid
- Ensure single quotes wrap the JSON string in TOML

### "Invalid PROXY_LIST configuration" error
- JSON parsing failed
- Validate JSON syntax: `echo 'YOUR_JSON' | jq .`
- Check for unescaped quotes or invalid characters

### Worker deployment fails
- Check worker size: `wrangler deploy --dry-run`
- If too large, reduce proxy list or split by regions

## Best Practices

1. **Keep it organized**: Group proxies by region
2. **Regular updates**: Review proxy list monthly
3. **Test before deploy**: Use `wrangler dev` locally
4. **Version control**: Commit `wrangler.toml` changes with clear messages
5. **Backup**: Keep a copy of working proxy lists

## Free Tier Compliance

✅ **Current Usage**:
- KV Reads: 0/day (was ~100+/day)
- KV Writes: 0/day (was ~1/day + cache storms)
- External Requests: 0/request (was 1/request on cache miss)
- Worker Size: ~2 KB + proxy list size

✅ **Free Tier Limits**:
- 100,000 requests/day ✓
- 1,000 KV writes/day ✓ (using 0)
- 3 MB worker size ✓
- 10ms CPU time/request ✓ (with 8s I/O timeout)
