use uuid::Uuid;

#[derive(Clone)]
#[allow(dead_code)] // host field is used but dead code analyzer gets confused by Clone + async spawn
pub struct Config {
    pub uuid: Uuid,
    pub host: String,
    pub proxy_addr: String,
    pub proxy_port: u16,

    pub main_page_url: String,
    pub sub_page_url: String,
    pub link_page_url: String,
    pub converter_page_url: String,
    pub checker_page_url: String,
}
