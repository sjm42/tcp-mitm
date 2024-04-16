// startup.rs

use std::{collections::HashSet, env};
use std::net::{IpAddr, SocketAddr};

use anyhow::bail;
use clap::Parser;
use tracing::*;

#[derive(Clone, Debug, Parser)]
pub struct OptsCommon {
    #[arg(short, long)]
    pub debug: bool,
    #[arg(short, long)]
    pub trace: bool,

    #[arg(long)]
    pub listen: IpAddr,
    #[arg(long)]
    pub ports: String,
    #[arg(long)]
    pub server: IpAddr,
    #[arg(long)]
    pub tap: SocketAddr,
    #[clap(skip)]
    pub ports_v: Vec<u16>,
}

impl OptsCommon {
    pub fn finalize(mut self) -> anyhow::Result<Self> {
        let mut ports_h = HashSet::new();
        for p in self.ports.split(',') {
            let p_r: Vec<&str> = p.split('-').collect();
            match p_r.len() {
                1 => { ports_h.insert(p_r[0].parse::<u16>()?); }
                2 => {
                    let p1 = p_r[0].parse::<u16>()?;
                    let p2 = p_r[1].parse::<u16>()?;
                    if p1 > p2 { bail!("Port range {p}: second port must be higher"); }
                    for i in p1..=p2 { ports_h.insert(i); }
                }
                _ => { bail!("Cannot parse port range: {p}"); }
            }
        }
        let mut ports_v
            = ports_h.iter().copied().collect::<Vec<u16>>();
        ports_v.sort();
        self.ports_v = ports_v;
        Ok(self)
    }

    pub fn get_loglevel(&self) -> Level {
        if self.trace {
            Level::TRACE
        } else if self.debug {
            Level::DEBUG
        } else {
            Level::INFO
        }
    }

    pub fn start_pgm(&self, name: &str) {
        tracing_subscriber::fmt()
            .with_max_level(self.get_loglevel())
            .with_target(false)
            .init();

        info!("Starting up {name}...");
        debug!("Git branch: {}", env!("GIT_BRANCH"));
        debug!("Git commit: {}", env!("GIT_COMMIT"));
        debug!("Source timestamp: {}", env!("SOURCE_TIMESTAMP"));
        debug!("Compiler version: {}", env!("RUSTC_VERSION"));
    }
}


// EOF
