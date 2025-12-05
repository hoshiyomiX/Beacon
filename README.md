# Beacon - Rust Cloudflare Workers Proxy

**Beacon** is a high-performance, secure proxy server built with Rust and deployed on Cloudflare Workers. It provides encrypted data tunneling with WebAssembly optimization for edge computing environments.

## What is Beacon?

Beacon acts as a secure intermediary for network traffic, running entirely on Cloudflare's global edge network. Built in Rust and compiled to WebAssembly (WASM), it delivers low-latency proxying with enterprise-grade encryption while staying within Cloudflare Workers' strict resource limits.

### Key Capabilities

- **Protocol Support**: VLESS, TCP tunneling, WebSocket connections
- **Edge Computing**: Runs on 300+ Cloudflare data centers worldwide
- **Security First**: AES-GCM encryption, SHA-256 integrity verification
- **WASM Optimized**: Sub-1MB binary size for instant cold starts
- **Production Ready**: Comprehensive error handling and WASM integrity validation

## Project Structure

```
Beacon/
├── src/                    # Core Rust application
│   ├── lib.rs             # Main entry point and request router
│   ├── config.rs          # Configuration management
│   ├── proxy/             # Proxy protocol implementations
│   └── common/            # Shared utilities and helpers
├── web/                   # Frontend assets (if applicable)
├── scripts/               # Build and deployment automation
├── .github/workflows/     # CI/CD pipelines
├── Cargo.toml             # Rust dependencies and build configuration
├── wrangler.toml          # Cloudflare Workers configuration
└── Makefile               # Convenient build targets
```

## Major Improvements from Original Fork

This fork has undergone significant enhancements for production stability, developer experience, and Cloudflare Workers compatibility:

### 🔧 Build System & Tooling

- **Wrangler v4 Upgrade**: Migrated to latest Wrangler for improved deployment reliability
- **WASM Integrity Checks**: Added comprehensive validation pipeline
  - Magic number verification (`0x6d736100`)
  - Version validation (`0x01000000`)
  - File size compliance (1MB/10MB limits)
  - Section header validation
  - Multi-file corruption detection
- **Pinned Dependencies**: Locked `wasm-bindgen` to `0.2.106` for consistent builds across environments
- **Optimized Build Profile**: Enhanced `Cargo.toml` with size-focused optimization (`opt-level = "z"`, LTO, single codegen unit)

### 🐛 Critical Bug Fixes

- **WASM Instance Lifecycle**: Fixed "Cannot invoke closure from previous WASM instance" errors
  - Removed `spawn_local` fire-and-forget patterns
  - Ensured WebSocket processing stays within request context
  - Proper closure lifecycle management for Workers' isolate reuse
- **WebSocket Stability**: Resolved "script will never generate a response" runtime errors
  - Added proper error handling for WebSocket handshake failures
  - Prevented unresolved Promises causing Worker hangs
- **Error Propagation**: Fixed false-positive SUCCESS logs with proper error classification
  - Benign errors (timeouts, disconnects) silenced from Cloudflare logs
  - Fatal errors properly propagated for debugging
  - Enhanced failover logic for VLESS protocol

### 🛡️ Code Quality Improvements

- **Eliminated Panics**: Replaced all `.unwrap()` and `.expect()` with proper error handling using `?` operator and `Result` types
- **Ownership Safety**: Added `Rc<WebSocket>` for safe sharing across closures without lifetime errors
- **Warning-Free Builds**: Cleaned up unused variables, dead code annotations, false-positive warnings
- **Clone Derivation**: Made `Config` cloneable for safe sharing in async contexts

### 🔬 Developer Experience

- **CI/CD Enhancements**: 
  - Automated Cargo.lock regeneration workflow
  - Build artifact cleaning strategies
  - Version verification outputs for debugging
  - Harmless warning suppression for cleaner logs
- **Makefile Targets**: Simple `make build`, `make deploy`, `make deploy-prod` commands
- **Documentation**: Inline comments explaining WASM-specific quirks and Cloudflare Workers constraints

### 🎯 Performance Optimizations

- **Binary Size Reduction**: Aggressive optimization for Workers' 1MB compressed limit
  - Strip debug symbols in release mode
  - Link-time optimization (LTO)
  - Panic abort strategy (smaller binary)
- **WASM-Compatible Dependencies**: All crates configured for `wasm32-unknown-unknown` target
  - Tokio with minimal I/O features (no runtime)
  - Reqwest with JSON-only support
  - `getrandom` with JS feature flag

## Quick Start

### Prerequisites

- **Rust 1.70+** with `wasm32-unknown-unknown` target
- **Node.js 18+** (for Wrangler)
- **Cloudflare Workers account** with API token

### Installation

```bash
# Install Rust target
rustup target add wasm32-unknown-unknown

# Install worker-build (WASM compilation tool)
cargo install worker-build

# Clone repository
git clone https://github.com/hoshiyomiX/Beacon.git
cd Beacon
```

### Build & Deploy

```bash
# Build for Cloudflare Workers
make build

# Deploy to testing environment
make deploy

# Deploy to production
make deploy-prod
```

### Manual Build (Advanced)

```bash
# Compile to WASM
cargo build --target wasm32-unknown-unknown --release

# Generate JavaScript bindings
wasm-bindgen target/wasm32-unknown-unknown/release/beacon.wasm \
  --out-dir target/wasm-bindgen \
  --target web

# Deploy with Wrangler
wrangler deploy
```

## Configuration

Edit `wrangler.toml` to customize deployment settings:

```toml
name = "beacon"
main = "build/worker/shim.mjs"
compatibility_date = "2024-01-01"

[vars]
# Add environment variables here
```

Configuration is loaded from Cloudflare Workers environment variables at runtime.

## Security Features

Beacon implements multiple layers of security:

- **AES-GCM Encryption**: Authenticated encryption for data confidentiality and integrity
- **SHA-256 Hashing**: Cryptographic checksums for tamper detection  
- **MD5 Support**: Legacy hash support for compatibility
- **UUID v4 Generation**: Cryptographically secure unique identifiers
- **Base64 Encoding**: Safe binary data transmission in text protocols

All cryptographic operations use WASM-compatible implementations with JavaScript fallbacks where needed (e.g., `getrandom` with `js` feature).

## Troubleshooting

### WASM Build Errors

**Error**: `error: failed to parse manifest at .../Cargo.toml`
- **Solution**: Ensure `wasm-bindgen` versions match between `Cargo.toml` (0.2.106) and `wasm-bindgen-cli`

**Error**: `Cannot invoke closure from previous WASM instance`
- **Solution**: Already fixed in this fork. Ensure you're using the latest code from `testing` branch.

### Deployment Issues

**Error**: `script will never generate a response`
- **Solution**: Check WebSocket error handling is present (fixed in commit `67cc59d`)

**Warning**: `wrangler out-of-date`
- **Solution**: CI/CD automatically uses Wrangler v4. Ignore or update local installation.

### Build Size Exceeds Limits

If WASM binary exceeds 1MB (Workers free tier) or 10MB (paid tier):
1. Verify release profile settings in `Cargo.toml`
2. Run `cargo clean` before building
3. Check for accidentally included debug symbols (`strip = true`)

## Technical Architecture

### Request Flow

1. **Cloudflare Edge** receives incoming request
2. **WASM Router** (`lib.rs`) parses protocol and headers
3. **Protocol Handler** (VLESS/TCP/WebSocket) processes connection
4. **Proxy Module** establishes upstream connection
5. **Bidirectional Tunnel** streams encrypted data
6. **Response** returned to client via Cloudflare network

### Key Design Decisions

- **No Runtime Dependencies**: Tokio used only for I/O traits, not async runtime (WASM incompatible)
- **Closure Lifecycle**: All async closures await within request context to avoid isolate reuse issues
- **Error Classification**: Benign errors (client disconnects) silenced; fatal errors (bugs) logged
- **Size-First Optimization**: Every feature weighed against binary size impact

## CI/CD Pipeline

Automated workflows on every push:

1. **Lint & Test**: `cargo clippy`, `cargo test`
2. **Build WASM**: Compile with release optimizations
3. **Verify Integrity**: Custom scripts validate WASM structure
4. **Deploy**: Automatic deployment to testing/production environments
5. **Lock Files**: Auto-regenerate `Cargo.lock` on dependency changes

See `.github/workflows/` for complete pipeline definitions.

## Resources

- [Cloudflare Workers Documentation](https://developers.cloudflare.com/workers/)
- [Rust WASM Book](https://rustwasm.github.io/docs/book/)
- [worker-rs Framework](https://github.com/cloudflare/workers-rs)
- [wasm-bindgen Guide](https://rustwasm.github.io/wasm-bindgen/)

## Contributing

This is a personal fork with active development. Key areas for contribution:

- Protocol implementations (Shadowsocks, Trojan, etc.)
- Performance benchmarking
- Documentation improvements
- Additional WASM optimization techniques

## License

Check original project license. Modifications in this fork maintain the same licensing terms.

---

**Project Status**: Active development on `testing` branch. Production-ready features merge to `main` after validation.