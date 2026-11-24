//! The listening socket: binds `127.0.0.1` on the first free port of a list, accepts
//! loopback peers only and serves every connection on its own thread.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::hub::Hub;
use crate::protocol::HEARTBEAT_TIMEOUT_MS;

/// Default port of the bridge.
pub const DEFAULT_PORT: u16 = 17_555;
/// Ports tried in order when the default one is taken.
pub const FALLBACK_PORTS: std::ops::RangeInclusive<u16> = 17_556..=17_564;
/// Id of the Chrome extension, derived from the public `key` in `extension/manifest.json`
/// (ADR 0021). The application accepts only this origin unless the settings say otherwise.
pub const EXTENSION_ID: &str = "pbgjebpmicmffhjbphlgginplaflodnj";
/// Sockets served at the same time, handshakes included. More are closed right away.
const MAX_SOCKETS: usize = 8;
/// How often streams waiting for `resume` and expired codes are checked.
const REAP_EVERY: Duration = Duration::from_millis(200);

/// Settings of the listening socket.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Ports tried in order; `0` picks a free port (tests).
    pub ports: Vec<u16>,
    /// Extension ids accepted in the `Origin` header. Empty accepts any
    /// `chrome-extension://` origin; the pairing secret still decides who may stream.
    pub allowed_extension_ids: Vec<String>,
    /// A connection that sends nothing for this long is closed.
    pub heartbeat_timeout: Duration,
    /// Time allowed between the upgrade and `hello`.
    pub hello_timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            ports: std::iter::once(DEFAULT_PORT)
                .chain(FALLBACK_PORTS)
                .collect(),
            allowed_extension_ids: Vec::new(),
            heartbeat_timeout: Duration::from_millis(u64::from(HEARTBEAT_TIMEOUT_MS)),
            hello_timeout: Duration::from_secs(10),
        }
    }
}

/// A running bridge server. Dropping it stops the server and closes every connection.
pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    hub: Arc<Hub>,
    accept: Option<JoinHandle<()>>,
    reaper: Option<JoinHandle<()>>,
}

fn bind_first(ports: &[u16]) -> io::Result<TcpListener> {
    let mut last = io::Error::new(io::ErrorKind::InvalidInput, "no port to listen on");
    for &port in ports {
        match TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))) {
            Ok(l) => return Ok(l),
            Err(e) => {
                tracing::info!(port, error = %e, "bridge port not available");
                last = e;
            }
        }
    }
    Err(last)
}

impl Server {
    /// Bind and start accepting connections.
    pub fn start(config: ServerConfig, hub: Arc<Hub>) -> io::Result<Self> {
        let listener = bind_first(&config.ports)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let config = Arc::new(config);

        let accept = {
            let (stop, hub) = (stop.clone(), hub.clone());
            std::thread::Builder::new()
                .name("bridge-accept".into())
                .spawn(move || accept_loop(listener, port, config, hub, stop))?
        };
        let reaper = {
            let (stop, hub) = (stop.clone(), hub.clone());
            std::thread::Builder::new()
                .name("bridge-reaper".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(REAP_EVERY);
                        hub.reap(Instant::now());
                    }
                })?
        };
        tracing::info!(port, "bridge listening");
        Ok(Self {
            port,
            stop,
            hub,
            accept: Some(accept),
            reaper: Some(reaper),
        })
    }

    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// Stop accepting, close every connection and wait for the threads.
    pub fn shutdown(mut self) {
        self.stop_threads();
    }

    fn stop_threads(&mut self) {
        if self.stop.swap(true, Ordering::Relaxed) {
            return;
        }
        self.hub.close_all("The application is closing the bridge.");
        // Wake the blocking accept.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
            Duration::from_millis(500),
        );
        for h in [self.accept.take(), self.reaper.take()]
            .into_iter()
            .flatten()
        {
            let _ = h.join();
        }
        tracing::info!(port = self.port, "bridge stopped");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop_threads();
    }
}

fn accept_loop(
    listener: TcpListener,
    port: u16,
    config: Arc<ServerConfig>,
    hub: Arc<Hub>,
    stop: Arc<AtomicBool>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    for stream in listener.incoming() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        workers.retain(|h| !h.is_finished());
        let Ok(stream) = stream else { continue };
        let loopback = stream.peer_addr().is_ok_and(|a| a.ip().is_loopback());
        if !loopback {
            tracing::warn!("bridge refused a connection from outside this machine");
            continue;
        }
        if active.load(Ordering::Relaxed) >= MAX_SOCKETS {
            tracing::warn!("bridge refused a connection: too many sockets");
            continue;
        }
        active.fetch_add(1, Ordering::Relaxed);
        let (hub, config, stop, done) = (hub.clone(), config.clone(), stop.clone(), active.clone());
        let spawned = std::thread::Builder::new()
            .name("bridge-conn".into())
            .spawn(move || {
                crate::conn::serve(stream, hub, config, port, stop);
                done.fetch_sub(1, Ordering::Relaxed);
            });
        match spawned {
            Ok(h) => workers.push(h),
            Err(e) => {
                active.fetch_sub(1, Ordering::Relaxed);
                tracing::error!(error = %e, "bridge could not start a connection thread");
            }
        }
    }
    for h in workers {
        let _ = h.join();
    }
}
