//! Bench server: a "realistic" router (~300 routes, nested routers, from_fn
//! middleware, ConcurrencyLimitLayer, shared state) served either as
//! `serve(listener, router)` or `serve(listener, router.into_make_service())`.
//!
//! usage: bench-server <router|make-service> <port> [stateless]
//!
//! `stateless` builds the same route table as `Router<()>` without ever calling
//! `with_state`, so the patched axum has to run `with_state(())` on the first
//! connection (scenario 7: first-connection cost).

use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Router,
};
use futures_util::{Stream, StreamExt};
use tokio_stream::wrappers::IntervalStream;
use tower::limit::ConcurrencyLimitLayer;

#[derive(Clone)]
struct AppState {
    name: Arc<str>,
    counter: Arc<AtomicU64>,
}

async fn ok() -> &'static str {
    "ok"
}

async fn with_id(Path(id): Path<u64>) -> String {
    id.to_string()
}

async fn stateful(State(s): State<AppState>) -> String {
    let n = s.counter.fetch_add(1, Ordering::Relaxed);
    format!("{} {n}", s.name)
}

async fn sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = IntervalStream::new(tokio::time::interval(Duration::from_secs(1)))
        .enumerate()
        .map(|(i, _)| Ok(Event::default().data(i.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn mw_request_id(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut()
        .insert("x-request-id", HeaderValue::from_static("bench"));
    res
}

async fn mw_auth(req: Request, next: Next) -> Response {
    if req.headers().get("x-deny").is_some() {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

async fn mw_timing(req: Request, next: Next) -> Response {
    let start = std::time::Instant::now();
    let mut res = next.run(req).await;
    let v = HeaderValue::from_str(&start.elapsed().as_micros().to_string()).unwrap();
    res.headers_mut().insert("x-elapsed-us", v);
    res
}

/// One nested section: 30 routes behind its own middleware.
fn section<S: Clone + Send + Sync + 'static>() -> Router<S> {
    let mut r = Router::new();
    for i in 0..15 {
        r = r
            .route(&format!("/item{i}/{{id}}"), get(with_id).post(with_id))
            .route(&format!("/list{i}"), get(ok));
    }
    r.layer(middleware::from_fn(mw_auth))
}

/// 8 nested sections (240 routes) + 60 top-level routes + /ping + /sse = 302
/// routes, two router-wide from_fn layers and a ConcurrencyLimitLayer.
fn build<S: Clone + Send + Sync + 'static>() -> Router<S> {
    let mut app = Router::new();
    for k in 0..8 {
        app = app.nest(&format!("/svc{k}"), section());
    }
    for i in 0..60 {
        app = app.route(&format!("/r{i}/{{id}}"), get(with_id).post(with_id));
    }
    app.route("/ping", get(ok))
        .route("/sse", get(sse))
        .layer(middleware::from_fn(mw_timing))
        .layer(ConcurrencyLimitLayer::new(100_000))
        .layer(middleware::from_fn(mw_request_id))
}

#[tokio::main(worker_threads = 4)]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let port: u16 = args.next().map_or(3177, |p| p.parse().unwrap());
    let stateless = args.next().as_deref() == Some("stateless");

    let app: Router<()> = if stateless {
        build::<()>()
    } else {
        let state = AppState {
            name: Arc::from("bench"),
            counter: Arc::new(AtomicU64::new(0)),
        };
        build::<AppState>()
            .route("/state", get(stateful))
            .with_state(state)
    };

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    println!("ready");
    match mode.as_str() {
        "router" => match axum::serve(listener, app).await {},
        "make-service" => match axum::serve(listener, app.into_make_service()).await {},
        _ => panic!("usage: bench-server <router|make-service> <port> [stateless]"),
    }
}
