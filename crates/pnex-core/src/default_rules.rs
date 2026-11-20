//! Navigation rules shipped with PneX (agents.md D780, lot U4):
//! what used to be mission types coded in the agent is now a rule like any
//! other, played by the same engines. Each rule is YAML, parsed and checked
//! by the backend like an org's own (tested there); the server fills its
//! parameters and acts on the run's outcome by the purpose it started it
//! for, never by a kind of mission. Pure, wasm-safe.

/// Site of a profile network (D333): the host its canonical profile
/// addresses carry ([`crate::browser_agent::profile_of`]).
pub fn network_site(network: &str) -> Option<&'static str> {
    Some(match network {
        "linkedin" => "www.linkedin.com",
        "x" => "x.com",
        "facebook" => "www.facebook.com",
        "instagram" => "www.instagram.com",
        "tiktok" => "www.tiktok.com",
        "youtube" => "www.youtube.com",
        _ => return None,
    })
}
