/**
 * Configuration class for Beacon proxy
 */
export class Config {
  constructor({
    uuid,
    host,
    proxyAddr,
    proxyPort,
    mainPageUrl,
    subPageUrl,
    linkPageUrl,
    converterPageUrl,
    checkerPageUrl
  }) {
    this.uuid = uuid;
    this.host = host;
    this.proxyAddr = proxyAddr;
    this.proxyPort = proxyPort;
    this.mainPageUrl = mainPageUrl;
    this.subPageUrl = subPageUrl;
    this.linkPageUrl = linkPageUrl;
    this.converterPageUrl = converterPageUrl;
    this.checkerPageUrl = checkerPageUrl;
  }
}
