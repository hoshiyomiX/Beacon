mod common;
mod config;
mod proxy;

use crate::config::Config;
use crate::proxy::*;

use std::collections::HashMap;
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use serde_json::json;
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
        || error_lower.contains("benign")
        || error_lower.contains("not enough buffer")
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
        
        // Spawn WebSocket processing in fire-and-forget mode with full error suppression
        wasm_bindgen_futures::spawn_local(async move {
            use gloo_timers::future::TimeoutFuture;
            
            // Wrap entire task in Result to catch ALL errors before they bubble to Cloudflare
            let task_result: Result<(), Box<dyn std::error::Error>> = async {
                // Accept connection
                server.accept().map_err(|e| {
                    let msg = e.to_string();
                    if !is_benign_error(&msg) {
                        // Only log non-benign errors
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, msg)) as Box<dyn std::error::Error>
                    } else {
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, "benign")) as Box<dyn std::error::Error>
                    }
                })?;
                
                // Get event stream
                let events = server.events().map_err(|e| {
                    let msg = e.to_string();
                    if !is_benign_error(&msg) {
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, msg)) as Box<dyn std::error::Error>
                    } else {
                        Box::new(std::io::Error::new(std::io::ErrorKind::Other, "benign")) as Box<dyn std::error::Error>
                    }
                })?;

                // Process proxy stream with timeout
                let process_future = async {
                    ProxyStream::new(cx.data, &server, events).process().await.map_err(|e| {
                        let msg = e.to_string();
                        if !is_benign_error(&msg) {
                            Box::new(std::io::Error::new(std::io::ErrorKind::Other, msg)) as Box<dyn std::error::Error>
                        } else {
                            Box::new(std::io::Error::new(std::io::ErrorKind::Other, "benign")) as Box<dyn std::error::Error>
                        }
                    })
                };

                let timeout = TimeoutFuture::new(8_000);
                futures_util::pin_mut!(process_future);
                
                match futures_util::future::select(process_future, timeout).await {
                    futures_util::future::Either::Left((result, _)) => result?,
                    futures_util::future::Either::Right(_) => {
                        // Timeout is expected, treat as benign
                        return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "benign")));
                    }
                }

                Ok(())
            }.await;

            // Close WebSocket based on result
            let _ = match task_result {
                Ok(_) => server.close(Some(1000), Some("Normal closure".to_string())),
                Err(e) if e.to_string().contains("benign") => {
                    // Benign error - close normally without logging
                    server.close(Some(1000), Some("Connection closed".to_string()))
                },
                Err(_) => {
                    // Only non-benign errors reach here, already logged above
                    server.close(Some(1011), Some("Internal error".to_string()))
                }
            };
        });

        // Return WebSocket response immediately - processing happens in spawned task
        Response::from_websocket(client)
    } else {
        Response::from_html("hi from wasm!")
    }
}
