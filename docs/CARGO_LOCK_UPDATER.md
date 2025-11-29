# Cargo.lock Updater Workflow

## Purpose

This workflow fixes critical version mismatches between `Cargo.toml` and `Cargo.lock`, specifically targeting `wasm-bindgen` to ensure exact version synchronization.

## Why This Matters

### The Problem

wasm-bindgen requires **exact version matching** between:
- The Rust library version (in `Cargo.lock`)
- The CLI tool version (in build workflows)

When versions don't match, builds fail with:
```
error: it looks like the Rust project used to create this wasm file 
was linked against version of wasm-bindgen that uses a different 
bindgen format than this binary:

  rust wasm file schema version: 0.2.100
      this binary schema version: 0.2.105
```

### Current Issue (Beta Branch)

- `Cargo.toml` specifies: `wasm-bindgen = "0.2.105"`
- `Cargo.lock` resolved to: `wasm-bindgen = "0.2.100"`
- Workflow uses: `wasm-bindgen-cli --version 0.2.105`

**Result:** Build failures due to version mismatch.

## How to Use

### Method 1: GitHub Actions UI (Recommended)

1. Go to [Actions tab](https://github.com/hoshiyomiX/Beacon/actions)
2. Select "Cargo.lock Updater" from the workflows list
3. Click "Run workflow" button
4. Configure inputs (or use defaults):
   - **Package**: `wasm-bindgen` (default)
   - **Version**: `0.2.105` (default)
5. Click "Run workflow" to start

### Method 2: GitHub CLI

```bash
gh workflow run updater.yml \
  --ref beta \
  -f package=wasm-bindgen \
  -f version=0.2.105
```

### Method 3: API Call

```bash
curl -X POST \
  -H "Accept: application/vnd.github+json" \
  -H "Authorization: Bearer $GITHUB_TOKEN" \
  https://api.github.com/repos/hoshiyomiX/Beacon/actions/workflows/updater.yml/dispatches \
  -d '{"ref":"beta","inputs":{"package":"wasm-bindgen","version":"0.2.105"}}'
```

## What the Workflow Does

1. **Checkout beta branch** - Gets the latest code
2. **Setup Rust toolchain** - Installs stable Rust with wasm32 target
3. **Update dependencies** - Runs targeted `cargo update` commands:
   ```bash
   cargo update -p wasm-bindgen --precise 0.2.105
   cargo update -p wasm-bindgen-macro --precise 0.2.105
   cargo update -p wasm-bindgen-backend --precise 0.2.105
   cargo update -p wasm-bindgen-shared --precise 0.2.105
   cargo update -p wasm-bindgen-macro-support --precise 0.2.105
   ```
4. **Verify version** - Confirms Cargo.lock has correct version
5. **Test build** - Runs `cargo check` for wasm32 target
6. **Create PR** - Automatically creates a pull request if changes detected

## Workflow Outputs

### Successful Run

The workflow will:
- ✅ Update `Cargo.lock` with exact versions
- ✅ Verify build compatibility
- ✅ Create a pull request to beta branch
- ✅ Label PR with: `dependencies`, `critical`, `version-sync`, `automated`

### Pull Request Contents

The auto-created PR includes:
- Clear title: "🔧 Fix: Sync wasm-bindgen to 0.2.105 in Cargo.lock"
- Detailed explanation of changes
- Verification checklist
- Next steps for deployment

### No Changes Needed

If `Cargo.lock` already has the correct version:
- ℹ️ Workflow completes successfully
- ℹ️ No PR is created
- ℹ️ Summary confirms versions are in sync

## After Running the Workflow

### 1. Review the Pull Request

- Check the PR created by the workflow
- Review `Cargo.lock` changes
- Verify version numbers match expectations

### 2. Merge the PR

```bash
# Via GitHub UI
Click "Merge pull request" button

# Via GitHub CLI
gh pr merge <PR_NUMBER> --squash --delete-branch
```

### 3. Verify Deployment Workflow

Ensure `.github/workflows/cf.yml` uses matching versions:

```yaml
- name: Install build tools (cached)
  run: |
    if ! command -v wasm-bindgen &> /dev/null; then
      # Should match Cargo.lock version
      cargo install -f wasm-bindgen-cli --version 0.2.105
    fi
```

### 4. Test Deployment

After merging:
1. Monitor the `cf workers` workflow
2. Check for successful build
3. Verify deployment to Cloudflare Workers

## Troubleshooting

### Workflow Fails at "Update dependencies"

**Possible causes:**
- Network issues with crates.io
- Version doesn't exist
- Dependency conflicts

**Solution:**
```bash
# Manually check available versions
cargo search wasm-bindgen --limit 1

# Try updating locally first
cargo update -p wasm-bindgen
```

### Build Check Fails

**Possible causes:**
- Breaking changes in wasm-bindgen version
- Incompatible with worker 0.7.0
- Missing dependencies

**Solution:**
```bash
# Test locally
cargo check --target wasm32-unknown-unknown --release

# Check for errors
cargo build --target wasm32-unknown-unknown 2>&1 | grep error
```

### PR Not Created

**Possible causes:**
- No changes detected (version already correct)
- GitHub token lacks permissions
- Branch protection rules

**Solution:**
- Check workflow summary for "No changes detected"
- Verify `secrets.GITHUB_TOKEN` has write permissions
- Review branch protection settings

## Related Files

- Workflow file: `.github/workflows/updater.yml`
- Dependencies: `Cargo.toml`
- Lock file: `Cargo.lock`
- Deployment: `.github/workflows/cf.yml`
- Dependency updates: `.github/workflows/dependency-update.yml`

## Future Enhancements

- [ ] Auto-detect version from Cargo.toml
- [ ] Support multiple packages in single run
- [ ] Integration with dependency-update workflow
- [ ] Slack/Discord notifications on completion
- [ ] Auto-merge for low-risk updates

## Support

For issues or questions:
1. Check workflow run logs in Actions tab
2. Review this documentation
3. Open an issue with `workflow` label
4. Tag maintainers in PR if help needed

---

**Last Updated:** November 29, 2025  
**Workflow Version:** 1.0  
**Status:** Active
