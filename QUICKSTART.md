# Quick Start Guide - Beacon JavaScript Edition

## 🚀 3 Ways to Deploy

### Method 1: Auto-Deploy via Git Push (Recommended)

**One-time setup:**
1. Go to repository **Settings** → **Secrets and variables** → **Actions**
2. Add secret: `CLOUDFLARE_API_TOKEN` (get from [Cloudflare Dashboard](https://dash.cloudflare.com/profile/api-tokens))

**Deploy:**
```bash
# Just push to js-migration branch
git push origin js-migration

# GitHub Actions will automatically deploy!
```

### Method 2: Manual Script (Simple & Fast)

```bash
# Make script executable (first time only)
chmod +x deploy.sh

# Run deployment
./deploy.sh
```

The script will:
- ✅ Check Node.js installation
- ✅ Install dependencies
- ✅ Validate JavaScript syntax
- ✅ Check bundle size
- ✅ Deploy to Cloudflare Workers

### Method 3: Direct npm Command

```bash
# Install dependencies (first time only)
npm install

# Deploy
npm run deploy
```

---

## 🔧 Local Development

```bash
# Install dependencies
npm install

# Run locally
npm run dev

# Test at http://localhost:8787
```

---

## ⚠️ GitHub Actions "Run Workflow" Button Not Visible?

**Why this happens:**
GitHub only shows the "Run workflow" button for workflows on the **default branch** (usually `master` or `main`).

**Solution 1: Use Git Push (Easiest)**
```bash
# Any push to js-migration triggers deployment
git commit --allow-empty -m "Trigger deployment"
git push origin js-migration
```

**Solution 2: Use deploy.sh Script**
```bash
./deploy.sh
```

**Solution 3: Merge to Default Branch**
```bash
# If you want the workflow visible in UI
git checkout master
git merge js-migration
git push origin master

# Then the "Run workflow" button will appear
```

**Solution 4: View Workflow from js-migration Branch**
1. Go to: `https://github.com/hoshiyomiX/Beacon/actions/workflows/deploy-js.yml`
2. Switch branch selector to `js-migration`
3. Click "Run workflow" (should now be visible)

---

## ⚙️ Configuration

All settings are in `wrangler.toml`:

```toml
# Main configuration
name = "beacon"
main = "src/index.js"  # JavaScript entry point

# Your authentication UUID
UUID = "38425afe-8466-4876-8223-f3d604ca3c18"

# Page URLs
MAIN_PAGE_URL = "https://raw.githubusercontent.com/..."

# Proxy list (country codes to IP:port mappings)
PROXY_LIST = '{"HK":[...], "SG":[...]}'
```

### Update Configuration

```bash
# Edit wrangler.toml
nano wrangler.toml  # or use your preferred editor

# Deploy changes
./deploy.sh
# or
git add wrangler.toml
git commit -m "Update configuration"
git push origin js-migration
```

---

## 📊 Monitoring

### View Live Logs
```bash
npm run tail
# or
npx wrangler tail
```

### Check Deployment Status
```bash
# List recent deployments
npx wrangler deployments list

# View worker details
npx wrangler status
```

### Check from GitHub
- Go to **Actions** tab
- View latest workflow runs
- Check deployment logs

---

## 🐛 Troubleshooting

### "No module found" Error
```bash
# Reinstall dependencies
rm -rf node_modules package-lock.json
npm install
```

### "Syntax error" in Deployment
```bash
# Check syntax locally
node --check src/index.js
node --check src/proxy/*.js

# Or use the deploy script which validates automatically
./deploy.sh
```

### "API Token Invalid"
```bash
# Check your token has Workers edit permissions
# Create new token at: https://dash.cloudflare.com/profile/api-tokens
# Use "Edit Cloudflare Workers" template
```

### "Bundle too large"
```bash
# Check current size
find src -name "*.js" -exec wc -c {} + | awk '{sum+=$1} END {print sum/1024 " KB"}'

# JavaScript version should be ~50-100KB
# If larger, check for unnecessary dependencies
```

### Deployment Stuck/Failed
```bash
# Try manual deployment
./deploy.sh

# Or direct wrangler command
npx wrangler deploy --verbose
```

---

## 🔄 Rollback to Rust/WASM

If you need to go back to the Rust version:

```bash
# Switch to Rust branch
git checkout checkpoint

# Deploy Rust version
wrangler deploy

# Or push to master to trigger Rust workflow
git push origin checkpoint:master
```

---

## ✨ Quick Commands Reference

```bash
# Deploy (multiple options)
./deploy.sh                    # Using script
npm run deploy                 # Using npm
git push origin js-migration   # Auto-deploy via GitHub

# Development
npm run dev                    # Run locally
npm run tail                   # View live logs

# Validation
node --check src/index.js      # Check syntax
npx wrangler validate          # Validate config

# Monitoring
npx wrangler deployments list  # List deployments
npx wrangler tail              # Live logs
npx wrangler status            # Worker status
```

---

## 🎯 Features

### ✅ Working
- VLESS protocol (full support)
- Trojan protocol (basic support)
- Country-based proxy selection
- Multiple custom domains
- WebSocket tunneling
- Multiple deployment methods

### ⚠️ Limitations
- VMess: Stub only (needs crypto)
- Shadowsocks: Stub only (needs AEAD)

---

## 📚 Learn More

- **Full Documentation**: [MIGRATION.md](MIGRATION.md)
- **Original Version**: `checkpoint` branch
- **Cloudflare Docs**: https://developers.cloudflare.com/workers/

---

## 🎉 You're All Set!

**Fastest way to deploy right now:**
```bash
./deploy.sh
```

Your Beacon proxy will be live in ~30 seconds! 🚀
