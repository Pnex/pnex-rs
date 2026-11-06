//! Network of a micro-VM (agent-environments.md D762): one tap per VM, a /30 of its own,
//! and an nftables table of its own on the host side. What leaves the tap goes only to
//! the destinations of the claim and to the resolver; the rest is dropped, whatever the
//! guest does inside. Needs `CAP_NET_ADMIN` in the runner's network namespace.

use std::process::{Command, Stdio};

use pnex_core::runners::SandboxEgress;

/// Addresses of one VM: `172.31.x.y/30`, host side first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub host: String,
    pub guest: String,
}

/// The `n`th /30 of `172.31.0.0/16`, for up to 16384 VMs at once.
pub fn slot(n: u16) -> Slot {
    let base = u32::from(n % 16384) * 4;
    let ip = |offset: u32| {
        let v = base + offset;
        format!("172.31.{}.{}", v / 256, v % 256)
    };
    Slot {
        host: ip(1),
        guest: ip(2),
    }
}

/// A Linux interface name (15 characters at most) and an nftables table name per job.
pub fn names(job: &str) -> (String, String) {
    let id: String = job
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(10)
        .collect();
    (format!("pnx{id}"), format!("pnex_vm_{id}"))
}

fn family(cidr: &str) -> &'static str {
    if cidr.contains(':') {
        "ip6"
    } else {
        "ip"
    }
}

/// The rules of one VM: its own table, touching nothing else of the host. Traffic from the
/// tap to the host itself (`input`) and beyond it (`forward`) passes only to the listed
/// destinations and the resolver; replies come back; the guest is masqueraded on the way
/// out.
pub fn ruleset(table: &str, tap: &str, slot: &Slot, egress: &[SandboxEgress], dns: &str) -> String {
    let mut allow = String::new();
    for proto in ["udp", "tcp"] {
        allow.push_str(&format!(
            "    iifname \"{tap}\" {} daddr {dns} {proto} dport 53 accept\n",
            family(dns)
        ));
    }
    for e in egress {
        allow.push_str(&format!(
            "    iifname \"{tap}\" {} daddr {} tcp dport {} accept\n",
            family(&e.cidr),
            e.cidr,
            e.port
        ));
    }
    format!(
        "table inet {table} {{\n\
         \x20 chain input {{\n\
         \x20   type filter hook input priority 0; policy accept;\n\
         \x20   iifname \"{tap}\" ct state established,related accept\n\
         {allow}\
         \x20   iifname \"{tap}\" drop\n\
         \x20 }}\n\
         \x20 chain forward {{\n\
         \x20   type filter hook forward priority 0; policy accept;\n\
         \x20   iifname \"{tap}\" ct state established,related accept\n\
         \x20   oifname \"{tap}\" ct state established,related accept\n\
         {allow}\
         \x20   iifname \"{tap}\" drop\n\
         \x20   oifname \"{tap}\" drop\n\
         \x20 }}\n\
         \x20 chain postrouting {{\n\
         \x20   type nat hook postrouting priority 100; policy accept;\n\
         \x20   ip saddr {guest} oifname != \"{tap}\" masquerade\n\
         \x20 }}\n\
         }}\n",
        guest = slot.guest
    )
}

/// The kernel parameter that gives the guest its address and route, no DHCP.
pub fn guest_boot_arg(slot: &Slot) -> String {
    format!("ip={}::{}:255.255.255.252::eth0:off", slot.guest, slot.host)
}

fn run(args: &[&str], stdin: Option<&str>) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new(args[0])
        .args(&args[1..])
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", args[0]))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Creates the tap and its rules: the rules first, so not one packet passes before them.
pub fn up(job: &str, slot: &Slot, egress: &[SandboxEgress], dns: &str) -> Result<String, String> {
    let (tap, table) = names(job);
    let _ = down(job);
    run(
        &["nft", "-f", "-"],
        Some(&ruleset(&table, &tap, slot, egress, dns)),
    )?;
    run(&["ip", "tuntap", "add", "dev", &tap, "mode", "tap"], None)?;
    run(
        &[
            "ip",
            "addr",
            "add",
            &format!("{}/30", slot.host),
            "dev",
            &tap,
        ],
        None,
    )?;
    run(&["ip", "link", "set", &tap, "up"], None)?;
    Ok(tap)
}

/// Removes what [`up`] made; what is already gone is fine.
pub fn down(job: &str) -> Result<(), String> {
    let (tap, table) = names(job);
    let a = run(&["ip", "link", "del", &tap], None);
    let b = run(&["nft", "delete", "table", "inet", &table], None);
    match (a, b) {
        (Err(e), _) if !e.contains("Cannot find") && !e.contains("does not exist") => Err(e),
        (_, Err(e)) if !e.contains("No such file") => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_vm_gets_its_own_addresses_names_and_rules() {
        assert_eq!(
            slot(0),
            Slot {
                host: "172.31.0.1".into(),
                guest: "172.31.0.2".into()
            }
        );
        assert_eq!(slot(64).host, "172.31.1.1");
        let (tap, table) = names("8a2f-41c3-9e10-77aa");
        assert_eq!(tap, "pnx8a2f41c39e");
        assert!(tap.len() <= 15);
        assert_eq!(table, "pnex_vm_8a2f41c39e");
        let rules = ruleset(
            &table,
            &tap,
            &slot(0),
            &[SandboxEgress {
                cidr: "203.0.113.7/32".into(),
                port: 443,
            }],
            "10.0.0.53",
        );
        assert!(rules
            .contains("iifname \"pnx8a2f41c39e\" ip daddr 203.0.113.7/32 tcp dport 443 accept"));
        assert!(rules.contains("ip daddr 10.0.0.53 udp dport 53 accept"));
        assert!(rules.contains("ip saddr 172.31.0.2 oifname != \"pnx8a2f41c39e\" masquerade"));
        // Whatever is not listed is dropped, both to the host and beyond it.
        assert_eq!(rules.matches("iifname \"pnx8a2f41c39e\" drop").count(), 2);
        assert_eq!(
            guest_boot_arg(&slot(0)),
            "ip=172.31.0.2::172.31.0.1:255.255.255.252::eth0:off"
        );
    }
}
