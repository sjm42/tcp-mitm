// bin/tcp_mitm.rs

use clap::Parser;
use log::*;
use std::{net::SocketAddr, time};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{self, TcpStream};
use tokio::sync::mpsc;

use tcp_mitm::*;

const BUFSZ: usize = 65536;
const CHANSZ: usize = 256;

fn main() -> anyhow::Result<()> {
    let opts = OptsCommon::parse();
    start_pgm(&opts, "TCP-mitm");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        if let Err(e) = run_mitm(opts).await {
            error!("Error: {}", e);
        }
    });
    runtime.shutdown_timeout(time::Duration::new(5, 0));
    info!("Exit.");
    Ok(())
}

async fn run_mitm(opts: OptsCommon) -> anyhow::Result<()> {
    let listener = net::TcpListener::bind(&opts.listen).await?;
    info!("Listening on {}", &opts.listen);

    loop {
        let (sock, addr) = match listener.accept().await {
            Err(e) => {
                error!("accept failed: {e:?}");
                continue;
            }
            Ok(x) => x,
        };

        let o = opts.clone();
        tokio::spawn(async move {
            let ret = handle_client(o, sock, addr).await;
            if let Err(e) = ret {
                // log errors
                error!("client error: {e:?}");
            }
        });
    }
}

async fn handle_client(
    opts: OptsCommon,
    client: net::TcpStream,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    info!("Client connection from {addr:?}");

    let mut buf_c = [0; BUFSZ];
    let mut buf_s = [0; BUFSZ];

    let (mut client_r, client_w) = client.into_split();

    let serv_a = opts.server;
    info!("Connecting to server {serv_a}...");
    let (mut serv_r, serv_w) = net::TcpStream::connect(serv_a).await?.into_split();
    info!("Server connected.");

    let tap_a = opts.tap;
    info!("Connecting to tap {tap_a}...");
    let tap = net::TcpStream::connect(tap_a).await?;
    info!("Tap connected.");

    let (client_tx, client_rx) = mpsc::channel(CHANSZ);
    let (serv_tx, serv_rx) = mpsc::channel(CHANSZ);
    let (tap_tx, tap_rx) = mpsc::channel(CHANSZ);

    tokio::spawn(run_tcp_writer(client_rx, client_w));
    tokio::spawn(run_tcp_writer(serv_rx, serv_w));
    tokio::spawn(handle_tap(tap_rx, tap));

    loop {
        tokio::select! {
            res = client_r.read(&mut buf_c) => {
                let n = res?;
                if n == 0 {
                    info!("Client disconnected: {addr:?}");
                    return Ok(());
                }

                debug!("Client read #{n}");
                serv_tx.send(buf_c[0..n].to_owned()).await?;
                tap_tx.send(buf_c[0..n].to_owned()).await?;
            }

            res = serv_r.read(&mut buf_s) => {
                let n = res?;
                if n == 0 {
                    info!("Server disconnected.");
                    return Ok(());
                }

                debug!("Server read #{n}");
                client_tx.send(buf_s[0..n].to_owned()).await?;
                tap_tx.send(buf_s[0..n].to_owned()).await?;
            }
        }
    }
}

async fn run_tcp_writer(
    mut c: mpsc::Receiver<Vec<u8>>,
    mut w: OwnedWriteHalf,
) -> anyhow::Result<()> {
    while let Some(m) = c.recv().await {
        w.write_all(&m).await?;
    }
    Ok(())
}

async fn handle_tap(mut c: mpsc::Receiver<Vec<u8>>, mut t: TcpStream) -> anyhow::Result<()> {
    let mut buf = [0; BUFSZ];

    loop {
        tokio::select! {
            msg = c.recv() => {
                match msg {
                    Some(m) => {
                        t.write_all(&m).await?;
                    }
                    None => {
                        info!("Client closed.");
                        return Ok(());
                    }
                }
            }

            res = t.read(&mut buf) => {
                let n = res?;
                if n == 0 {
                    info!("Tap disconnected.");
                    return Ok(());
                }
                debug!("Tap read #{n} (ignored)");
            }
        }
    }
}

// EOF
