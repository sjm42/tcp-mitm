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

    let t_cw = tokio::spawn(run_tcp_writer(format!("clientW #{i}"), client_rx, client_w));
    let t_sw = tokio::spawn(run_tcp_writer(format!("servW #{i}"), serv_rx, serv_w));
    let t_tap = tokio::spawn(handle_tap(format!("tap #{i}"), tap_rx, tap));

    // https://docs.rs/tokio/1.37.0/tokio/macro.select.html#cancellation-safety

    // we don't need to use .readable() at tokio::select!() top level entries
    // because read() is already cancel safe

    let mut buf_c = [0; BUFSZ];
    let mut buf_s = [0; BUFSZ];
    let mut bytes_c: usize = 0;
    let mut bytes_s: usize = 0;
    let mut bytes_t: usize = 0;

    loop {
        tokio::select! {
            res = client_r.read(&mut buf_c) => {
                let n = match res {
                    Ok(n) => n,
                    Err(e) => {
                        error!("#{i} client read error: {e}");
                        break;
                    }
                };

                if n == 0 {
                    info!("#{i} client disconnected: {addr:?}");
                    break;
                }

                debug!("#{i} client read {n}");
                bytes_c += n;
                bytes_t += n;
                serv_tx.send(buf_c[0..n].to_owned()).await?;
                tap_tx.send(buf_c[0..n].to_owned()).await?;
            }

            res = serv_r.read(&mut buf_s) => {
                let n = match res {
                    Ok(n) => n,
                    Err(e) => {
                        error!("#{i} server read error: {e}");
                        break;
                    }
                };

                if n == 0 {
                    info!("#{i} server disconnected.");
                    break;
                }

                debug!("#{i} server read {n}");
                bytes_s += n;
                bytes_t += n;
                client_tx.send(buf_s[0..n].to_owned()).await?;
                tap_tx.send(buf_s[0..n].to_owned()).await?;
            }

            else => {
                error!("#{i} select error");
                break;
            }
        }
    }

    drop(client_r);
    drop(serv_r);

    drop(client_tx);
    drop(serv_tx);
    drop(tap_tx);

    let (r_cw, r_sw, r_tap) = tokio::join!(
        t_cw,
        t_sw,
        t_tap
    );
    info!("#{i} client writer status: {r_cw:?}");
    info!("#{i} server writer status: {r_sw:?}");
    info!("#{i} tap status: {r_tap:?}");

    info!("#{i} client read {bytes_c} bytes, server read {bytes_s} bytes, tapped {bytes_t} bytes.");
    info!("#{i} connection closed.");
    Ok(())
}


async fn run_tcp_writer(
    id: String,
    mut c: mpsc::Receiver<Vec<u8>>,
    mut w: OwnedWriteHalf,
) -> anyhow::Result<usize> {
    let mut bytes: usize = 0;
    while let Some(m) = c.recv().await {
        w.write_all(&m).await?;
        bytes += m.len();
    }
    info!("{id} closed.");
    Ok(bytes)
}


async fn handle_tap(id: String, mut rx: mpsc::Receiver<Vec<u8>>, tap: SocketAddr) -> anyhow::Result<usize> {
    let mut bytes: usize = 0;

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

        if let Some(tap) = otap.take() {
            match feed_tap(&id, &mut rx, tap).await {
                Ok(b) => {
                    info!("{id} tap closed");
                    bytes += b;
                }
                Err(e) => { error!("{id} tap error {e}") }
            }
        }
        // at this point, either receiver is closed or tap is closed/errored

        if rx.is_closed() {
            info!("{id} tap receiver closed.");
            return Ok(bytes);
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

async fn feed_tap(id: &str, rx: &mut mpsc::Receiver<Vec<u8>>, mut tap: TcpStream) -> anyhow::Result<usize> {
    let mut bytes: usize = 0;
    let mut buf = [0; BUFSZ];

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(m) => {
                        tap.write_all(&m).await?;
                        bytes += m.len();
                    }
                    None => {
                        info!("{id} closed, sender gone.");
                        // TODO: make this configurable
                        // hack: send something extra with linefeeds to tap when closing.
                        tap.write_all("\n<EOF>\n".as_bytes()).await?;
                        return Ok(bytes);
                    }
                }
            }

            res = tap.read(&mut buf) => {
                let n = res?;
                if n == 0 {
                    info!("{id} closed, tap disconnected.");
                    return Ok(bytes);
                }
                debug!("{id} tap read {n} (ignored)");
            }
        }
    }
}

// EOF
