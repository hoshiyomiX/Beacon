#!/bin/bash

# Beacon JavaScript Deployment Script
# Simple script to deploy to Cloudflare Workers

set -e  # Exit on error

echo "====================================="
echo "🚀 Beacon JavaScript Deployment"
echo "====================================="
echo ""

# Check if Node.js is installed
if ! command -v node &> /dev/null; then
    echo "❌ Error: Node.js is not installed"
    echo "Please install Node.js 18+ from https://nodejs.org/"
    exit 1
fi

echo "✅ Node.js version: $(node --version)"

# Check if npm is installed
if ! command -v npm &> /dev/null; then
    echo "❌ Error: npm is not installed"
    exit 1
fi

echo "✅ npm version: $(npm --version)"
echo ""

# Install dependencies if needed
if [ ! -d "node_modules" ]; then
    echo "Installing dependencies..."
    npm install
    echo "✅ Dependencies installed"
else
    echo "✅ Dependencies already installed"
fi
echo ""

# Validate JavaScript syntax
echo "Validating JavaScript files..."
JS_FILES=$(find src -name "*.js" -type f)
ERROR_COUNT=0

for file in $JS_FILES; do
    if ! node --check "$file" 2>&1; then
        echo "❌ Syntax error in $file"
        ERROR_COUNT=$((ERROR_COUNT + 1))
    fi
done

if [ $ERROR_COUNT -gt 0 ]; then
    echo "❌ Found $ERROR_COUNT file(s) with syntax errors"
    exit 1
fi

echo "✅ All JavaScript files are valid"
echo ""

# Check bundle size
echo "Checking bundle size..."
JS_SIZE=$(find src -name "*.js" -type f -exec stat -c%s {} + 2>/dev/null | awk '{sum+=$1} END {print sum}' || \
          find src -name "*.js" -type f -exec stat -f%z {} + 2>/dev/null | awk '{sum+=$1} END {print sum}')

if [ -n "$JS_SIZE" ]; then
    JS_SIZE_KB=$((JS_SIZE / 1024))
    echo "✅ Bundle size: ${JS_SIZE_KB} KB"
fi
echo ""

# Deploy
echo "====================================="
echo "Deploying to Cloudflare Workers..."
echo "====================================="
echo ""

npx wrangler@3 deploy

echo ""
echo "====================================="
echo "🎉 DEPLOYMENT COMPLETE!"
echo "====================================="
echo ""
echo "Your Beacon proxy is now live!"
echo ""
echo "Routes:"
echo "  - hoshiyomi.qzz.io"
echo "  - ava.game.naver.com.hoshiyomi.qzz.io"
echo "  - df.game.naver.com.hoshiyomi.qzz.io"
echo "  - graph.instagram.com.hoshiyomi.qzz.io"
echo "  - (and more...)"
echo ""
echo "To view logs: npm run tail"
echo "====================================="
