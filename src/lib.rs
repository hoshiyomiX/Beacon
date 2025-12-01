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
    if PROXYKV_PATTERN.is_match(&proxyip)  {
        let kvid_list: Vec<String> = proxyip.split(",").map(|s| s.to_string()).collect();
        let kv = cx.kv("library")?;
        let mut proxy_kv_str = kv.get("proxy_kv").text().await?.unwrap_or("".to_string());
        let mut rand_buf = [0u8, 1];
        getrandom::getrandom(&mut rand_buf).expect("failed generating random number");

        if proxy_kv_str.len() == 0 {
            console_log!("getting proxy kv from github...");
            let req = Fetch::Url(Url::parse("https://raw.githubusercontent.com/FoolVPN-ID/Nautica/refs/heads/main/kvProxyList.json")?);
            let mut res = req.send().await?;
            if res.status_code() == 200 {
                proxy_kv_str = res.text().await?.to_string();
                kv.put("proxy_kv", &proxy_kv_str)?.expiration_ttl(60 * 60 * 24).execute().await?; // 24 hours
            } else {
                return Err(Error::from(format!("error getting proxy kv: {}", res.status_code())));
            }
        }

        let proxy_kv: HashMap<String, Vec<String>> = serde_json::from_str(&proxy_kv_str)?;

        let kv_index = (rand_buf[0] as usize) % kvid_list.len();
        proxyip = kvid_list[kv_index].clone();

        let proxyip_index = (rand_buf[0] as usize) % proxy_kv[&proxyip].len();
        proxyip = proxy_kv[&proxyip][proxyip_index].clone().replace(":", "-");
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
        server.accept()?;

        // WASM-compatible spawn: Use spawn_local instead of tokio::spawn
        // tokio::spawn requires multi-threaded runtime not available in Cloudflare Workers
        wasm_bindgen_futures::spawn_local(async move {
            // Get events with benign error handling
            let events = match server.events() {
                Ok(ev) => ev,
                Err(e) => {
                    let error_msg = e.to_string();
                    // Only log non-benign errors
                    if !is_benign_error(&error_msg) {
                        console_log!("[ERROR] Failed to get WebSocket events: {}", error_msg);
                    }
                    // Exit gracefully for benign errors - no panic, no CF error log
                    return;
                }
            };

            // Process proxy stream with full benign error suppression
            match ProxyStream::new(cx.data, &server, events).process().await {
                Ok(_) => {
                    // Normal completion - connection closed gracefully
                },
                Err(e) => {
                    let error_msg = e.to_string();
                    // Only log fatal errors - benign errors are silently dropped
                    if !is_benign_error(&error_msg) {
                        console_log!("[FATAL] Proxy processing error: {}", error_msg);
                    }
                    // Benign errors don't bubble up - they end here
                }
            }
            // No error propagation = no CF error logs for benign cases
        });

        Response::from_websocket(client)
    } else {
        Response::from_html("hi from wasm!")
    }
}
