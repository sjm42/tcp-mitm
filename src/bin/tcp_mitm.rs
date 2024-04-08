// bin/tcp_mitm.rs

use std::{net::SocketAddr, time};

use clap::Parser;
use log::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{self, TcpStream};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::mpsc;

use tcp_mitm::*;

const BUFSZ: usize = 65536;
const CHANSZ: usize = 1024;


fn main() -> anyhow::Result<()> {
    let opts = OptsCommon::parse();
    start_pgm(&opts, "TCP-mitm");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        if let Err(e) = run_mitm(&opts).await {
            error!("Error: {}", e);
        }
    });
    runtime.shutdown_timeout(time::Duration::new(5, 0));
    info!("Exit.");
    Ok(())
}


async fn run_mitm(opts: &OptsCommon) -> anyhow::Result<()> {
    let listen = &opts.listen;
    info!("Listening on {listen}...");
    let listener = net::TcpListener::bind(&listen).await?;

    loop {
        let (sock, addr) = match listener.accept().await {
            Err(e) => {
                error!("accept failed: {e:?}");
                continue;
            }
            Ok(x) => x,
        };

        let serv = opts.server.clone();
        let tap = opts.tap.clone();
        tokio::spawn(async move {
            let ret = handle_client(sock, addr, serv, tap).await;
            if let Err(e) = ret {
                // log errors
                error!("client error: {e:?}");
            }
        });
    }
}

async fn handle_client(
    client: TcpStream,
    addr: SocketAddr,
    serv: String,
    tap: String,
) -> anyhow::Result<()> {
    info!("Client connection from {addr:?}");

    let mut buf_c = [0; BUFSZ];
    let mut buf_s = [0; BUFSZ];

    let (mut client_r, client_w) = client.into_split();

    info!("Connecting to server {serv}...");
    let (mut serv_r, serv_w) = TcpStream::connect(serv).await?.into_split();
    info!("Server connected.");

    info!("Connecting to tap {tap}...");
    let tap = TcpStream::connect(tap).await?;
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
