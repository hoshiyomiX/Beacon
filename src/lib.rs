mod common;
mod config;
mod proxy;

use crate::config::Config;
use crate::proxy::*;

use std::collections::HashMap;
use uuid::Uuid;
use worker::*;
use once_cell::sync::Lazy;
use regex::Regex;

// Regex compilation at initialization - only fails at compile time, safe to unwrap
static PROXYIP_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^.+-\d+$").unwrap());
static PROXYKV_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Z]{2})").unwrap());

/// Check if an error is benign (expected during normal operation)
fn is_benign_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    error_lower.contains("writablestream has been closed")
        || error_lower.contains("broken pipe")
        || error_lower.contains("connection reset")
        || error_lower.contains("connection closed")
        || error_lower.contains("network connection lost")
        || error_lower.contains("stream closed")
        || error_lower.contains("eof")
        || error_lower.contains("connection aborted")
        || error_lower.contains("transfer error")
        || error_lower.contains("canceled")
        || error_lower.contains("cancelled")
        || error_lower.contains("benign")
        || error_lower.contains("not enough buffer")
        || error_lower.contains("websocket")
        || error_lower.contains("handshake")
        || error_lower.contains("hung")
        || error_lower.contains("never generate")
}

#[event(fetch)]
async fn main(req: Request, env: Env, _: Context) -> Result<Response> {
    // Parse UUID with proper error handling
    let uuid = match env.var("UUID") {
        Ok(uuid_var) => {
            match Uuid::parse_str(&uuid_var.to_string()) {
                Ok(parsed) => parsed,
                Err(_) => {
                    console_error!("[ERROR] Invalid UUID format in environment variable");
                    return Response::error("Invalid server configuration: UUID", 502);
                }
            }
        }
        Err(_) => {
            console_error!("[ERROR] UUID environment variable not found");
            return Response::error("Server configuration error: Missing UUID", 502);
        }
    };
    
    let host = match req.url() {
        Ok(url) => url.host().map(|x| x.to_string()).unwrap_or_default(),
        Err(_) => {
            console_error!("[ERROR] Failed to parse request URL");
            return Response::error("Invalid request URL", 400);
        }
    };
    
    // Parse environment variables with proper error handling
    let main_page_url = match env.var("MAIN_PAGE_URL") {
        Ok(val) => val.to_string(),
        Err(_) => {
            console_error!("[ERROR] MAIN_PAGE_URL not configured");
            return Response::error("Server configuration error: Missing MAIN_PAGE_URL", 502);
        }
    };
    
    let sub_page_url = match env.var("SUB_PAGE_URL") {
        Ok(val) => val.to_string(),
        Err(_) => {
            console_error!("[ERROR] SUB_PAGE_URL not configured");
            return Response::error("Server configuration error: Missing SUB_PAGE_URL", 502);
        }
    };
    
    let link_page_url = match env.var("LINK_PAGE_URL") {
        Ok(val) => val.to_string(),
        Err(_) => {
            console_error!("[ERROR] LINK_PAGE_URL not configured");
            return Response::error("Server configuration error: Missing LINK_PAGE_URL", 502);
        }
    };
    
    let converter_page_url = match env.var("CONVERTER_PAGE_URL") {
        Ok(val) => val.to_string(),
        Err(_) => {
            console_error!("[ERROR] CONVERTER_PAGE_URL not configured");
            return Response::error("Server configuration error: Missing CONVERTER_PAGE_URL", 502);
        }
    };
    
    let checker_page_url = match env.var("CHECKER_PAGE_URL") {
        Ok(val) => val.to_string(),
        Err(_) => {
            console_error!("[ERROR] CHECKER_PAGE_URL not configured");
            return Response::error("Server configuration error: Missing CHECKER_PAGE_URL", 502);
        }
    };

    let config = Config { 
        uuid, 
        host: host.clone(), 
        proxy_addr: host, 
        proxy_port: 443, 
        main_page_url, 
        sub_page_url,
        link_page_url,
        converter_page_url,
        checker_page_url
    };

    Router::with_data(config)
        .on_async("/", fe)
        .on_async("/sub", sub)
        .on_async("/link", link)
        .on_async("/converter", converter)
        .on_async("/checker", checker)
        .on_async("/:proxyip", tunnel)
        .on_async("/Geo-Project/:proxyip", tunnel)
        .run(req, env)
        .await
}

async fn get_response_from_url(url: String) -> Result<Response> {
    let req = Fetch::Url(Url::parse(url.as_str())?);
    let mut res = req.send().await?;
    Response::from_html(res.text().await?)
}

async fn fe(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    get_response_from_url(cx.data.main_page_url).await
}

async fn sub(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    get_response_from_url(cx.data.sub_page_url).await
}

async fn link(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    get_response_from_url(cx.data.link_page_url).await
}

async fn converter(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    get_response_from_url(cx.data.converter_page_url).await
}

async fn checker(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    get_response_from_url(cx.data.checker_page_url).await
}

async fn tunnel(req: Request, mut cx: RouteContext<Config>) -> Result<Response> {
    // Wrap entire function to catch WebSocket handshake errors
    let result = tunnel_inner(req, &mut cx).await;
    
    // Suppress benign errors before they reach Cloudflare logs
    match result {
        Ok(response) => Ok(response),
        Err(e) => {
            let error_msg = e.to_string();
            if is_benign_error(&error_msg) {
                // Return a simple response instead of propagating error
                Response::ok("Connection closed")
            } else {
                // Only unexpected errors propagate
                console_error!("[FATAL] Unexpected tunnel error: {}", error_msg);
                Err(e)
            }
        }
    }
}

async fn tunnel_inner(req: Request, cx: &mut RouteContext<Config>) -> Result<Response> {
    let proxyip_param = match cx.param("proxyip") {
        Some(param) => param.to_string(),
        None => {
            console_error!("[ERROR] Missing proxyip parameter");
            return Response::error("Missing proxy parameter", 400);
        }
    };
    
    let mut proxyip = proxyip_param;
    
    // Handle proxy selection from bundled list
    if PROXYKV_PATTERN.is_match(&proxyip) {
        let kvid_list: Vec<String> = proxyip.split(",").map(|s| s.to_string()).collect();
        
        // Get bundled proxy list from environment variables
        let proxy_list_json = cx.env.var("PROXY_LIST")
            .map(|x| x.to_string())
            .unwrap_or_else(|_| "{}".to_string());
        
        // Parse proxy list (no KV or external fetch needed!)
        let proxy_kv: HashMap<String, Vec<String>> = match serde_json::from_str(&proxy_list_json) {
            Ok(map) => map,
            Err(e) => {
                console_error!("[ERROR] Invalid PROXY_LIST configuration: {}", e);
                return Response::error("Invalid server configuration: PROXY_LIST", 502);
            }
        };
        
        // Random selection logic with proper error handling
        let mut rand_buf = [0u8, 1];
        match getrandom::getrandom(&mut rand_buf) {
            Ok(_) => {},
            Err(e) => {
                console_error!("[ERROR] Failed to generate random number: {}", e);
                return Response::error("Server error: Random generation failed", 500);
            }
        }
        
        let kv_index = (rand_buf[0] as usize) % kvid_list.len();
        proxyip = kvid_list[kv_index].clone();
        
        // Select random proxy from the country list
        if let Some(proxy_list) = proxy_kv.get(&proxyip) {
            if !proxy_list.is_empty() {
                let proxyip_index = (rand_buf[0] as usize) % proxy_list.len();
                proxyip = proxy_list[proxyip_index].clone().replace(":", "-");
            } else {
                console_error!("[ERROR] No proxies available for country: {}", &proxyip);
                return Response::error("No proxies available for selected region", 502);
            }
        } else {
            console_error!("[ERROR] Country code not found: {}", &proxyip);
            return Response::error("Invalid country code", 400);
        }
    }

    if PROXYIP_PATTERN.is_match(&proxyip) {
        if let Some((addr, port_str)) = proxyip.split_once('-') {
            if let Ok(port) = port_str.parse() {
                cx.data.proxy_addr = addr.to_string();
                cx.data.proxy_port = port;
            }
        }
    }

    let upgrade = match req.headers().get("Upgrade") {
        Ok(Some(val)) => val,
        Ok(None) => String::new(),
        Err(e) => {
            console_error!("[ERROR] Failed to read Upgrade header: {}", e);
            return Response::error("Invalid request headers", 400);
        }
    };
    
    if upgrade == "websocket".to_string() {
        let WebSocketPair { server, client } = match WebSocketPair::new() {
            Ok(pair) => pair,
            Err(e) => {
                console_error!("[ERROR] Failed to create WebSocket pair: {}", e);
                return Response::error("WebSocket initialization failed", 500);
            }
        };
        
        // Clone config for the spawned task
        let config = cx.data.clone();
        
        // Spawn WebSocket processing in fire-and-forget mode with best-effort error handling
        wasm_bindgen_futures::spawn_local(async move {
            use gloo_timers::future::TimeoutFuture;

            // Accept connection; ignore errors (Cloudflare will close the socket)
            let _ = server.accept();

            // Get events; if this fails, nothing to do
            let events = match server.events() {
                Ok(ev) => ev,
                Err(_) => {
                    let _ = server.close(Some(1000), Some("Connection closed".to_string()));
                    return;
                }
            };

            // Process proxy stream with timeout; ignore processing errors as they are already classified as benign/non-benign inside ProxyStream
            let process_future = async {
                let _ = ProxyStream::new(config, &server, events).process().await;
            };

            // REDUCED: 5-second timeout (from 8s) to minimize CPU time accumulation
            // Free tier has 10ms CPU limit; shorter timeout reduces accumulation risk
            let timeout = TimeoutFuture::new(5_000);
            futures_util::pin_mut!(process_future);
            
            match futures_util::future::select(process_future, timeout).await {
                futures_util::future::Either::Left(_) => {
                    let _ = server.close(Some(1000), Some("Normal closure".to_string()));
                },
                futures_util::future::Either::Right(_) => {
                    let _ = server.close(Some(1000), Some("Connection closed".to_string()));
                }
            }
        });

        // CRITICAL FIX: Handle Response::from_websocket() errors to prevent hung workers
        // If this fails (e.g., client disconnected), we must return a proper error response
        // instead of leaving an unresolved Promise that hangs the worker
        Response::from_websocket(client).or_else(|e| {
            console_log!("[DEBUG] WebSocket response creation failed: {}", e);
            Response::error("WebSocket handshake failed", 400)
        })
    } else {
        Response::from_html("hi from wasm!")
    }
}
