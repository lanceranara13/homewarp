use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    pin::pin,
};

use bollard::{
    Docker,
    errors::Error as DockerError,
    models::{
        ContainerCreateBody, ContainerStatsResponse, HostConfig, HostConfigLogConfig, Ipam,
        IpamConfig, NetworkCreateRequest, PortBinding,
    },
    query_parameters::{
        AttachContainerOptionsBuilder, CreateContainerOptionsBuilder, CreateImageOptionsBuilder,
        KillContainerOptionsBuilder, RemoveContainerOptionsBuilder, StatsOptionsBuilder,
    },
};
use futures_util::{Stream, StreamExt};

use crate::Console;

/// What an install script may use while it runs.
const INSTALL_MEMORY: i64 = 1024 * 1024 * 1024;
const PIDS: i64 = 512;
/// Where servers look names up unless told otherwise. Left to Docker they would
/// ask the home's own resolver, which is as a rule the router, and a server is
/// kept from every address on the home network. Wings gives its servers these
/// two as well.
pub const RESOLVERS: [&str; 2] = ["1.1.1.1", "1.0.0.1"];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Docker: {0}")]
    Docker(#[from] DockerError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("container {0} ended without an exit code")]
    NoExitCode(String),
    #[error("could not hand the server's files to its user (chown exited with {0})")]
    Ownership(i64),
}

/// The bridge that server containers sit on.
#[derive(Debug, Clone)]
pub struct Network {
    /// Also the name of the bridge interface on the host, so at most 15 characters.
    pub name: String,
    pub subnet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

/// A port published on the host under the same number the server listens on.
#[derive(Debug, Clone, Copy)]
pub struct Port {
    pub host_ip: IpAddr,
    pub port: u16,
    pub protocol: Protocol,
}

/// One server, as Docker needs to know it.
#[derive(Debug, Clone)]
pub struct Server {
    /// Names its containers; a UUID in practice.
    pub id: String,
    /// The server's files, as a path on the Docker host.
    pub dir: PathBuf,
    pub image: String,
    /// The startup command, placeholders already filled in.
    pub startup: String,
    /// The template's variables and their values.
    pub variables: Vec<(String, String)>,
    pub memory_mb: u32,
    /// How much processor it may use, where 100 is one core. 0 is no limit.
    pub cpu_percent: u32,
    /// The first is the server's default port.
    pub ports: Vec<Port>,
    /// The user the server runs as. Deliberately not a user that exists on the host.
    pub uid: u32,
    pub gid: u32,
    pub network: String,
    pub timezone: String,
    /// Where it looks names up.
    pub resolvers: Vec<String>,
}

impl Server {
    /// The environment Wings gives a server, which eggs and their images rely on.
    fn environment(&self) -> Vec<String> {
        let port = self.ports.first().map_or(0, |port| port.port);
        let mut environment = vec![
            format!("TZ={}", self.timezone),
            format!("STARTUP={}", self.startup),
            format!("SERVER_MEMORY={}", self.memory_mb),
            "SERVER_IP=0.0.0.0".to_owned(),
            format!("SERVER_PORT={port}"),
            format!("P_SERVER_UUID={}", self.id),
            "P_SERVER_LOCATION=home".to_owned(),
            "P_SERVER_ALLOCATION_LIMIT=0".to_owned(),
        ];
        environment.extend(
            self.variables
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        environment
    }

    /// The container's memory limit in bytes: what was asked for plus the
    /// headroom Wings adds, because eggs size the JVM heap from `SERVER_MEMORY`
    /// and the process needs more than its heap.
    fn memory_limit(&self) -> i64 {
        let megabytes = i64::from(self.memory_mb);
        let percent = match megabytes {
            ..=2048 => 115,
            2049..=4096 => 110,
            _ => 105,
        };
        megabytes * 1024 * 1024 * percent / 100
    }

    fn container(&self) -> String {
        format!("homewarp-{}", self.id)
    }
}

/// How much of the machine a running server is using.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    /// Of one core: 150 is a core and a half.
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    /// The most it may use before the kernel stops it.
    pub memory_limit_bytes: u64,
}

/// Works out a server's usage from what Docker reports. Processor time is
/// reported as a running total, so a sample means something only beside the
/// one before it, which is kept in `before`.
fn measure(sample: &ContainerStatsResponse, before: &mut Option<(u64, u64)>) -> Option<Usage> {
    let cpu = sample.cpu_stats.as_ref()?;
    let used = cpu.cpu_usage.as_ref()?.total_usage?;
    let passed = cpu.system_cpu_usage?;
    let cores = cpu.online_cpus.unwrap_or(1).max(1);
    let memory = sample.memory_stats.as_ref()?;
    // As `docker stats` counts it: without the file cache, which the kernel
    // gives back the moment something else needs the memory.
    let cache = memory
        .stats
        .as_ref()
        .and_then(|stats| stats.get("inactive_file"))
        .copied()
        .unwrap_or(0);
    let memory_bytes = memory.usage?.saturating_sub(cache);
    let (used_before, passed_before) = before.replace((used, passed))?;
    if passed <= passed_before {
        return None;
    }
    let share = used.saturating_sub(used_before) as f64 / (passed - passed_before) as f64;
    Some(Usage {
        cpu_percent: (share * f64::from(cores) * 100.0) as f32,
        memory_bytes,
        memory_limit_bytes: memory.limit.unwrap_or(0),
    })
}

/// An egg's install script and where to put it.
#[derive(Debug, Clone, Copy)]
pub struct InstallScript<'a> {
    pub image: &'a str,
    pub entrypoint: &'a str,
    pub script: &'a str,
    /// A directory on the Docker host for the script file. Not inside the server's.
    pub scratch: &'a Path,
}

/// A program run once where a server runs: on the servers' bridge, with one
/// TCP port published as a server's is. Core's probe of the tunnel listens
/// from one, because that is the way a player's packets come.
#[derive(Debug, Clone)]
pub struct Listener<'a> {
    /// Names its container.
    pub name: &'a str,
    pub network: &'a str,
    pub image: &'a str,
    /// The program and its arguments, in place of whatever the image starts.
    pub command: Vec<String>,
    pub port: u16,
    pub user: u32,
}

/// What such a program may use. It is given none of the machine's files.
const LISTENER_MEMORY: i64 = 64 * 1024 * 1024;

/// The Docker daemon.
pub struct Engine {
    docker: Docker,
}

impl Engine {
    /// Connects to the daemon on the local socket.
    pub fn connect() -> Result<Self, Error> {
        Ok(Self {
            docker: Docker::connect_with_local_defaults()?,
        })
    }

    /// Creates the servers' bridge network unless it exists.
    pub async fn ensure_network(&self, network: &Network) -> Result<(), Error> {
        match self.docker.inspect_network(&network.name, None).await {
            Ok(_) => return Ok(()),
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(other) => return Err(other.into()),
        }
        let options = HashMap::from([
            (
                "com.docker.network.bridge.name".to_owned(),
                network.name.clone(),
            ),
            // Servers have no business talking to each other.
            (
                "com.docker.network.bridge.enable_icc".to_owned(),
                "false".to_owned(),
            ),
        ]);
        let ipam = Ipam {
            config: Some(vec![IpamConfig {
                subnet: Some(network.subnet.clone()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        self.docker
            .create_network(NetworkCreateRequest {
                name: network.name.clone(),
                driver: Some("bridge".to_owned()),
                ipam: Some(ipam),
                options: Some(options),
                ..Default::default()
            })
            .await?;
        Ok(())
    }

    /// Whether the daemon has `image` already, so that nothing waits for it.
    pub async fn has_image(&self, image: &str) -> bool {
        self.docker.inspect_image(image).await.is_ok()
    }

    /// Pulls `image` unless the daemon already has it.
    pub async fn pull(&self, image: &str) -> Result<(), Error> {
        if self.has_image(image).await {
            return Ok(());
        }
        // Without a tag the daemon would fetch every tag of the repository.
        let name = image.rsplit('/').next().unwrap_or(image);
        let image = if name.contains([':', '@']) {
            image.to_owned()
        } else {
            format!("{image}:latest")
        };
        let options = CreateImageOptionsBuilder::new().from_image(&image).build();
        let mut progress = pin!(self.docker.create_image(Some(options), None, None));
        while let Some(step) = progress.next().await {
            step?;
        }
        Ok(())
    }

    /// Runs an install script to its end and returns its exit code. The script
    /// runs as root in its own container and sees the server's directory at
    /// `/mnt/server`; each line it prints goes to `on_line`.
    pub async fn install(
        &self,
        server: &Server,
        install: &InstallScript<'_>,
        on_line: impl FnMut(&str),
    ) -> Result<i64, Error> {
        tokio::fs::create_dir_all(install.scratch).await?;
        tokio::fs::write(install.scratch.join("install.sh"), install.script).await?;

        let body = ContainerCreateBody {
            image: Some(install.image.to_owned()),
            cmd: Some(vec![
                install.entrypoint.to_owned(),
                "/mnt/install/install.sh".to_owned(),
            ]),
            env: Some(server.environment()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(true),
            host_config: Some(HostConfig {
                binds: Some(vec![
                    format!("{}:/mnt/server", server.dir.display()),
                    format!("{}:/mnt/install:ro", install.scratch.display()),
                ]),
                network_mode: Some(server.network.clone()),
                dns: Some(server.resolvers.clone()),
                memory: Some(INSTALL_MEMORY),
                memory_swap: Some(INSTALL_MEMORY),
                pids_limit: Some(PIDS),
                // An install script is root in its container, as eggs expect:
                // it installs packages and owns what it makes. What it does
                // not need is to write raw packets, with which it could pass
                // itself off as a neighbour on the servers' network, or to
                // make device files.
                cap_drop: Some(
                    ["NET_RAW", "MKNOD", "AUDIT_WRITE"]
                        .map(str::to_owned)
                        .to_vec(),
                ),
                log_config: Some(small_log()),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.run_to_end(&format!("{}-install", server.container()), body, on_line)
            .await
    }

    /// Gives everything in the server's directory to the server's user.
    ///
    /// Install scripts run as root and leave root-owned files behind. The fix
    /// runs in a container that can see nothing but that directory, so no link
    /// a script planted can point it anywhere else, and the host never has to
    /// walk a tree it does not trust. `image` only needs to contain `chown`.
    pub async fn own_files(&self, server: &Server, image: &str) -> Result<(), Error> {
        let body = ContainerCreateBody {
            image: Some(image.to_owned()),
            entrypoint: Some(vec!["chown".to_owned()]),
            cmd: Some(vec![
                "-hR".to_owned(),
                format!("{}:{}", server.uid, server.gid),
                "/mnt/server".to_owned(),
            ]),
            user: Some("0:0".to_owned()),
            host_config: Some(HostConfig {
                binds: Some(vec![format!("{}:/mnt/server", server.dir.display())]),
                network_mode: Some("none".to_owned()),
                cap_drop: Some(vec!["ALL".to_owned()]),
                cap_add: Some(vec!["CHOWN".to_owned(), "DAC_READ_SEARCH".to_owned()]),
                security_opt: Some(vec!["no-new-privileges".to_owned()]),
                log_config: Some(small_log()),
                ..Default::default()
            }),
            ..Default::default()
        };
        match self
            .run_to_end(&format!("{}-chown", server.container()), body, |_| {})
            .await?
        {
            0 => Ok(()),
            code => Err(Error::Ownership(code)),
        }
    }

    /// Runs a [`Listener`] to its end and returns its exit code; each line it
    /// prints goes to `on_line`. Its container is held as a server's is, and
    /// tighter: no files, little memory, few processes.
    pub async fn listen(
        &self,
        listener: &Listener<'_>,
        on_line: impl FnMut(&str),
    ) -> Result<i64, Error> {
        let key = format!("{}/tcp", listener.port);
        let binding = PortBinding {
            host_ip: Some("0.0.0.0".to_owned()),
            host_port: Some(listener.port.to_string()),
        };
        let body = ContainerCreateBody {
            image: Some(listener.image.to_owned()),
            entrypoint: Some(listener.command.clone()),
            cmd: Some(Vec::new()),
            user: Some(format!("{0}:{0}", listener.user)),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(true),
            exposed_ports: Some(vec![key.clone()]),
            labels: Some(HashMap::from([(
                "homewarp.probe".to_owned(),
                listener.port.to_string(),
            )])),
            host_config: Some(HostConfig {
                port_bindings: Some(HashMap::from([(key, Some(vec![binding]))])),
                network_mode: Some(listener.network.to_owned()),
                memory: Some(LISTENER_MEMORY),
                memory_swap: Some(LISTENER_MEMORY),
                pids_limit: Some(32),
                cap_drop: Some(vec!["ALL".to_owned()]),
                security_opt: Some(vec!["no-new-privileges".to_owned()]),
                readonly_rootfs: Some(true),
                log_config: Some(small_log()),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.run_to_end(listener.name, body, on_line).await
    }

    /// Ends a [`Listener`] that is still listening, by the name it was given.
    /// What ran it is then told that it ended.
    pub async fn end_listener(&self, name: &str) -> Result<(), Error> {
        self.remove_container(name).await
    }

    /// The address a running server has on its network. None if it is not running.
    pub async fn address_of(&self, server: &Server) -> Result<Option<IpAddr>, Error> {
        let found = match self
            .docker
            .inspect_container(&server.container(), None)
            .await
        {
            Ok(found) => found,
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => return Ok(None),
            Err(other) => return Err(other.into()),
        };
        Ok(found
            .network_settings
            .and_then(|settings| settings.networks)
            .and_then(|mut networks| networks.remove(&server.network))
            .and_then(|network| network.ip_address)
            .and_then(|address| address.parse().ok()))
    }

    /// The image the container with this id was made from, by its own id, which
    /// goes on naming it whatever its tags are moved to. None if the daemon
    /// knows no such container.
    pub async fn image_of(&self, container: &str) -> Result<Option<String>, Error> {
        match self.docker.inspect_container(container, None).await {
            Ok(found) => Ok(found.image),
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(other) => Err(other.into()),
        }
    }

    /// Creates the server's container, replacing one left from an earlier run.
    /// It is hardened as PLAN.md §5.6 lists: not root, no capabilities, no new
    /// privileges, a read-only root, and limits on memory and processes.
    pub async fn create(&self, server: &Server) -> Result<(), Error> {
        let mut exposed = Vec::new();
        let mut published = HashMap::new();
        for port in &server.ports {
            let key = format!(
                "{}/{}",
                port.port,
                if port.protocol == Protocol::Tcp {
                    "tcp"
                } else {
                    "udp"
                }
            );
            let binding = PortBinding {
                host_ip: Some(port.host_ip.to_string()),
                host_port: Some(port.port.to_string()),
            };
            exposed.push(key.clone());
            published.insert(key, Some(vec![binding]));
        }
        let body = ContainerCreateBody {
            image: Some(server.image.clone()),
            user: Some(format!("{}:{}", server.uid, server.gid)),
            env: Some(server.environment()),
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            open_stdin: Some(true),
            tty: Some(true),
            exposed_ports: Some(exposed),
            labels: Some(HashMap::from([(
                "homewarp.server".to_owned(),
                server.id.clone(),
            )])),
            host_config: Some(HostConfig {
                binds: Some(vec![format!("{}:/home/container", server.dir.display())]),
                port_bindings: Some(published),
                network_mode: Some(server.network.clone()),
                dns: Some(server.resolvers.clone()),
                memory: Some(server.memory_limit()),
                memory_swap: Some(server.memory_limit()),
                // Docker counts in billionths of a core.
                nano_cpus: (server.cpu_percent > 0)
                    .then(|| i64::from(server.cpu_percent) * 10_000_000),
                pids_limit: Some(PIDS),
                cap_drop: Some(vec!["ALL".to_owned()]),
                security_opt: Some(vec!["no-new-privileges".to_owned()]),
                readonly_rootfs: Some(true),
                tmpfs: Some(HashMap::from([(
                    "/tmp".to_owned(),
                    "rw,exec,nosuid,size=100M".to_owned(),
                )])),
                log_config: Some(small_log()),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.create_container(&server.container(), body).await
    }

    /// Attaches to the server's console. Attach before [`Engine::start`] to see the first lines.
    pub async fn attach(&self, server: &Server) -> Result<Console, Error> {
        self.attach_container(&server.container()).await
    }

    pub async fn start(&self, server: &Server) -> Result<(), Error> {
        Ok(self
            .docker
            .start_container(&server.container(), None)
            .await?)
    }

    /// Whether the server's container is running: one started before this
    /// process was, perhaps.
    pub async fn is_running(&self, server: &Server) -> Result<bool, Error> {
        match self
            .docker
            .inspect_container(&server.container(), None)
            .await
        {
            Ok(found) => Ok(found.state.and_then(|state| state.running) == Some(true)),
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(false),
            Err(other) => Err(other.into()),
        }
    }

    /// What the server is using, about once a second for as long as it is asked.
    pub fn usage(&self, server: &Server) -> impl Stream<Item = Usage> + Send {
        let options = StatsOptionsBuilder::new().stream(true).build();
        let mut before = None;
        self.docker
            .stats(&server.container(), Some(options))
            .filter_map(move |sample| {
                let usage = sample.ok().and_then(|sample| measure(&sample, &mut before));
                std::future::ready(usage)
            })
    }

    /// Sends a signal such as `SIGINT` to the server.
    pub async fn signal(&self, server: &Server, signal: &str) -> Result<(), Error> {
        let options = KillContainerOptionsBuilder::new().signal(signal).build();
        Ok(self
            .docker
            .kill_container(&server.container(), Some(options))
            .await?)
    }

    /// Waits for the server to exit and returns its exit code.
    pub async fn wait(&self, server: &Server) -> Result<i64, Error> {
        self.wait_container(&server.container()).await
    }

    /// Removes the server's container, running or not. Its files stay.
    pub async fn remove(&self, server: &Server) -> Result<(), Error> {
        self.remove_container(&server.container()).await
    }

    /// Removes every container the server with this id may have left behind,
    /// its install's among them. Its files stay.
    pub async fn forget(&self, id: &str) -> Result<(), Error> {
        for kind in ["", "-install", "-chown", "-standin"] {
            self.remove_container(&format!("homewarp-{id}{kind}"))
                .await?;
        }
        Ok(())
    }

    async fn create_container(&self, name: &str, body: ContainerCreateBody) -> Result<(), Error> {
        self.remove_container(name).await?;
        let options = CreateContainerOptionsBuilder::new().name(name).build();
        self.docker.create_container(Some(options), body).await?;
        Ok(())
    }

    async fn attach_container(&self, name: &str) -> Result<Console, Error> {
        let options = AttachContainerOptionsBuilder::new()
            .stream(true)
            .stdin(true)
            .stdout(true)
            .stderr(true)
            .build();
        Ok(Console::new(
            self.docker.attach_container(name, Some(options)).await?,
        ))
    }

    async fn wait_container(&self, name: &str) -> Result<i64, Error> {
        let mut exits = pin!(self.docker.wait_container(name, None));
        match exits.next().await {
            Some(Ok(exit)) => Ok(exit.status_code),
            // bollard reports a non-zero exit as an error; here it is an answer.
            Some(Err(DockerError::DockerContainerWaitError { code, .. })) => Ok(code),
            Some(Err(other)) => Err(other.into()),
            None => Err(Error::NoExitCode(name.to_owned())),
        }
    }

    async fn remove_container(&self, name: &str) -> Result<(), Error> {
        let options = RemoveContainerOptionsBuilder::new()
            .force(true)
            .v(true)
            .build();
        match self.docker.remove_container(name, Some(options)).await {
            Ok(())
            | Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(()),
            // Somebody else is removing it this moment: a server's own task
            // lets go of what stood in for it just as the server is forgotten.
            // It is going either way.
            Err(DockerError::DockerResponseServerError {
                status_code: 409,
                message,
            }) if message.contains("already in progress") => Ok(()),
            Err(other) => Err(other.into()),
        }
    }

    /// Runs a throwaway container to its end, handing its output to `on_line`.
    async fn run_to_end(
        &self,
        name: &str,
        body: ContainerCreateBody,
        mut on_line: impl FnMut(&str),
    ) -> Result<i64, Error> {
        self.create_container(name, body).await?;
        let mut console = self.attach_container(name).await?;
        self.docker.start_container(name, None).await?;
        while let Some(line) = console.next_line().await? {
            on_line(&line);
        }
        let code = self.wait_container(name).await?;
        self.remove_container(name).await?;
        Ok(code)
    }
}

fn small_log() -> HostConfigLogConfig {
    // The daemon refuses a single file with compression, which is its default.
    let config = [("max-size", "5m"), ("max-file", "1"), ("compress", "false")];
    HostConfigLogConfig {
        typ: Some("local".to_owned()),
        config: Some(
            config
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use bollard::models::{
        ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats, ContainerStatsResponse,
    };

    use super::{Port, Protocol, Server, measure};

    /// A report from Docker with the totals of processor time so far.
    fn report(used: u64, passed: u64, memory: u64, cache: u64) -> ContainerStatsResponse {
        ContainerStatsResponse {
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(used),
                    ..Default::default()
                }),
                system_cpu_usage: Some(passed),
                online_cpus: Some(2),
                ..Default::default()
            }),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(memory),
                limit: Some(1000),
                stats: Some([("inactive_file".to_owned(), cache)].into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn measures_usage_between_one_report_and_the_next() {
        let mut before = None;
        // The first has nothing before it to be measured against.
        assert_eq!(measure(&report(100, 1000, 500, 100), &mut before), None);
        // A quarter of what both cores had to give is half a core.
        let usage = measure(&report(600, 3000, 700, 200), &mut before).unwrap();
        assert_eq!(usage.cpu_percent, 50.0);
        assert_eq!((usage.memory_bytes, usage.memory_limit_bytes), (500, 1000));
        // A report in which no time has passed says nothing.
        assert_eq!(measure(&report(600, 3000, 700, 200), &mut before), None);
    }

    fn server(memory_mb: u32) -> Server {
        Server {
            id: "abc".to_owned(),
            dir: "/srv/abc".into(),
            image: "example.invalid/java:25".to_owned(),
            startup: "java -jar server.jar".to_owned(),
            variables: vec![("SERVER_JARFILE".to_owned(), "server.jar".to_owned())],
            memory_mb,
            cpu_percent: 0,
            ports: vec![Port {
                host_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port: 25565,
                protocol: Protocol::Tcp,
            }],
            uid: 4857,
            gid: 4857,
            network: "homewarp-br".to_owned(),
            timezone: "UTC".to_owned(),
            resolvers: super::RESOLVERS.map(str::to_owned).to_vec(),
        }
    }

    #[test]
    fn gives_a_server_the_environment_eggs_expect() {
        let environment = server(1024).environment();
        for expected in [
            "STARTUP=java -jar server.jar",
            "SERVER_MEMORY=1024",
            "SERVER_IP=0.0.0.0",
            "SERVER_PORT=25565",
            "P_SERVER_UUID=abc",
            "SERVER_JARFILE=server.jar",
        ] {
            assert!(
                environment.iter().any(|entry| entry == expected),
                "{expected} in {environment:?}"
            );
        }
    }

    #[test]
    fn leaves_headroom_above_the_memory_asked_for() {
        const MB: i64 = 1024 * 1024;
        assert_eq!(server(2048).memory_limit(), 2048 * MB * 115 / 100);
        assert_eq!(server(4096).memory_limit(), 4096 * MB * 110 / 100);
        assert_eq!(server(8192).memory_limit(), 8192 * MB * 105 / 100);
    }
}
