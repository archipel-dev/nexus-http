# Performance budgets

Speed is a feature of Nexus, so the budgets are CI gates: a pull request that breaks one is red.
The limits live in [`perf-budgets.toml`](../perf-budgets.toml); change them only in a PR that
explains why.

| Budget | Limit | What it means |
|---|---:|---|
| Cold start | 400 ms | Median of 5 runs, from process spawn to the first presented frame of the demo workspace. |
| Frame time | p99 < 16 ms | CPU time of one `update` plus tessellation over the scripted interactions. |
| Idle CPU | 1 % | Process CPU time over 10 s without input, as a share of one core. |
| Idle repaints | 2 frames | Frames the window paints during those 10 s; guards against continuous repainting. |
| Peak memory | 200 MB | `VmHWM` after the interaction script, with software rendering (lavapipe; real-GPU target in #45). |

## Run it locally

```bash
sudo apt-get install xvfb mesa-vulkan-drivers libxkbcommon-x11-0   # Linux only
cargo xtask perf                  # release build, 5 runs, prints the table
cargo xtask perf --no-build       # reuse target/release/nexus
cargo xtask perf --no-xvfb        # use the current $DISPLAY (a real GPU) instead
cargo xtask perf --runs 3 --out /tmp/perf
```

The command exits non-zero when a budget is exceeded. It writes `perf.json` and `perf.md` to
`target/perf/` (plus one `run-N.json` per run) and appends the table to `$GITHUB_STEP_SUMMARY`
when set. Under Xvfb it starts a private server, so it does not touch your session.

## How it is measured

`cargo xtask perf` runs `nexus --demo` with `NEXUS_PERF=<out.json>` (see
`crates/nexus/src/perf.rs`). The first run measures everything; the other runs only measure
cold start (`NEXUS_PERF_QUICK=1`).

- **Cold start**: xtask passes the spawn time in `NEXUS_PERF_T0_US`; the app records the time at
  the start of its second `update`. By then the first frame with the demo workspace has been laid
  out, painted and presented and the window accepts input. It includes the window and GPU setup
  and the three demo servers.
- **Frame time**: a measurement harness, not a smoke test. It feeds scripted input into a headless
  `egui::Context` that renders the same app state: select every request, switch every request and
  response tab, a click, type into the sidebar filter, open the quick switcher (Ctrl+P), type and
  close it, open and close the environments dialog, scroll the sidebar. Each input is followed by
  two settling frames; all frames are timed (690 per run). Selection, tab and filter changes set
  the state a click or keystroke would set instead of hitting pixel positions, so the harness
  does not break when the layout moves. GPU work is not included, so the number is the same on
  lavapipe and on a real GPU; it is the budget for what Nexus itself spends per frame.
- **Idle CPU**: after the script a sampler thread waits for the last frame to be painted, then reads
  `utime + stime` of `/proc/self/stat` (all threads, including the demo servers) before and after
  10 s without input. The frames painted in the window are counted too: anything above the limit
  means something requests repaints continuously.
- **Memory**: peak resident set (`VmHWM`) at the end of the run, so it includes the script and the
  idle window. It is dominated by the software renderer; a real GPU keeps its textures and
  buffers out of the process, so expect a lower number there (measure with `--no-xvfb`).

Numbers are noisy on shared CI runners; cold start in particular varies by tens of milliseconds
between runs, which is why the median is used.
