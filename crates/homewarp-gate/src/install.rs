//! Enrolling a VPS and undoing it (PLAN.md §5.5).
//!
//! `join` turns a join token into this Gate's `config.json`, opens what the
//! VPS's own firewall keeps shut, and installs and starts the service. `leave`
//! takes all of that away again. Nothing else on the machine is touched.

use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail, ensure};
use homewarp_net::{INTERFACE, default_interface, take_down};
use homewarp_proto::JoinToken;

use crate::{Config, Options, read};

/// Where the service is run from, and what it is called.
const PROGRAM: &str = "/usr/local/bin/homewarp-gate";
const UNIT: &str = "/etc/systemd/system/homewarp-gate.service";
const SERVICE: &str = "homewarp-gate";

/// Runs a program and returns what it printed. None if it is not there or failed.
fn said(program: &str, arguments: &[&str]) -> Option<String> {
    let done = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    done.status
        .success()
        .then(|| String::from_utf8_lossy(&done.stdout).into_owned())
}

/// Runs a program that has to succeed.
fn must(program: &str, arguments: &[&str]) -> anyhow::Result<()> {
    let done = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("running {program}"))?;
    ensure!(
        done.status.success(),
        "`{program} {}` failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&done.stderr).trim()
    );
    Ok(())
}

fn is_root() -> bool {
    fs::read_to_string("/proc/self/status").is_ok_and(|status| {
        status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))
            .and_then(|ids| ids.split_whitespace().nth(1))
            == Some("0")
    })
}

fn has_systemd() -> bool {
    Path::new("/run/systemd/system").exists()
}

/// The service: started at boot, started again if it ever ends, and held to
/// what it needs, which is the network and its own directory.
fn unit(dir: &Path) -> String {
    format!(
        "[Unit]
Description=Homewarp Gate
Documentation=https://github.com/lanceranara13/homewarp
After=network-online.target
Wants=network-online.target

[Service]
ExecStart={PROGRAM} run --dir {dir}
Restart=always
RestartSec=2
NoNewPrivileges=yes
CapabilityBoundingSet=CAP_NET_ADMIN
ProtectSystem=strict
ReadWritePaths={dir}
ProtectHome=yes
PrivateTmp=yes
MemoryMax=64M
TasksMax=32

[Install]
WantedBy=multi-user.target
",
        dir = dir.display()
    )
}

/// The openings a host firewall needs, as ufw is asked for them.
fn ufw_rules(wg_port: u16, api_port: u16, wan: &str) -> Vec<Vec<String>> {
    let rule = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
    vec![
        // Home dials the tunnel here.
        rule(&["allow", &format!("{wg_port}/udp")]),
        // Core talks to the Gate here, and only from inside the tunnel.
        rule(&[
            "allow",
            "in",
            "on",
            INTERFACE,
            "to",
            "any",
            "port",
            &api_port.to_string(),
            "proto",
            "tcp",
        ]),
        // Players' traffic is passed on, not received, so it is routed traffic.
        rule(&["route", "allow", "in", "on", wan, "out", "on", INTERFACE]),
    ]
}

fn ufw_is_active() -> bool {
    said("ufw", &["status"]).is_some_and(|status| status.starts_with("Status: active"))
}

/// A host firewall that drops what it was not told about would drop the
/// tunnel too: an accept in this Gate's own table cannot undo a drop in
/// another. So the firewall itself is asked, where it is one this knows.
fn open_firewall(config: &Config) -> anyhow::Result<()> {
    if ufw_is_active() {
        for rule in ufw_rules(config.listen_port, config.api_port, &config.wan) {
            let words: Vec<&str> = rule.iter().map(String::as_str).collect();
            must("ufw", &words)?;
        }
        println!(
            "ufw: opened UDP {} for the tunnel, and let the tunnel's traffic pass.",
            config.listen_port
        );
    } else if said("firewall-cmd", &["--state"]).is_some_and(|state| state.trim() == "running") {
        println!(
            "This machine runs firewalld, which Homewarp does not set up yet. Open the tunnel with:\n  \
             firewall-cmd --permanent --add-port={}/udp\n  \
             firewall-cmd --permanent --zone=trusted --add-interface={INTERFACE}\n  \
             firewall-cmd --reload",
            config.listen_port
        );
    }
    Ok(())
}

/// The same openings, as ufw is asked to take them away: `ufw delete allow ...`,
/// but `ufw route delete allow ...` for the one that is about routed traffic.
fn ufw_removals(wg_port: u16, api_port: u16, wan: &str) -> Vec<Vec<String>> {
    ufw_rules(wg_port, api_port, wan)
        .into_iter()
        .map(|mut rule| {
            let after_route = usize::from(rule.first().is_some_and(|word| word == "route"));
            rule.insert(after_route, "delete".to_owned());
            rule
        })
        .collect()
}

fn close_firewall(config: &Config) {
    if !ufw_is_active() {
        return;
    }
    for rule in ufw_removals(config.listen_port, config.api_port, &config.wan) {
        let words: Vec<&str> = rule.iter().map(String::as_str).collect();
        if let Err(error) = must("ufw", &words) {
            println!("An opening in ufw was left as it is: {error:#}");
        }
    }
}

/// Makes this VPS the Gate of the home that made `token`.
pub(crate) fn join(token: &str, options: &Options) -> anyhow::Result<()> {
    let token = JoinToken::decode(token)
        .context("That is not a join token. Copy the whole command from the panel.")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    ensure!(
        token.expires_at > now,
        "This join token ran out {} minutes ago by this machine's clock. Make a new one in the panel: Network, then Connect a VPS.",
        (now - token.expires_at) / 60 + 1
    );
    ensure!(is_root(), "Run this as root: it sets up the network.");
    ensure!(
        said("nft", &["--version"]).is_some(),
        "This machine has no `nft`, which the Gate forwards with. Install the package `nftables` and run this again."
    );
    let wan = match &options.wan {
        Some(wan) => wan.clone(),
        None => default_interface().context(
            "This machine has no default route to tell which interface players arrive on. Name it with --wan.",
        )?,
    };
    let config = Config {
        private_key: token.private_key,
        listen_port: token.wg_port,
        address: token.gate_address,
        home_address: token.home_address,
        home_public_key: token.home_public_key,
        preshared_key: token.preshared_key,
        token: token.token,
        wan,
        api_port: token.api_port,
    };

    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&options.dir)
        .with_context(|| format!("making {}", options.dir.display()))?;
    config.keep(&options.dir)?;
    // What another home asked this VPS to forward is nothing to this one.
    let _ = fs::remove_file(options.dir.join("desired.json"));
    println!(
        "Homewarp Gate: the tunnel listens on UDP {}, and players arrive on {}.",
        config.listen_port, config.wan
    );
    if !options.service {
        return Ok(());
    }

    open_firewall(&config)?;
    let this = std::env::current_exe().context("finding this program")?;
    if this != Path::new(PROGRAM) {
        // Beside it and then over it: the old one may be running.
        let beside = format!("{PROGRAM}.new");
        fs::copy(&this, &beside).with_context(|| format!("copying this program to {PROGRAM}"))?;
        fs::set_permissions(&beside, fs::Permissions::from_mode(0o755))?;
        fs::rename(&beside, PROGRAM)?;
    }
    if !has_systemd() {
        println!(
            "This machine does not use systemd, so nothing starts the Gate for you. Have this run at boot:\n  {PROGRAM} run --dir {}",
            options.dir.display()
        );
        return Ok(());
    }
    fs::write(UNIT, unit(&options.dir)).with_context(|| format!("writing {UNIT}"))?;
    must("systemctl", &["daemon-reload"])?;
    must("systemctl", &["enable", SERVICE])?;
    must("systemctl", &["restart", SERVICE])?;
    println!("The Gate is running. Go back to the panel, which finds it within a few seconds.");
    Ok(())
}

/// Takes away everything `join` made, the running tunnel with it.
pub(crate) fn leave(options: &Options) -> anyhow::Result<()> {
    ensure!(is_root(), "Run this as root: it sets up the network.");
    if has_systemd() && Path::new(UNIT).exists() {
        let _ = must("systemctl", &["disable", "--now", SERVICE]);
        fs::remove_file(UNIT).with_context(|| format!("removing {UNIT}"))?;
        let _ = must("systemctl", &["daemon-reload"]);
    }
    match read::<Config>(&options.dir.join("config.json")) {
        Ok(Some(config)) => close_firewall(&config),
        Ok(None) => {}
        Err(error) => println!("The firewall was left as it is: {error:#}"),
    }
    take_down();
    match fs::remove_dir_all(&options.dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            bail!("removing {}: {error}", options.dir.display())
        }
        _ => {}
    }
    let _ = fs::remove_file(PROGRAM);
    println!("This machine is no longer a Homewarp Gate.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ufw_removals, ufw_rules, unit};

    #[test]
    fn the_service_runs_from_its_directory_and_may_write_nowhere_else() {
        let unit = unit(Path::new("/var/lib/homewarp-gate"));
        assert!(
            unit.contains(
                "ExecStart=/usr/local/bin/homewarp-gate run --dir /var/lib/homewarp-gate\n"
            )
        );
        assert!(unit.contains("ProtectSystem=strict\nReadWritePaths=/var/lib/homewarp-gate\n"));
        assert!(unit.contains("CapabilityBoundingSet=CAP_NET_ADMIN\n"));
    }

    #[test]
    fn a_host_firewall_is_asked_for_the_tunnel_its_api_and_what_passes_through() {
        let rules: Vec<String> = ufw_rules(51820, 4857, "eth0")
            .into_iter()
            .map(|rule| rule.join(" "))
            .collect();
        assert_eq!(
            rules,
            [
                "allow 51820/udp",
                "allow in on homewarp0 to any port 4857 proto tcp",
                "route allow in on eth0 out on homewarp0",
            ]
        );
        // Taken away again, each as ufw wants that said.
        let removals: Vec<String> = ufw_removals(51820, 4857, "eth0")
            .into_iter()
            .map(|rule| rule.join(" "))
            .collect();
        assert_eq!(
            removals,
            [
                "delete allow 51820/udp",
                "delete allow in on homewarp0 to any port 4857 proto tcp",
                "route delete allow in on eth0 out on homewarp0",
            ]
        );
    }
}
