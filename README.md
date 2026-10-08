# axum-serve-bench

Benchmark for [tokio-rs/axum#3936](https://github.com/tokio-rs/axum/pull/3936). It compares `axum::serve(listener, router)` on axum `main` with the same code on the branch that finalizes the router once per `serve` instead of once per connection.

It also covers [#3915](https://github.com/tokio-rs/axum/pull/3915), which puts `Route` behind an `Arc`, both on its own and combined with #3936.

The same server is built four times:

| crate | axum |
|---|---|
| `server-main/` | `tokio-rs/axum` @ `61849628` (current `main` at the time) |
| `server-fork/` | #3936: `joshuapassos/axum` @ `a641cb0e` |
| `server-3915/` | #3915 cherry-picked onto `61849628`: `joshuapassos/axum` @ `811d91a6` |
| `server-both/` | #3915 + #3936 on `61849628`: `joshuapassos/axum` @ `bdf0ee55` |

All four compile the same source, `server-src/main.rs`. Its router is meant to look like a real app:

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
./build.sh                # release builds of the four servers and the client
./bench.sh all 3          # every scenario × 4 builds × {router, make-service}, 3 reps
python3 summarize.py      # medians per build
```

`./bench.sh <scenario> <reps>` runs a single scenario. Raw rows go to `out/results.tsv`.

Three environment variables narrow the run:

- `BUILDS="main fork"` limits the builds.
- `MODES=router` limits the modes.
- `STATELESS=stateless` builds the router as `Router<()>` without ever calling `with_state`. Those rows are recorded as `<scenario>-stateless`.

| scenario | what it measures |
|---|---|
| `held` | RSS growth per connection while 1000 keep-alive connections are held open |
| `sse` | RSS growth per stream with 500 SSE streams open for 30 s |
| `churn` | 50k short connections, 128 in flight: conn/s, latency, RSS before/after |
| `keepalive` | 64 persistent connections × 5000 requests: req/s and latency |
| `connect` | new connection per request, 16 in flight |
| `burst` | 1000 connections opened at once, one request each |
| `first` | first request after boot, with and without `with_state`, plus a second request and a new connection |

## Results: #3936 vs `main`

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

## Results: with #3915

Same machine, `router` mode, medians of 3 runs. Run with:

```sh
BUILDS="main 3915 fork both" MODES=router ./bench.sh "held keepalive churn" 3
BUILDS="main 3915 fork both" MODES=router STATELESS=stateless ./bench.sh "held keepalive" 3
```

| | `main` | #3915 | #3936 | both |
|---|---:|---:|---:|---:|
| RSS per held connection, router ends in `with_state(state)` | 308 kB | 126 kB | 27 kB | 27 kB |
| RSS per held connection, `Router<()>` without `with_state` | 329 kB | 374 kB | 27 kB | 27 kB |
| `churn` conn/s | 643 | 6 654 | 51 760 | 51 778 |
| `churn` p99 | 297 ms | 31 ms | 4.1 ms | 4.5 ms |
| `keepalive` req/s, with state | 216k | 252k | 228k | 248k |
| `keepalive` p99, with state | 809 µs | 607 µs | 665 µs | 629 µs |

The two PRs target different costs.

**#3915: shares `Route`s.** Cloning an already-finalized `Route` becomes an `Arc` bump. That shrinks the per-connection copy when the router went through `with_state`, but the `MethodRouter`s and the path router's tables are still copied: about 100 kB per connection here. A router never given state is not helped at all, because its endpoints are still boxed handlers and are converted again on every connection. Where #3915 does help is the per-request path through layered routes.

**#3936: finalizes once.** It removes the per-connection copy in both cases and does not touch the request path.

Combined, you get both benefits.

`keepalive` throughput varies by about ±20% between separate runs on this machine, so compare columns within one table, not across tables.

The raw rows are in `results/`:

- `results.tsv`: #3936 vs `main`, every scenario;
- `results-3915.tsv`: the four-build run;
- `results-first15.tsv`: the `first` scenario rerun with 15 reps.
