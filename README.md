# axum-serve-bench

Benchmark for [tokio-rs/axum#3936](https://github.com/tokio-rs/axum/pull/3936). It compares `axum::serve(listener, router)` on axum `main` with the same code on the branch that finalizes the router once per `serve` instead of once per connection.

The same server is built twice:

| crate | axum |
|---|---|
| `server-main/` | `tokio-rs/axum` @ `61849628` (current `main` at the time) |
| `server-fork/` | `joshuapassos/axum` @ `a641cb0e` (the PR branch) |

Both builds compile the same source, `server-src/main.rs`. Its router is meant to look like a real app:

- 302 routes: 8 nested sections of 30 routes each, plus 60 top-level routes, `/ping` and `/sse`;
- a `from_fn` layer on each nested section;
- two router-wide `from_fn` layers and a `ConcurrencyLimitLayer`;
- shared state through `with_state`.

The server runs in one of two modes:

- `router`: `axum::serve(listener, router)`;
- `make-service`: `axum::serve(listener, router.into_make_service())`.

The load client in `client/` uses tokio and hyper over HTTP/1.1.

## Running

Linux only, because RSS is read from `/proc/<pid>/status`. You need Rust (stable) and Python 3.

```sh
./build.sh                # release builds of both servers and the client
./bench.sh all 3          # every scenario × {main, fork} × {router, make-service}, 3 reps
python3 summarize.py      # medians, main vs fork
```

`./bench.sh <scenario> <reps>` runs a single scenario. Raw rows go to `out/results.tsv`.

| scenario | what it measures |
|---|---|
| `held` | RSS growth per connection while 1000 keep-alive connections are held open |
| `sse` | RSS growth per stream with 500 SSE streams open for 30 s |
| `churn` | 50k short connections, 128 in flight: conn/s, latency, RSS before/after |
| `keepalive` | 64 persistent connections × 5000 requests: req/s and latency |
| `connect` | new connection per request, 16 in flight |
| `burst` | 1000 connections opened at once, one request each |
| `first` | first request after boot, with and without `with_state`, plus a second request and a new connection |

## Results

Linux aarch64 (16 KiB pages), glibc, release build. Medians of 3 runs, `router` mode:

| | `main` | fork |
|---|---:|---:|
| RSS per held connection | 308 kB | 27 kB |
| RSS per SSE stream | 309 kB | 28 kB |
| `churn` | 588 conn/s, p99 322 ms | 52 604 conn/s, p99 4 ms |
| `connect` | 575 conn/s | 45 597 conn/s |
| `burst` (all 1000 answered) | 4288 ms | 46 ms |
| `keepalive` | 274k req/s | 283k req/s |

In `make-service` mode, `main` and the fork give the same numbers within noise. That path is untouched by the change, and 27 kB is hyper's and tokio's own per-connection baseline.

Notes:

- Absolute numbers depend on page size, allocator and core count. Expect the per-connection numbers to be lower on x86_64 with 4 KiB pages. The ratio between the two builds is what matters.
- `burst` in `make-service` mode takes about 1 s on both builds. That is the listen backlog overflowing and TCP retransmitting the SYN after 1 s, not axum.
- `first` is sub-millisecond and noisy. Use 15+ reps (`./bench.sh first 15`) before reading anything into it.

The raw rows behind these tables are in `results/`. `results-first15.tsv` is the `first` scenario rerun with 15 reps.
