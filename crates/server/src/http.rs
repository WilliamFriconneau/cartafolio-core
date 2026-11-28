use crate::admin::{self, AdminState};
use crate::config::Config;
use crate::error::ServerError;
use crate::mcp_http::{router, AppState};
use crate::oauth::{self, OAuth};
use pensine_access::{Accounts, Principal};
use pensine_mcp::Mcp;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::watch;

pub trait Authenticator: Send + Sync {
    fn authenticate(&self, bearer: &str) -> Option<Principal>;
}

pub struct KeysAuthenticator {
    accounts: Arc<Accounts>,
}

impl KeysAuthenticator {
    pub fn new(accounts: Arc<Accounts>) -> Self {
        Self { accounts }
    }
}

impl Authenticator for KeysAuthenticator {
    fn authenticate(&self, bearer: &str) -> Option<Principal> {
        self.accounts.authenticate(bearer).ok()
    }
}

pub struct Server;

impl Server {
    pub async fn start(config: Config) -> Result<Handle, ServerError> {
        let accounts = Arc::new(Accounts::open(&config.data_dir)?);
        let mcp = Arc::new(Mcp::new(accounts.clone()));
        let authenticators: Vec<Arc<dyn Authenticator>> =
            vec![Arc::new(KeysAuthenticator::new(accounts.clone()))];
        Self::start_inner(config, mcp, authenticators, Some(accounts)).await
    }

    pub async fn start_with(
        config: Config,
        mcp: Arc<Mcp>,
        authenticators: Vec<Arc<dyn Authenticator>>,
    ) -> Result<Handle, ServerError> {
        Self::start_inner(config, mcp, authenticators, None).await
    }

    async fn start_inner(
        config: Config,
        mcp: Arc<Mcp>,
        mut authenticators: Vec<Arc<dyn Authenticator>>,
        accounts: Option<Arc<Accounts>>,
    ) -> Result<Handle, ServerError> {
        check_bindable(&config)?;
        let bound = bind(config.listen).await?;
        let oauth = accounts
            .clone()
            .map(|accounts| {
                let fallback = format!(
                    "http://{}",
                    SocketAddr::new(
                        if !bound.local_addr.ip().is_loopback() {
                            IpAddr::V4(Ipv4Addr::LOCALHOST)
                        } else {
                            bound.local_addr.ip()
                        },
                        bound.local_addr.port(),
                    )
                );
                OAuth::open(config.clone(), accounts, fallback).map(Arc::new)
            })
            .transpose()?;
        if let Some(oauth) = &oauth {
            authenticators.push(oauth.clone());
        }
        let state = AppState {
            config: config.clone(),
            mcp,
            authenticators: Arc::new(authenticators),
            oauth: oauth.clone(),
        };
        let mut router = router(state);
        if let Some(oauth) = &oauth {
            router = router.merge(oauth::routes(oauth.clone()));
        }
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        for listener in bound.listeners {
            let router = router.clone();
            let mut receiver = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = receiver.changed().await;
                    })
                    .await;
            });
        }
        let admin_addr = serve_admin(&config, accounts, oauth.clone(), shutdown_rx.clone()).await?;
        Ok(Handle {
            local_addr: bound.local_addr,
            admin_addr,
            shutdown: shutdown_tx,
            oauth,
        })
    }
}

async fn serve_admin(
    config: &Config,
    accounts: Option<Arc<Accounts>>,
    oauth: Option<Arc<OAuth>>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<Option<SocketAddr>, ServerError> {
    let Some(admin_listen) = config.admin_listen else {
        return Ok(None);
    };
    let (Some(accounts), Some(oauth)) = (accounts, oauth) else {
        return Err(ServerError::Invalid(
            "the admin listener needs accounts and OAuth".into(),
        ));
    };
    let listener = tokio::net::TcpListener::bind(admin_listen).await?;
    let addr = listener.local_addr()?;
    let state = AdminState {
        accounts,
        oauth,
        trusted_proxy: config.admin_trusted_proxy,
        data_dir: config.data_dir.clone(),
        version: env!("CARGO_PKG_VERSION"),
        config: config.clone(),
    };
    let router = admin::router(state);
    let mut receiver = shutdown_rx;
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = receiver.changed().await;
        })
        .await;
    });
    Ok(Some(addr))
}

/// Starts both the main and the admin listeners and blocks until the process
/// receives Ctrl-C or SIGTERM, then shuts them down.
pub fn serve_blocking(config: Config) -> Result<(), ServerError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let handle = Server::start(config).await?;
        shutdown_signal().await;
        handle.shutdown();
        Ok(())
    })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            let _ = signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

pub struct Handle {
    local_addr: SocketAddr,
    admin_addr: Option<SocketAddr>,
    shutdown: watch::Sender<bool>,
    oauth: Option<Arc<OAuth>>,
}

impl Handle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn admin_addr(&self) -> Option<SocketAddr> {
        self.admin_addr
    }

    /// Issues a one-time OAuth code for an account already verified by a trusted caller.
    pub fn oauth_invitation(&self, account: &str) -> Result<String, ServerError> {
        self.oauth
            .as_ref()
            .ok_or_else(|| ServerError::Invalid("OAuth is unavailable".into()))?
            .invitation(account)
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

fn check_bindable(config: &Config) -> Result<(), ServerError> {
    if !config.listen.ip().is_loopback() && !config.allow_remote {
        return Err(ServerError::Invalid(
            "binding a non-loopback address requires allow_remote = true".into(),
        ));
    }
    Ok(())
}

struct Bound {
    listeners: Vec<tokio::net::TcpListener>,
    local_addr: SocketAddr,
}

async fn bind(listen: SocketAddr) -> Result<Bound, ServerError> {
    if !listen.ip().is_unspecified() {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        let local_addr = listener.local_addr()?;
        return Ok(Bound {
            listeners: vec![listener],
            local_addr,
        });
    }
    let wildcard = tokio::net::TcpListener::bind(listen).await?;
    let local_addr = wildcard.local_addr()?;
    let loopback_ip = if local_addr.is_ipv4() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        IpAddr::V6(Ipv6Addr::LOCALHOST)
    };
    let loopback_addr = SocketAddr::new(loopback_ip, local_addr.port());
    match tokio::net::TcpListener::bind(loopback_addr).await {
        Ok(loopback) => Ok(Bound {
            listeners: vec![wildcard, loopback],
            local_addr: loopback_addr,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if wildcard_covers_loopback() {
                Ok(Bound {
                    listeners: vec![wildcard],
                    local_addr,
                })
            } else {
                Err(ServerError::Invalid(format!(
                    "the loopback address {loopback_addr} is already held by another program"
                )))
            }
        }
        Err(error) => Err(ServerError::Io(error)),
    }
}

fn wildcard_covers_loopback() -> bool {
    cfg!(target_os = "linux")
}
