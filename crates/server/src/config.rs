use crate::error::ServerError;
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::{Host, Url};

pub const DEFAULT_MAX_BODY: usize = 64 * 1024 * 1024;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_ADMIN_PROXY: Ipv4Addr = Ipv4Addr::new(172, 30, 32, 2);

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub allow_remote: bool,
    pub public_url: Option<String>,
    pub public_url_file: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub max_body: usize,
    /// End-to-end MCP deadline, including admission and the body. A synchronous
    /// operation already running can finish after the client receives 504.
    /// Its account lock and admission permits remain held until it exits.
    pub request_timeout: Duration,
    /// Maximum time to read an authenticated, admitted MCP body.
    pub read_timeout: Duration,
    pub account_lock_timeout: Duration,
    pub account_in_flight: usize,
    pub account_queue: usize,
    pub global_in_flight: usize,
    pub global_queue: usize,
    pub oauth_registration_cap: usize,
    pub oauth_registrations_per_minute: usize,
    pub file_quota: pensine_store::FileQuota,
    pub admin_listen: Option<SocketAddr>,
    pub admin_trusted_proxy: IpAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8791),
            allow_remote: false,
            public_url: None,
            public_url_file: None,
            data_dir: PathBuf::from("data"),
            max_body: DEFAULT_MAX_BODY,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            read_timeout: DEFAULT_REQUEST_TIMEOUT,
            account_lock_timeout: Duration::from_secs(5),
            account_in_flight: 4,
            account_queue: 16,
            global_in_flight: 64,
            global_queue: 16,
            oauth_registration_cap: 200,
            oauth_registrations_per_minute: 10,
            file_quota: pensine_store::FileQuota::default(),
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
    allow_plain_http: Option<bool>,
    read_timeout: Option<u64>,
    account_lock_timeout: Option<u64>,
    account_in_flight: Option<usize>,
    account_queue: Option<usize>,
    global_in_flight: Option<usize>,
    global_queue: Option<usize>,
    oauth_registration_cap: Option<usize>,
    oauth_registrations_per_minute: Option<usize>,
    file_quota_bytes: Option<u64>,
    file_quota_count: Option<u64>,
    #[serde(default)]
    public_url: Option<String>,
    public_url_file: Option<PathBuf>,
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
    pub fn public_url(&self) -> Option<String> {
        if let Some(url) = &self.public_url {
            return Some(url.clone());
        }
        let value = std::fs::read_to_string(self.public_url_file.as_ref()?).ok()?;
        let value = value.trim();
        let (scheme, authority) = value.split_once("://")?;
        if !scheme.eq_ignore_ascii_case("https")
            || authority.trim_end_matches('/').is_empty()
            || authority
                .strip_suffix('/')
                .unwrap_or(authority)
                .contains('/')
            || authority.contains(['\\', '"'])
            || authority
                .contains(|character: char| character.is_whitespace() || character.is_control())
        {
            return None;
        }
        parse_public_origin(value).map(|_| value.to_owned())
    }

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
        let listen = if file.allow_plain_http == Some(false) && !listen.ip().is_loopback() {
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen.port())
        } else {
            listen
        };
        Ok(Config {
            listen,
            allow_remote: file.allow_remote,
            public_url: file.public_url,
            public_url_file: file.public_url_file,
            data_dir: file.data_dir.unwrap_or(defaults.data_dir),
            max_body: file.max_body.unwrap_or(defaults.max_body),
            request_timeout: file
                .request_timeout
                .map(Duration::from_secs)
                .unwrap_or(defaults.request_timeout),
            read_timeout: file
                .read_timeout
                .map(Duration::from_secs)
                .unwrap_or(defaults.read_timeout),
            account_lock_timeout: file
                .account_lock_timeout
                .map(Duration::from_secs)
                .unwrap_or(defaults.account_lock_timeout),
            account_in_flight: file.account_in_flight.unwrap_or(defaults.account_in_flight),
            account_queue: file.account_queue.unwrap_or(defaults.account_queue),
            global_in_flight: file.global_in_flight.unwrap_or(defaults.global_in_flight),
            global_queue: file.global_queue.unwrap_or(defaults.global_queue),
            oauth_registration_cap: file
                .oauth_registration_cap
                .unwrap_or(defaults.oauth_registration_cap),
            oauth_registrations_per_minute: file
                .oauth_registrations_per_minute
                .unwrap_or(defaults.oauth_registrations_per_minute),
            file_quota: pensine_store::FileQuota {
                max_bytes: file
                    .file_quota_bytes
                    .unwrap_or(defaults.file_quota.max_bytes),
                max_files: file
                    .file_quota_count
                    .unwrap_or(defaults.file_quota.max_files),
            },
            admin_listen,
            admin_trusted_proxy,
        })
    }
}

pub(crate) fn parse_public_origin(value: &str) -> Option<Url> {
    parse_web_url(value.trim_end_matches('/'))
        .filter(|url| url.path() == "/" && url.query().is_none())
}

pub(crate) fn parse_web_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let loopback = match url.host()? {
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
        Host::Domain(domain) => domain == "localhost",
    };
    ((url.scheme() == "https" || url.scheme() == "http" && loopback)
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
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
