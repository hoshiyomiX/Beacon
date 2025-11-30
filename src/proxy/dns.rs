use anyhow::Result;
use worker::{Fetch, Request, RequestInit, Headers, Method};

pub async fn doh(req_wireformat: &[u8]) -> Result<Vec<u8>> {
    let mut headers = Headers::new();
    headers.set("content-type", "application/dns-message")?;
    headers.set("accept", "application/dns-message")?;
    
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(req_wireformat.to_vec().into()));

    let request = Request::new_with_init("https://1.1.1.1/dns-query", &init)?;
    let mut response = Fetch::Request(request).send().await?;
    let bytes = response.bytes().await?;
    Ok(bytes)
}
