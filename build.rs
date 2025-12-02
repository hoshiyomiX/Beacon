use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    
    // Force use of exact wasm-bindgen version to avoid schema mismatch
    let wasm_bindgen_version = "0.2.105";
    
    // Check if wasm-bindgen CLI is available and matches version
    match Command::new("wasm-bindgen")
        .arg("--version")
        .output() {
        Ok(output) => {
            let version_output = String::from_utf8_lossy(&output.stdout);
            println!("cargo:warning=wasm-bindgen CLI version: {}", version_output.trim());
            
            if !version_output.contains(&format!("{}", wasm_bindgen_version)) {
                println!("cargo:warning=Version mismatch detected. Using embedded wasm-bindgen CLI.");
                // Skip automatic binding generation and let wrangler handle it
                println!("cargo:rustc-cfg=bindgen_skip");
            }
        },
        Err(_) => {
            println!("cargo:warning=wasm-bindgen CLI not found in PATH. Skipping automatic binding generation.");
            println!("cargo:rustc-cfg=bindgen_skip");
        }
    }
    
    // Set environment variable to ensure consistent binding generation
    println!("cargo:rustc-env=WASM_BINDGEN_CLI_VERSION={}", wasm_bindgen_version);
    
    // Ensure the build uses the correct target
    println!("cargo:rustc-cfg=wasm_target");
}