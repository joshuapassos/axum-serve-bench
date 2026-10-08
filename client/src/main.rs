//! Load client for the axum serve bench (tokio + hyper, HTTP/1.1).
//!
//! bench-client <addr> <subcommand> ...
//!   sse <n> <secs>                    open n /sse streams, hold for secs, close
//!   churn <total> <concurrency>       connect, one request, close (no keep-alive)
//!   keepalive <clients> <reqs>        clients × reqs on persistent connections
//!   burst <n>                         n connections opened at once, one request each
//!   first                             one connection, one request; prints its latency
//!
//! Every subcommand prints `key=value` lines for the harness.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::{
    body::Incoming,
    client::conn::http1::{self, SendRequest},
    header::HOST,
    Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpStream, sync::Barrier, task::JoinHandle};

type Sender = SendRequest<Empty<Bytes>>;

async fn connect(addr: &str) -> std::io::Result<(Sender, JoinHandle<()>)> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let (sender, conn) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(std::io::Error::other)?;
    let handle = tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok((sender, handle))
}

fn req(path: &str, close: bool) -> Request<Empty<Bytes>> {
    let mut b = Request::builder().uri(path).header(HOST, "bench");
    if close {
        b = b.header("connection", "close");
    }
    b.body(Empty::new()).unwrap()
}

async fn one(sender: &mut Sender, path: &str, close: bool) -> Result<(), String> {
    let res: Response<Incoming> = sender
        .send_request(req(path, close))
        .await
        .map_err(|e| e.to_string())?;
    if res.status() != StatusCode::OK {
        return Err(format!("status {}", res.status()));
    }
    res.into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn percentiles(lat: &mut [u64]) -> (u64, u64, u64, u64) {
    lat.sort_unstable();
    let p = |q: f64| lat[((lat.len() as f64 - 1.0) * q) as usize];
    (p(0.5), p(0.9), p(0.99), *lat.last().unwrap_or(&0))
}

fn print_lat(lat: &mut Vec<u64>) {
    let (p50, p90, p99, max) = percentiles(lat);
    println!("p50_us={p50}\np90_us={p90}\np99_us={p99}\nmax_us={max}");
}

async fn sse(addr: String, n: usize, secs: u64) {
    let events = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::with_capacity(n);
    let start = Instant::now();
    for _ in 0..n {
        let addr = addr.clone();
        let events = events.clone();
        let failed = failed.clone();
        handles.push(tokio::spawn(async move {
            let Ok((mut sender, conn)) = connect(&addr).await else {
                failed.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let Ok(res) = sender.send_request(req("/sse", false)).await else {
                failed.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let mut body = res.into_body();
            let deadline = tokio::time::sleep(Duration::from_secs(secs));
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    frame = body.frame() => match frame {
                        Some(Ok(f)) if f.is_data() => { events.fetch_add(1, Ordering::Relaxed); }
                        Some(Ok(_)) => {}
                        _ => { failed.fetch_add(1, Ordering::Relaxed); break; }
                    },
                }
            }
            drop(body);
            drop(sender);
            conn.abort();
        }));
    }
    // Tell the harness all streams are up (after ~2 s every one has connected).
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("open={}", n - failed.load(Ordering::Relaxed) as usize);
    for h in handles {
        let _ = h.await;
    }
    println!(
        "events={}\nfailed={}\nelapsed_s={:.1}",
        events.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        start.elapsed().as_secs_f64()
    );
}

/// `total` connections, each: connect, one request with `Connection: close`,
/// wait for the server to close. `concurrency` in flight at once.
async fn churn(addr: String, total: usize, concurrency: usize) {
    let per = total / concurrency;
    let failed = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let addr = addr.clone();
        let failed = failed.clone();
        handles.push(tokio::spawn(async move {
            let mut lat = Vec::with_capacity(per);
            for i in 0..per {
                let t = Instant::now();
                let path = format!("/svc{}/item{}/{}", i % 8, i % 15, i);
                let r = async {
                    let (mut sender, conn) = connect(&addr).await.map_err(|e| e.to_string())?;
                    one(&mut sender, &path, true).await?;
                    drop(sender);
                    let _ = conn.await; // server closes; wait for it
                    Ok::<(), String>(())
                }
                .await;
                match r {
                    Ok(()) => lat.push(t.elapsed().as_micros() as u64),
                    Err(e) => {
                        if failed.fetch_add(1, Ordering::Relaxed) < 3 {
                            eprintln!("churn error: {e}");
                        }
                    }
                }
            }
            lat
        }));
    }
    let mut lat = Vec::with_capacity(total);
    for h in handles {
        lat.extend(h.await.unwrap());
    }
    let el = start.elapsed().as_secs_f64();
    println!(
        "done={}\nfailed={}\nelapsed_s={el:.2}\nconn_per_s={:.0}",
        lat.len(),
        failed.load(Ordering::Relaxed),
        lat.len() as f64 / el
    );
    print_lat(&mut lat);
}

async fn keepalive(addr: String, clients: usize, reqs: usize) {
    let failed = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(clients + 1));
    let mut handles = Vec::with_capacity(clients);
    for c in 0..clients {
        let addr = addr.clone();
        let failed = failed.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let (mut sender, _conn) = connect(&addr).await.expect("connect");
            barrier.wait().await;
            let mut lat = Vec::with_capacity(reqs);
            for i in 0..reqs {
                let path = match i % 3 {
                    0 => format!("/svc{}/item{}/{}", c % 8, i % 15, i),
                    1 => format!("/r{}/{}", i % 60, i),
                    _ => "/ping".to_string(),
                };
                let t = Instant::now();
                match one(&mut sender, &path, false).await {
                    Ok(()) => lat.push(t.elapsed().as_micros() as u64),
                    Err(e) => {
                        if failed.fetch_add(1, Ordering::Relaxed) < 3 {
                            eprintln!("keepalive error: {e}");
                        }
                    }
                }
            }
            lat
        }));
    }
    barrier.wait().await;
    let start = Instant::now();
    let mut lat = Vec::with_capacity(clients * reqs);
    for h in handles {
        lat.extend(h.await.unwrap());
    }
    let el = start.elapsed().as_secs_f64();
    println!(
        "done={}\nfailed={}\nelapsed_s={el:.2}\nreq_per_s={:.0}",
        lat.len(),
        failed.load(Ordering::Relaxed),
        lat.len() as f64 / el
    );
    print_lat(&mut lat);
}

async fn burst(addr: String, n: usize) {
    let barrier = Arc::new(Barrier::new(n + 1));
    let failed = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let addr = addr.clone();
        let barrier = barrier.clone();
        let failed = failed.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            let t = Instant::now();
            let r = async {
                let (mut sender, _conn) = connect(&addr).await.map_err(|e| e.to_string())?;
                one(&mut sender, &format!("/r{}/{i}", i % 60), false).await
            }
            .await;
            match r {
                Ok(()) => Some(t.elapsed().as_micros() as u64),
                Err(e) => {
                    if failed.fetch_add(1, Ordering::Relaxed) < 3 {
                        eprintln!("burst error: {e}");
                    }
                    None
                }
            }
        }));
    }
    barrier.wait().await;
    let start = Instant::now();
    let mut lat = Vec::with_capacity(n);
    for h in handles {
        if let Some(l) = h.await.unwrap() {
            lat.push(l);
        }
    }
    println!(
        "done={}\nfailed={}\nall_answered_ms={:.1}",
        lat.len(),
        failed.load(Ordering::Relaxed),
        start.elapsed().as_secs_f64() * 1000.0
    );
    print_lat(&mut lat);
}

async fn first(addr: String) {
    let t = Instant::now();
    let (mut sender, _conn) = connect(&addr).await.expect("connect");
    let connected = t.elapsed().as_micros();
    one(&mut sender, "/r0/1", false).await.expect("first request");
    let total = t.elapsed().as_micros();
    // Second request on the same connection and a request on a new connection,
    // as the steady-state reference.
    let t2 = Instant::now();
    let second = match one(&mut sender, "/r0/1", false).await {
        Ok(()) => t2.elapsed().as_micros(),
        Err(e) => { eprintln!("second request: {e}"); 0 }
    };
    let t3 = Instant::now();
    let (mut s2, _c2) = connect(&addr).await.expect("connect 2");
    let newconn = match one(&mut s2, "/r0/1", false).await {
        Ok(()) => t3.elapsed().as_micros(),
        Err(e) => { eprintln!("new conn request: {e}"); 0 }
    };
    println!("connect_us={connected}\nfirst_us={total}\nsecond_us={second}\nnewconn_us={newconn}");
}

#[tokio::main(worker_threads = 4)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = args[0].clone();
    let num = |i: usize| -> usize { args[i].parse().unwrap() };
    match args[1].as_str() {
        "sse" => sse(addr, num(2), num(3) as u64).await,
        "churn" => churn(addr, num(2), num(3)).await,
        "keepalive" => keepalive(addr, num(2), num(3)).await,
        "burst" => burst(addr, num(2)).await,
        "first" => first(addr).await,
        other => panic!("unknown subcommand {other}"),
    }
}
