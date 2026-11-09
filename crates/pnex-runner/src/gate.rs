//! The egress gate (SEC-E4): a `NetworkPolicy` holds some seconds after it exists, so the
//! first container of a job pod waits until an address the rule refuses stops answering.
//! Build and scan pods may reach the internet, not the cluster: their probe is the runner's
//! own pod, which listens on [`PORT`] for that alone. The gate runs from the runner's image
//! (`pnex-runner egress-gate`): a TCP connect, nothing the job image has to provide.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Where the runner listens for the gates of its pods: it accepts and closes, nothing more.
pub const PORT: u16 = 7171;
pub const TRIES: u32 = 60;

/// Waits while `reachable()` holds, one try a second; `true` once it no longer does.
pub fn wait_until_blocked(
    mut reachable: impl FnMut() -> bool,
    tries: u32,
    pause: Duration,
) -> bool {
    for _ in 0..tries {
        if !reachable() {
            return true;
        }
        std::thread::sleep(pause);
    }
    false
}

/// `pnex-runner egress-gate`: exit 0 once the rule holds, 1 after a minute without it.
pub fn run() -> i32 {
    let Some(probe) = std::env::var("PNEX_EGRESS_PROBE")
        .ok()
        .and_then(|p| p.parse::<SocketAddr>().ok())
    else {
        eprintln!("PNEX_EGRESS_PROBE is not an address: not starting");
        return 1;
    };
    let reachable = || TcpStream::connect_timeout(&probe, Duration::from_secs(2)).is_ok();
    if wait_until_blocked(reachable, TRIES, Duration::from_secs(1)) {
        println!("egress rule in place");
        0
    } else {
        eprintln!("egress rule not applied after a minute: not starting");
        1
    }
}

/// The runner's side: accepts connections from gates and drops them, in the background.
pub async fn listen() {
    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", PORT)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("egress gate probe not listening on {PORT}: {e}");
            return;
        }
    };
    loop {
        if let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_opens_when_the_probe_stops_answering_and_never_otherwise() {
        let mut answers = vec![true, true, false].into_iter();
        assert!(wait_until_blocked(
            || answers.next().unwrap(),
            5,
            Duration::ZERO
        ));
        assert!(
            !wait_until_blocked(|| true, 3, Duration::ZERO),
            "still open: never starts"
        );
    }
}
