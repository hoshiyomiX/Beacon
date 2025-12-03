# Beacon Project - Rust Cloudflare Worker

Beacon is a Rust-based Cloudflare Workers application for secure, encrypted data processing.

## Project Structure
- **src/**: Core Rust logic
- **web/**: Frontend assets
- **scripts/**: Build and deployment scripts

## Build & Deploy

```bash
# Build for Cloudflare Workers
make build

# Deploy to testing environment
make deploy

# View production deployment
make deploy-prod
```

## Dependencies

The project uses:
- **Rust 1.70+** with wasm32-unknown-unknown target
- **wasm-bindgen** for WebAssembly bindings
- **worker** framework for Cloudflare Workers
- **reqwest** for HTTP requests (WASM-compatible)

## Security Features

- **AES-GCM encryption** for data protection
- **SHA-256 hashing** for integrity verification
- **UUID generation** for unique identifiers
- **Base64 encoding** for secure data transmission

## Development

Run locally with:

```bash
# Install dependencies
cargo install worker-build

# Build and test
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/beacon.wasm --out-dir target/wasm-bindgen --target web
```

See the [Cloudflare Workers documentation](https://developers.cloudflare.com/workers/) for more details.