// bin/tcp_mitm.rs

use std::{net::SocketAddr, time};
use std::time::Duration;

use anyhow::bail;
use clap::Parser;
use log::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{self, tcp::OwnedWriteHalf, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

use tcp_mitm::*;

const BUFSZ: usize = 65536;
const CHANSZ: usize = 1024;


fn main() -> anyhow::Result<()> {
    let opts = OptsCommon::parse();
    start_pgm(&opts, "tcp_mitm");

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

    let mut i: usize = 0;
    loop {
        let (sock, addr) = match listener.accept().await {
            Err(e) => {
                error!("accept failed: {e:?}");
                continue;
            }
            Ok(x) => x,
        };

        i += 1;
        let serv = opts.server;
        let tap = opts.tap;
        tokio::spawn(async move {
            let ret = handle_client(i, sock, addr, serv, tap).await;
            if let Err(e) = ret {
                // log errors
                error!("{e:?}");
            }
        });
    }
}

async fn handle_client(
    i: usize,
    client: TcpStream,
    addr: SocketAddr,
    serv: SocketAddr,
    tap: SocketAddr,
) -> anyhow::Result<()> {
    info!("#{i} client connection from {addr:?}");

    let mut buf = [0; BUFSZ];
    let (mut client_r, client_w) = client.into_split();

    info!("#{i} connecting to server {serv}...");
    let serv = match timeout(Duration::from_secs(5), TcpStream::connect(serv)).await
    {
        Err(_) => {
            bail!("#{i} server connection timeout");
        }
        Ok(r) => {
            match r {
                Ok(c) => {
                    info!("#{i} server connected.");
                    c
                }
                Err(e) => {
                    bail!("#{i} server connection error: {e}");
                }
            }
        }
    };
    let (mut serv_r, serv_w) = serv.into_split();

    let (client_tx, client_rx) = mpsc::channel(CHANSZ);
    let (serv_tx, serv_rx) = mpsc::channel(CHANSZ);
    let (tap_tx, tap_rx) = mpsc::channel(CHANSZ);

    tokio::spawn(run_tcp_writer(format!("clientW #{i}"), client_rx, client_w));
    tokio::spawn(run_tcp_writer(format!("servW #{i}"), serv_rx, serv_w));
    tokio::spawn(handle_tap(format!("tap #{i}"), tap_rx, tap));

    loop {
        tokio::select! {
            Ok(_) = client_r.readable() => {
                let n = client_r.read(&mut buf).await?;
                if n == 0 {
                    info!("#{i} client disconnected: {addr:?}");
                    return Ok(());
                }

                debug!("#{i} client read {n}");
                serv_tx.send(buf[0..n].to_owned()).await?;
                tap_tx.send(buf[0..n].to_owned()).await?;
            }

            Ok(_) = serv_r.readable() => {
                let n = serv_r.read(&mut buf).await?;
                if n == 0 {
                    info!("#{i} server disconnected.");
                    return Ok(());
                }

                debug!("#{i} server read {n}");
                client_tx.send(buf[0..n].to_owned()).await?;
                tap_tx.send(buf[0..n].to_owned()).await?;
            }

            else => {
                bail!("#{i} select error");
            }
        }
    }
}


async fn run_tcp_writer(
    id: String,
    mut c: mpsc::Receiver<Vec<u8>>,
    mut w: OwnedWriteHalf,
) -> anyhow::Result<()> {
    while let Some(m) = c.recv().await {
        w.write_all(&m).await?;
    }
    info!("{id} closed.");
    Ok(())
}


async fn handle_tap(id: String, mut rx: mpsc::Receiver<Vec<u8>>, tap: SocketAddr) -> anyhow::Result<()> {
    loop {
        info!("{id} connecting to tap {tap}...");
        let mut otap = match timeout(Duration::from_secs(2), TcpStream::connect(tap)).await
        {
            Err(_) => {
                error!("{id} connection timeout");
                None
            }
            Ok(r) => {
                match r {
                    Ok(c) => {
                        info!("{id} tap connected.");
                        Some(c)
                    }
                    Err(e) => {
                        error!("{id} connection error: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        None
                    }
                }
            }
        };

        if let Some(mut tap) = otap.take() {
            match feed_tap(&id, &mut rx, &mut tap).await {
                Ok(_) => { info!("{id} tap closed") }
                Err(e) => { error!("{id} tap error {e}") }
            }
        }
        // at this point, either receiver is closed or tap is closed/errored

        if rx.is_closed() {
            info!("{id} tap receiver closed.");
            return Ok(());
        }

        // flush incoming msgs before new try
        let n_msg = rx.len();
        if n_msg > 0 {
            info!("{id} flushing {n_msg} messages");
            for _ in 0..n_msg {
                rx.recv().await;
            }
        }
    }
}

async fn feed_tap(id: &str, rx: &mut mpsc::Receiver<Vec<u8>>, tap: &mut TcpStream) -> anyhow::Result<()> {
    let mut buf = [0; BUFSZ];
    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(m) => {
                        tap.write_all(&m).await?;
                    }
                    None => {
                        info!("{id} closed, sender gone.");
                        return Ok(());
                    }
                }
            }

            res = tap.read(&mut buf) => {
                let n = res?;
                if n == 0 {
                    info!("{id} closed, tap disconnected.");
                    return Ok(());
                }
                debug!("{id} tap read {n} (ignored)");
            }
        }
    }
}

// EOF
