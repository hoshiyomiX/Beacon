mod common;
mod config;
mod proxy;

use crate::config::Config;
use crate::proxy::*;

use std::collections::HashMap;
use std::rc::Rc;
use uuid::Uuid;
use worker::*;
use once_cell::sync::Lazy;
use regex::Regex;

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
    let uuid = env
        .var("UUID")
        .map(|x| Uuid::parse_str(&x.to_string()).unwrap_or_default())?;
    let host = req.url()?.host().map(|x| x.to_string()).unwrap_or_default();
    let main_page_url = env.var("MAIN_PAGE_URL").map(|x| x.to_string()).unwrap();
    let sub_page_url = env.var("SUB_PAGE_URL").map(|x| x.to_string()).unwrap();
    let link_page_url = env.var("LINK_PAGE_URL").map(|x| x.to_string()).unwrap();
    let converter_page_url = env.var("CONVERTER_PAGE_URL").map(|x| x.to_string()).unwrap();
    let checker_page_url = env.var("CHECKER_PAGE_URL").map(|x| x.to_string()).unwrap();

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
    let mut proxyip = cx.param("proxyip").unwrap().to_string();
    
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
                return Err(Error::from(format!("Invalid PROXY_LIST configuration: {}", e)));
            }
        };
        
        // Random selection logic
        let mut rand_buf = [0u8, 1];
        getrandom::getrandom(&mut rand_buf).expect("failed generating random number");
        
        let kv_index = (rand_buf[0] as usize) % kvid_list.len();
        proxyip = kvid_list[kv_index].clone();
        
        // Select random proxy from the country list
        if let Some(proxy_list) = proxy_kv.get(&proxyip) {
            if !proxy_list.is_empty() {
                let proxyip_index = (rand_buf[0] as usize) % proxy_list.len();
                proxyip = proxy_list[proxyip_index].clone().replace(":", "-");
            } else {
                return Err(Error::from(format!("No proxies available for country: {}", &proxyip)));
            }
        } else {
            return Err(Error::from(format!("Country code not found: {}", &proxyip)));
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

    let upgrade = req.headers().get("Upgrade")?.unwrap_or("".to_string());
    if upgrade == "websocket".to_string() {
        let WebSocketPair { server, client } = WebSocketPair::new()?;
        
        // CRITICAL FIX: Use Rc to share WebSocket across closures
        // Prevents "Cannot invoke closure from previous WASM instance" by properly managing lifecycle
        let server = Rc::new(server);
        let config = cx.data.clone();
        
        // Accept connection immediately
        server.accept();
        
        // Return response first to avoid "script will never generate a response" errors
        let response = Response::from_websocket(client)
            .or_else(|e| {
                console_log!("[DEBUG] WebSocket response creation failed: {}", e);
                Response::error("WebSocket handshake failed", 400)
            })?;
        
        // Process WebSocket stream in background using event context
        if let Ok(events) = server.events() {
            use gloo_timers::future::TimeoutFuture;
            
            let server_clone = Rc::clone(&server);
            let process_future = async move {
                let _ = ProxyStream::new(config, &server_clone, events).process().await;
                let _ = server_clone.close(Some(1000), Some("Normal closure".to_string()));
            };
            
            let timeout = TimeoutFuture::new(8_000);
            
            // Spawn background task with properly scoped closures
            let server_timeout = Rc::clone(&server);
            wasm_bindgen_futures::spawn_local(async move {
                futures_util::pin_mut!(process_future);
                match futures_util::future::select(process_future, timeout).await {
                    futures_util::future::Either::Left(_) => {
                        console_log!("[DEBUG] WebSocket processing completed");
                    },
                    futures_util::future::Either::Right(_) => {
                        console_log!("[DEBUG] WebSocket processing timed out");
                        let _ = server_timeout.close(Some(1000), Some("Connection timeout".to_string()));
                    }
                }
            });
        }
        
        Ok(response)
    } else {
        Response::from_html("hi from wasm!")
    }
}
