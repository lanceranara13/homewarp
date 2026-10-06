//! Phase 0 runtime spike (PLAN.md §11): an unmodified Paper egg from start to
//! finish. Install container, server container, console, a status ping the way
//! a Minecraft client sends one, and a clean stop.
//!
//! `scripts/dev.sh paper` runs it on the homelab. It talks to the Docker daemon
//! on the local socket and runs as root, as Core will.

use std::{
    env,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use homewarp_runtime::{
    Engine, InstallScript, Network, Port, Protocol, Server, ServerDir, strip_ansi,
};
use homewarp_template::{Parser, Stop, properties, substitute};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const ID: &str = "spike-paper";
const NETWORK: &str = "homewarp-br";

fn var(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is not set"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let data = PathBuf::from(var("HOMEWARP_DATA")?);
    let port: u16 = var("HOMEWARP_PORT")?.parse().context("HOMEWARP_PORT")?;
    let memory_mb: u32 = var("HOMEWARP_MEMORY")?.parse().context("HOMEWARP_MEMORY")?;
    let user_agent = var("HOMEWARP_USER_AGENT")?;
    let accept_eula = env::var("HOMEWARP_ACCEPT_EULA").is_ok_and(|value| value == "1");

    let template = homewarp_template::import(&std::fs::read_to_string(var("HOMEWARP_EGG")?)?)?;
    let image = &template.images[0];
    println!(
        "template   {} ({} variables), image {}",
        template.name,
        template.variables.len(),
        image.image
    );

    // Every variable at its default, except the one this egg leaves to the operator.
    let variables: Vec<(String, String)> = template
        .variables
        .iter()
        .map(|variable| {
            let value = if variable.env == "USER_AGENT" {
                user_agent.clone()
            } else {
                variable.default.clone()
            };
            (variable.env.clone(), value)
        })
        .collect();
    let variable = |name: &str| {
        variables
            .iter()
            .find(|(env, _)| env == name)
            .map(|(_, value)| value.clone())
    };
    let startup = substitute(&template.startup, |name| match name {
        "SERVER_MEMORY" => Some(memory_mb.to_string()),
        "SERVER_IP" => Some("0.0.0.0".to_owned()),
        "SERVER_PORT" => Some(port.to_string()),
        other => variable(other),
    });

    // Published on loopback only: this is a test server, not one to offer the LAN.
    let host_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let server = Server {
        id: ID.to_owned(),
        dir: data.join("servers").join(ID),
        image: image.image.clone(),
        startup,
        variables: variables.clone(),
        memory_mb,
        cpu_percent: 0,
        ports: vec![
            Port {
                host_ip,
                port,
                protocol: Protocol::Tcp,
            },
            Port {
                host_ip,
                port,
                protocol: Protocol::Udp,
            },
        ],
        uid: 4857,
        gid: 4857,
        network: NETWORK.to_owned(),
        timezone: "UTC".to_owned(),
        resolvers: homewarp_runtime::RESOLVERS.map(str::to_owned).to_vec(),
    };

    let engine = Engine::connect()?;
    engine
        .ensure_network(&Network {
            name: NETWORK.to_owned(),
            subnet: "10.213.80.0/24".to_owned(),
        })
        .await?;
    let files = ServerDir::open(&server.dir, server.uid, server.gid)?;
    let total = Instant::now();

    if let Some(install) = &template.install {
        step("pull the install image", engine.pull(&install.image)).await?;
        let script = InstallScript {
            image: &install.image,
            entrypoint: &install.entrypoint,
            script: &install.script,
            scratch: &data.join("install").join(ID),
        };
        let code = step(
            "run the install script",
            engine.install(&server, &script, |line| show("install", line)),
        )
        .await?;
        ensure!(code == 0, "the install script exited with {code}");
        step(
            "hand the files to the server's user",
            engine.own_files(&server, &install.image),
        )
        .await?;
    }
    step("pull the server image", engine.pull(&server.image)).await?;

    for file in &template.config_files {
        ensure!(
            file.parser == Parser::Properties,
            "{}: only the properties parser is written so far",
            file.path
        );
        let pairs: Vec<(String, String)> = file
            .find
            .iter()
            .map(|replacement| {
                let value = substitute(&replacement.value, |name| match name {
                    "server.build.default.port" | "server.allocations.default.port" => {
                        Some(port.to_string())
                    }
                    "server.build.default.ip" | "server.allocations.default.ip" => {
                        Some("0.0.0.0".to_owned())
                    }
                    other => variable(other.strip_prefix("server.build.env.")?),
                });
                ensure!(!value.contains("{{"), "{}: no value for {value}", file.path);
                Ok((replacement.key.clone(), value))
            })
            .collect::<Result<_>>()?;
        let before = files.read_to_string(&file.path)?.unwrap_or_default();
        files.write(&file.path, &properties::patch(&before, &pairs))?;
        println!("patched    {} ({} keys)", file.path, pairs.len());
    }
    if accept_eula {
        files.write("eula.txt", "eula=true\n")?;
    }

    engine.create(&server).await?;
    let mut console = engine.attach(&server).await?;
    engine.start(&server).await?;

    let started = step("start, until the console says done", async {
        let read = async {
            while let Some(line) = console.next_line().await? {
                let line = strip_ansi(&line);
                show("console", &line);
                if template
                    .done
                    .iter()
                    .any(|done| line.contains(done.as_str()))
                {
                    return Ok(true);
                }
            }
            anyhow::Ok(false)
        };
        timeout(Duration::from_secs(300), read)
            .await
            .context("no sign of a start within five minutes")?
    })
    .await?;
    ensure!(started, "the server exited before it had started");

    let status = step("answer a status ping on the published port", status(port)).await?;
    println!(
        "status     {} · {}/{} players",
        status["version"]["name"].as_str().unwrap_or("?"),
        status["players"]["online"],
        status["players"]["max"]
    );

    console.send("list").await?;
    let listed = async {
        while let Some(line) = console.next_line().await? {
            let line = strip_ansi(&line);
            if line.contains("players online") {
                return Ok(line);
            }
        }
        bail!("the console closed before answering")
    };
    let answer = timeout(Duration::from_secs(15), listed)
        .await
        .context("no answer to `list`")??;
    println!("command    list → {}", answer.trim());

    let code = step("stop", async {
        match &template.stop {
            Stop::Command(command) => console.send(command).await?,
            Stop::Signal(signal) => engine.signal(&server, signal).await?,
        }
        let drain = async {
            while let Some(line) = console.next_line().await? {
                show("console", &strip_ansi(&line));
            }
            anyhow::Ok(())
        };
        timeout(Duration::from_secs(90), drain)
            .await
            .context("still running 90 s after being asked to stop")??;
        anyhow::Ok(engine.wait(&server).await?)
    })
    .await?;
    engine.remove(&server).await?;
    ensure!(code == 0, "the server exited with {code}");

    println!(
        "done       exit code 0, {:.0} s in all",
        total.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Runs one step and reports how long it took.
async fn step<T, E>(label: &str, work: impl Future<Output = Result<T, E>>) -> Result<T>
where
    E: Into<anyhow::Error>,
{
    let started = Instant::now();
    let value = work
        .await
        .map_err(Into::<anyhow::Error>::into)
        .with_context(|| label.to_owned())?;
    println!("{:>8.1} s  {label}", started.elapsed().as_secs_f32());
    Ok(value)
}

fn show(from: &str, line: &str) {
    let line: String = line.chars().take(150).collect();
    println!("           {from:<7} | {line}");
}

/// Minecraft's Server List Ping: what the multiplayer screen sends before a join.
async fn status(port: u16) -> Result<serde_json::Value> {
    const HOST: &[u8] = b"127.0.0.1";
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;

    let mut handshake = vec![0x00]; // packet id
    handshake.push(0x00); // protocol version; a status request is answered whatever it says
    handshake.push(HOST.len() as u8);
    handshake.extend_from_slice(HOST);
    handshake.extend_from_slice(&port.to_be_bytes());
    handshake.push(0x01); // next state: status
    let mut request = vec![handshake.len() as u8];
    request.extend_from_slice(&handshake);
    request.extend_from_slice(&[0x01, 0x00]); // a one-byte packet: status request
    stream.write_all(&request).await?;

    let _packet_length = read_varint(&mut stream).await?;
    ensure!(
        read_varint(&mut stream).await? == 0,
        "not a status response"
    );
    let length = read_varint(&mut stream).await? as usize;
    ensure!(length <= 1 << 20, "a status response of {length} bytes");
    let mut json = vec![0; length];
    stream.read_exact(&mut json).await?;
    Ok(serde_json::from_slice(&json)?)
}

async fn read_varint(stream: &mut TcpStream) -> Result<u32> {
    let mut value = 0;
    for shift in (0..35).step_by(7) {
        let byte = stream.read_u8().await?;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("a varint longer than five bytes")
}
