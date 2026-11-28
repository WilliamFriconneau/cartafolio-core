use crate::error::ServerError;
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_MAX_BODY: usize = 64 * 1024 * 1024;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_ADMIN_PROXY: Ipv4Addr = Ipv4Addr::new(172, 30, 32, 2);

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub allow_remote: bool,
    pub public_url: Option<String>,
    pub data_dir: PathBuf,
    pub max_body: usize,
    pub request_timeout: Duration,
    pub admin_listen: Option<SocketAddr>,
    pub admin_trusted_proxy: IpAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8791),
            allow_remote: false,
            public_url: None,
            data_dir: PathBuf::from("data"),
            max_body: DEFAULT_MAX_BODY,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            admin_listen: None,
            admin_trusted_proxy: IpAddr::V4(DEFAULT_ADMIN_PROXY),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    listen: Option<String>,
    #[serde(default)]
    allow_remote: bool,
    #[serde(default)]
    public_url: Option<String>,
    #[serde(default)]
    data_dir: Option<PathBuf>,
    #[serde(default)]
    max_body: Option<usize>,
    #[serde(default)]
    request_timeout: Option<u64>,
    #[serde(default)]
    admin_listen: Option<String>,
    #[serde(default)]
    admin_trusted_proxy: Option<String>,
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self, ServerError> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            ServerError::Invalid(format!("could not read config file {path:?}: {error}"))
        })?;
        Self::from_toml(&text)
    }

    pub fn from_toml(text: &str) -> Result<Self, ServerError> {
        let file: FileConfig = toml::from_str(text)
            .map_err(|error| ServerError::Invalid(format!("invalid config: {error}")))?;
        let defaults = Config::default();
        let listen = match file.listen {
            Some(listen) => listen
                .parse()
                .map_err(|_| ServerError::Invalid(format!("invalid listen address: {listen}")))?,
            None => defaults.listen,
        };
        let admin_listen = file
            .admin_listen
            .map(|address| {
                address.parse().map_err(|_| {
                    ServerError::Invalid(format!("invalid admin_listen address: {address}"))
                })
            })
            .transpose()?;
        let admin_trusted_proxy = match file.admin_trusted_proxy {
            Some(address) => address.parse().map_err(|_| {
                ServerError::Invalid(format!("invalid admin_trusted_proxy address: {address}"))
            })?,
            None => defaults.admin_trusted_proxy,
        };
        Ok(Config {
            listen,
            allow_remote: file.allow_remote,
            public_url: file.public_url.or(defaults.public_url),
            data_dir: file.data_dir.unwrap_or(defaults.data_dir),
            max_body: file.max_body.unwrap_or(defaults.max_body),
            request_timeout: file
                .request_timeout
                .map(Duration::from_secs)
                .unwrap_or(defaults.request_timeout),
            admin_listen,
            admin_trusted_proxy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_loopback_port() {
        let config = Config::default();
        assert_eq!(config.listen.to_string(), "127.0.0.1:8791");
        assert!(!config.allow_remote);
        assert_eq!(config.max_body, DEFAULT_MAX_BODY);
        assert_eq!(config.request_timeout, DEFAULT_REQUEST_TIMEOUT);
    }

    #[test]
    fn parses_every_field() {
        let text = "listen = \"127.0.0.1:9000\"\n\
                    allow_remote = true\n\
                    public_url = \"https://example.test\"\n\
                    data_dir = \"/tmp/elsewhere\"\n\
                    max_body = 1234\n\
                    request_timeout = 5\n";
        let config = Config::from_toml(text).unwrap();
        assert_eq!(config.listen.to_string(), "127.0.0.1:9000");
        assert!(config.allow_remote);
        assert_eq!(config.public_url.as_deref(), Some("https://example.test"));
        assert_eq!(config.data_dir, PathBuf::from("/tmp/elsewhere"));
        assert_eq!(config.max_body, 1234);
        assert_eq!(config.request_timeout, Duration::from_secs(5));
    }

    #[test]
    fn the_admin_listener_is_off_by_default() {
        let config = Config::default();
        assert_eq!(config.admin_listen, None);
        assert_eq!(config.admin_trusted_proxy, IpAddr::V4(DEFAULT_ADMIN_PROXY));
    }

    #[test]
    fn parses_the_admin_fields() {
        let text = "admin_listen = \"0.0.0.0:8099\"\n\
                    admin_trusted_proxy = \"172.30.32.2\"\n";
        let config = Config::from_toml(text).unwrap();
        assert_eq!(config.admin_listen, Some("0.0.0.0:8099".parse().unwrap()));
        assert_eq!(
            config.admin_trusted_proxy,
            "172.30.32.2".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        let text = "listen = \"127.0.0.1:9000\"\nbogus = true\n";
        assert!(Config::from_toml(text).is_err());
    }

    #[test]
    fn a_bad_listen_address_is_an_error() {
        let text = "listen = \"not-an-address\"\n";
        assert!(Config::from_toml(text).is_err());
    }
}
