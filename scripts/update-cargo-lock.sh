#!/usr/bin/env bash

# Cargo.lock Updater Script
# Usage: ./update-cargo-lock.sh [package]

set -e

# Update all or specific package
if [ -n "$1" ]; then
    echo "Updating $1..."
    cargo update $1
else
    echo "Updating all dependencies..."
    cargo update
fi

# Check for breaking changes
cargo check

# Regenerate Cargo.lock
cargo generate-lockfile --lockfile Cargo.lock

# Verify no breaking changes
cargo build --release

# Commit changes
git add Cargo.lock Cargo.toml
if git diff --staged --quiet; then
    echo "No changes to commit."
else
    git commit -m "chore: update Cargo.lock $(date +%Y-%m-%d)"
    echo "Committed Cargo.lock updates."
fi

# Optional: Run tests
if command -v cargo &> /dev/null && [ -f Cargo.toml ]; then
    cargo test
    echo "Tests passed. Ready for deployment."
else
    echo "No tests to run."
fi
