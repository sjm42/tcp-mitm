// startup.rs

use std::env;

use clap::Parser;
use log::*;

#[derive(Clone, Debug, Default, Parser)]
pub struct OptsCommon {
    #[arg(short, long)]
    pub debug: bool,
    #[arg(short, long)]
    pub trace: bool,

    #[arg(long)]
    pub listen: String,
    #[arg(long)]
    pub server: String,
    #[arg(long)]
    pub tap: String,
}

impl OptsCommon {
    pub fn finish(&mut self) -> anyhow::Result<()> {
        // do sanity checking or env var expansion etc...
        Ok(())
    }
    pub fn get_loglevel(&self) -> LevelFilter {
        if self.trace {
            LevelFilter::Trace
        } else if self.debug {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        }
    }
}

pub fn start_pgm(opts: &OptsCommon, desc: &str) {
    env_logger::Builder::new()
        .filter_level(opts.get_loglevel())
        .format_timestamp_secs()
        .init();
    info!("Starting up {desc}...");
    debug!("Git branch: {}", env!("GIT_BRANCH"));
    debug!("Git commit: {}", env!("GIT_COMMIT"));
    debug!("Source timestamp: {}", env!("SOURCE_TIMESTAMP"));
    debug!("Compiler version: {}", env!("RUSTC_VERSION"));
}
// EOF
