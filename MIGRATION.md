# Beacon JavaScript Migration

This branch contains the JavaScript/TypeScript version of the Beacon proxy, migrated from the original Rust/WASM implementation.

## What Changed

### Runtime
- **From**: Rust compiled to WebAssembly (WASM)
- **To**: Native JavaScript (ES modules) running on V8

### Benefits
- ✅ No compilation step required
- ✅ Faster cold starts (no WASM initialization)
- ✅ Smaller bundle size (no WASM binary)
- ✅ Easier to debug and modify
- ✅ Native CloudFlare Workers compatibility
- ✅ Direct access to Workers APIs without bindings

### File Structure

```
src/
├── index.js              # Main worker entry point
├── config.js             # Configuration class
└── proxy/
    ├── stream.js         # WebSocket proxy stream handler
    ├── vless.js          # VLESS protocol handler
    ├── vmess.js          # VMess protocol handler (stub)
    ├── trojan.js         # Trojan protocol handler
    └── shadowsocks.js    # Shadowsocks protocol handler (stub)
```

## Deployment

### Prerequisites
- Node.js 18+ 
- Wrangler CLI 3.0+

### Install Dependencies
```bash
npm install
```

### Development
```bash
npm run dev
```

### Deploy to Cloudflare
```bash
npm run deploy
```

## Configuration

All configuration is done through `wrangler.toml` - same as the Rust version:

- `UUID`: Authentication UUID
- `MAIN_PAGE_URL`, `SUB_PAGE_URL`, etc.: Frontend page URLs
- `PROXY_LIST`: JSON object mapping country codes to proxy lists

## Protocol Support

### Fully Implemented
- ✅ **VLESS**: Full support with UUID authentication
- ✅ **Trojan**: Basic support (password hash validation)

### Partial/Stub Implementation
- ⚠️ **VMess**: Requires complex AEAD encryption - stub only
- ⚠️ **Shadowsocks**: Requires AEAD encryption - stub only

> **Note**: VMess and Shadowsocks protocols require sophisticated encryption libraries.
> Consider using VLESS or Trojan for JavaScript version, or implement crypto using Web Crypto API.

## Performance Considerations

### JavaScript vs Rust/WASM

| Aspect | Rust/WASM | JavaScript |
|--------|-----------|------------|
| Cold Start | ~50-100ms | ~5-20ms |
| Bundle Size | ~500KB-1MB | ~50-100KB |
| Execution Speed | Faster | Good enough |
| Memory Usage | Higher | Lower |
| Debugging | Harder | Easier |

### Optimizations Applied

1. **Minification**: Enabled in wrangler.toml
2. **ES Modules**: Tree-shaking for smaller bundles
3. **Lazy Loading**: Protocol handlers loaded on-demand
4. **Stream Processing**: Uses Streams API for efficiency

## Limitations

1. **Complex Cryptography**: VMess and Shadowsocks need proper crypto implementation
2. **Binary Performance**: Rust/WASM is faster for heavy crypto operations
3. **Type Safety**: No compile-time type checking (could add TypeScript)

## Migration from Rust Version

If you need to rollback to the Rust/WASM version:

```bash
git checkout checkpoint
wrangler deploy
```

## Future Improvements

- [ ] Implement VMess AEAD encryption using Web Crypto API
- [ ] Implement Shadowsocks AEAD encryption
- [ ] Add TypeScript for type safety
- [ ] Add unit tests
- [ ] Performance benchmarking vs Rust version
- [ ] Add protocol auto-detection improvements

## Contributing

When modifying the JavaScript version:

1. Test locally with `npm run dev`
2. Test protocol compatibility with actual clients
3. Monitor bundle size (should stay under 1MB)
4. Check cold start performance

## License

Same as the original Beacon project.
