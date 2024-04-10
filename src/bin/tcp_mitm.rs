// bin/tcp_mitm.rs

use std::{net::SocketAddr, time};
use std::time::Duration;

use anyhow::bail;
use clap::Parser;
use futures::stream::{SelectAll, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{self, tcp::OwnedWriteHalf, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_stream::wrappers::TcpListenerStream;
use tracing::*;

use tcp_mitm::*;

const BUFSZ: usize = 65536;
const CHANSZ: usize = 1024;


fn main() -> anyhow::Result<()> {
    let opts = OptsCommon::parse().finalize()?;
    tracing_subscriber::fmt()
        .with_max_level(opts.get_loglevel())
        .with_target(false)
        .init();
    start_pgm("tcp_mitm");

    debug!("Going to listen on ports {:?}", opts.ports_v);
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
    let mut listeners = SelectAll::new();
    for port in opts.ports_v.iter() {
        let sock_addr = SocketAddr::new(opts.listen, *port);
        listeners.push(TcpListenerStream::new(net::TcpListener::bind(sock_addr).await?));
        info!("Listening on {sock_addr}...");
    }

    let mut i: usize = 0;
    while let Some(conn) = listeners.next().await {
        let sock = match conn {
            Err(e) => {
                error!("accept failed: {e:?}");
                continue;
            }
            Ok(x) => x,
        };

        i += 1;
        let local_port = sock.local_addr()?.port();
        let serv = SocketAddr::new(opts.server, local_port);
        let tap = opts.tap;
        info!("id={i} new connection on port {local_port}" );

        tokio::spawn(async move {
            let ret = handle_client(sock, serv, tap).instrument(tracing::info_span!("conn", id = i)).await;
            if let Err(e) = ret {
                // log errors
                error!("{e:?}");
            }
        });
    }
    Ok(())
}

async fn handle_client(
    client: TcpStream,
    serv: SocketAddr,
    tap: SocketAddr,
) -> anyhow::Result<()> {
    let peer = client.peer_addr()?;
    info!("client connection from {peer:?}", );

    let (mut client_r, client_w) = client.into_split();

    info!("connecting to server {serv}...");
    let serv = match timeout(Duration::from_secs(5), TcpStream::connect(serv)).await
    {
        Err(_) => {
            bail!("server connection timeout");
        }
        Ok(r) => {
            match r {
                Ok(c) => {
                    info!("server connected.");
                    c
                }
                Err(e) => {
                    bail!("server connection error: {e}");
                }
            }
        }
    };
    let (mut serv_r, serv_w) = serv.into_split();

    let (client_tx, client_rx) = mpsc::channel(CHANSZ);
    let (serv_tx, serv_rx) = mpsc::channel(CHANSZ);
    let (tap_tx, tap_rx) = mpsc::channel(CHANSZ);

    let t_cw = tokio::spawn(run_tcp_writer(client_rx, client_w).instrument(info_span!("writer", side = "client").or_current()));
    let t_sw = tokio::spawn(run_tcp_writer(serv_rx, serv_w).instrument(info_span!("writer", side = "server").or_current()));
    let t_tap = tokio::spawn(handle_tap(tap_rx, tap).instrument(info_span!("tap")));

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
                        error!("client read error: {e}");
                        break;
                    }
                };

                if n == 0 {
                    info!("client disconnected");
                    break;
                }

                debug!("client read {n}");
                bytes_c += n;
                bytes_t += n;
                serv_tx.send(buf_c[0..n].to_owned()).await?;
                tap_tx.send(buf_c[0..n].to_owned()).await?;
            }

            res = serv_r.read(&mut buf_s) => {
                let n = match res {
                    Ok(n) => n,
                    Err(e) => {
                        error!("server read error: {e}");
                        break;
                    }
                };

                if n == 0 {
                    info!("server disconnected.");
                    break;
                }

                debug!("server read {n}");
                bytes_s += n;
                bytes_t += n;
                client_tx.send(buf_s[0..n].to_owned()).await?;
                tap_tx.send(buf_s[0..n].to_owned()).await?;
            }

            else => {
                error!("select error");
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
    info!("client writer status: {r_cw:?}");
    info!("server writer status: {r_sw:?}");
    info!("tap status: {r_tap:?}");

    info!("Finished: client read {bytes_c} bytes, server read {bytes_s} bytes, tapped {bytes_t} bytes.");
    Ok(())
}


async fn run_tcp_writer(
    mut c: mpsc::Receiver<Vec<u8>>,
    mut w: OwnedWriteHalf,
) -> anyhow::Result<usize> {
    let mut bytes: usize = 0;
    while let Some(m) = c.recv().await {
        w.write_all(&m).await?;
        bytes += m.len();
    }
    info!("closed.");
    Ok(bytes)
}


async fn handle_tap(mut rx: mpsc::Receiver<Vec<u8>>, tap: SocketAddr) -> anyhow::Result<usize> {
    let mut bytes: usize = 0;

    loop {
        info!("connecting to tap {tap}...");
        let mut otap = match timeout(Duration::from_secs(2), TcpStream::connect(tap)).await
        {
            Err(_) => {
                error!("connection timeout");
                None
            }
            Ok(r) => {
                match r {
                    Ok(c) => {
                        info!("tap connected.");
                        Some(c)
                    }
                    Err(e) => {
                        error!("connection error: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        None
                    }
                }
            }
        };

        if let Some(tap) = otap.take() {
            match feed_tap(&mut rx, tap).await {
                Ok(b) => {
                    info!("tap closed");
                    bytes += b;
                }
                Err(e) => { error!("tap error {e}") }
            }
        }
        // at this point, either receiver is closed or tap is closed/errored

        if rx.is_closed() {
            info!("tap receiver closed.");
            return Ok(bytes);
        }

        // flush incoming msgs before new try
        let n_msg = rx.len();
        if n_msg > 0 {
            info!("flushing {n_msg} messages");
            for _ in 0..n_msg {
                rx.recv().await;
            }
        }
    }
}

async fn feed_tap(rx: &mut mpsc::Receiver<Vec<u8>>, mut tap: TcpStream) -> anyhow::Result<usize> {
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
                        info!("closed, sender gone.");
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
                    info!("closed, tap disconnected.");
                    return Ok(bytes);
                }
                debug!("tap read {n} (ignored)");
            }
        }
    }
}

// EOF
