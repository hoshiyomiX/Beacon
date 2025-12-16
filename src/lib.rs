mod common;
mod config;
mod proxy;

use crate::config::Config;
use crate::proxy::*;

use std::collections::HashMap;
use uuid::Uuid;
use worker::*;

/// Check if an error is benign (expected during normal operation)
#[inline(always)]
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

/// Fast IP-PORT pattern validation with early exit
#[inline(always)]
fn is_proxyip_format(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    
    // Fast path: minimum length check (e.g., "1-1" = 3 chars)
    if len < 3 { return false; }
    
    // Find last dash from the end (faster than rfind for short strings)
    let mut dash_pos = len;
    for i in (0..len).rev() {
        if bytes[i] == b'-' {
            dash_pos = i;
            break;
        }
    }
    
    if dash_pos == 0 || dash_pos >= len - 1 { return false; }
    
    // Validate all chars after dash are ASCII digits
    for &b in &bytes[dash_pos + 1..] {
        if !b.is_ascii_digit() { return false; }
    }
    true
}

/// Fast country code validation (2 uppercase letters)
#[inline(always)]
fn is_country_code_format(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_uppercase() && bytes[1].is_ascii_uppercase()
}

#[event(fetch)]
async fn main(req: Request, env: Env, _: Context) -> Result<Response> {
    // Extract host early before any env.var calls
    let host = req.url()
        .ok()
        .and_then(|url| url.host().map(|h| h.to_string()))
        .unwrap_or_default();
    
    // Batch all env.var() calls together to minimize FFI overhead
    let (uuid, main_url, sub_url, link_url, conv_url, check_url) = {
        let uuid_str = env.var("UUID")?.to_string();
        let uuid = Uuid::parse_str(&uuid_str)
            .map_err(|_| Error::RustError("Invalid UUID format".to_string()))?;
        
        (
            uuid,
            env.var("MAIN_PAGE_URL")?.to_string(),
            env.var("SUB_PAGE_URL")?.to_string(),
            env.var("LINK_PAGE_URL")?.to_string(),
            env.var("CONVERTER_PAGE_URL")?.to_string(),
            env.var("CHECKER_PAGE_URL")?.to_string(),
        )
    };

    let config = Config { 
        uuid, 
        host: host.clone(), 
        proxy_addr: host, 
        proxy_port: 443, 
        main_page_url: main_url, 
        sub_page_url: sub_url,
        link_page_url: link_url,
        converter_page_url: conv_url,
        checker_page_url: check_url
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

/// KV-cached HTML fetcher with fallback
#[inline(always)]
async fn get_cached_html(kv: &kv::KvStore, cache_key: &str, url: String) -> Result<Response> {
    // Try KV cache first (sub-1ms read)
    if let Some(cached) = kv.get(cache_key).text().await? {
        return Response::from_html(cached);
    }
    
    // Cache miss: fetch from GitHub and store for 1 hour
    let req = Fetch::Url(Url::parse(url.as_str())?);
    let mut res = req.send().await?;
    let html = res.text().await?;
    
    // Fire-and-forget cache update (don't block response)
    let _ = kv.put(cache_key, &html)?.expiration_ttl(3600).execute().await;
    
    Response::from_html(html)
}

async fn fe(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    let kv = cx.kv("library")?;
    get_cached_html(&kv, "page:main", cx.data.main_page_url).await
}

async fn sub(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    let kv = cx.kv("library")?;
    get_cached_html(&kv, "page:sub", cx.data.sub_page_url).await
}

async fn link(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    let kv = cx.kv("library")?;
    get_cached_html(&kv, "page:link", cx.data.link_page_url).await
}

async fn converter(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    let kv = cx.kv("library")?;
    get_cached_html(&kv, "page:converter", cx.data.converter_page_url).await
}

async fn checker(_: Request, cx: RouteContext<Config>) -> Result<Response> {
    let kv = cx.kv("library")?;
    get_cached_html(&kv, "page:checker", cx.data.checker_page_url).await
}

async fn tunnel(req: Request, mut cx: RouteContext<Config>) -> Result<Response> {
    let result = tunnel_inner(req, &mut cx).await;
    
    match result {
        Ok(response) => Ok(response),
        Err(e) => {
            let error_msg = e.to_string();
            if is_benign_error(&error_msg) {
                Response::ok("Connection closed")
            } else {
                console_error!("[FATAL] Unexpected tunnel error: {}", error_msg);
                Err(e)
            }
        }
    }
}

async fn tunnel_inner(req: Request, cx: &mut RouteContext<Config>) -> Result<Response> {
    let proxyip_param = cx.param("proxyip")
        .ok_or_else(|| Error::RustError("Missing proxyip parameter".to_string()))?;
    
    let mut proxyip = proxyip_param.to_string();
    
    // KV-based proxy selection (lazy-loaded only when needed)
    if is_country_code_format(&proxyip) {
        let kvid_list: Vec<&str> = proxyip.split(',').collect();
        
        // Lazy load PROXY_LIST only for country code requests
        let proxy_list_json = cx.env.var("PROXY_LIST")
            .map(|x| x.to_string())
            .unwrap_or_else(|_| "{}".to_string());
        
        let proxy_kv: HashMap<String, Vec<String>> = serde_json::from_str(&proxy_list_json)
            .map_err(|e| Error::RustError(format!("Invalid PROXY_LIST: {}", e)))?;
        
        let mut rand_buf = [0u8; 1];
        getrandom::getrandom(&mut rand_buf)
            .map_err(|e| Error::RustError(format!("Random generation failed: {}", e)))?;
        
        let kv_index = (rand_buf[0] as usize) % kvid_list.len();
        let selected_country = kvid_list[kv_index];
        
        if let Some(proxy_list) = proxy_kv.get(selected_country) {
            if proxy_list.is_empty() {
                return Response::error("No proxies available", 502);
            }
            let proxyip_index = (rand_buf[0] as usize) % proxy_list.len();
            proxyip = proxy_list[proxyip_index].replace(':', "-");
        } else {
            return Response::error("Invalid country code", 400);
        }
    }

    // Fast path: Parse IP-PORT format (e.g., "1.2.3.4-443")
    if is_proxyip_format(&proxyip) {
        // Use split_once for zero-allocation parsing
        if let Some((addr, port_str)) = proxyip.split_once('-') {
            if let Ok(port) = port_str.parse() {
                cx.data.proxy_addr = addr.to_string();
                cx.data.proxy_port = port;
            }
        }
    }

    // Fast header check with early return
    let is_websocket = req.headers()
        .get("Upgrade")
        .ok()
        .flatten()
        .map(|v| v == "websocket")
        .unwrap_or(false);
    
    if !is_websocket {
        return Response::from_html("hi from wasm!");
    }
    
    // WebSocket handshake - moved after all validation
    let WebSocketPair { server, client } = WebSocketPair::new()
        .map_err(|e| Error::RustError(format!("WebSocket init failed: {}", e)))?;
    
    let config = cx.data.clone();
    
    wasm_bindgen_futures::spawn_local(async move {
        use gloo_timers::future::TimeoutFuture;

        if let Err(e) = server.accept() {
            console_error!("[ERROR] WebSocket accept failed: {}", e);
            return;
        }

        let events = match server.events() {
            Ok(ev) => ev,
            Err(e) => {
                console_error!("[ERROR] WebSocket events failed: {}", e);
                let _ = server.close(Some(1011), Some("Event stream error".to_string()));
                return;
            }
        };

        let process_future = async {
            match ProxyStream::new(config, &server, events).process().await {
                Ok(_) => console_log!("[DEBUG] Stream completed successfully"),
                Err(e) => {
                    let error_msg = e.to_string();
                    if !is_benign_error(&error_msg) {
                        console_error!("[ERROR] Stream processing failed: {}", error_msg);
                    }
                }
            }
        };

        // 60-second timeout for video streaming
        let timeout = TimeoutFuture::new(60_000);
        futures_util::pin_mut!(process_future);
        
        match futures_util::future::select(process_future, timeout).await {
            futures_util::future::Either::Left(_) => {
                let _ = server.close(Some(1000), Some("Normal closure".to_string()));
            },
            futures_util::future::Either::Right(_) => {
                console_log!("[DEBUG] WebSocket timeout after 60s");
                let _ = server.close(Some(1000), Some("Timeout".to_string()));
            }
        }
    });

    Response::from_websocket(client)
        .map_err(|e| Error::RustError(format!("WebSocket handshake failed: {}", e)))
}
