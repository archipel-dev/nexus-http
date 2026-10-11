//! Performance measurement mode (`NEXUS_PERF=<out.json>`), driven by `cargo xtask perf`.
//!
//! The harness measures four numbers and writes them as JSON:
//!
//! * `cold_start_ms`: from the process spawn (`NEXUS_PERF_T0_US`, unix microseconds,
//!   set by the parent) to the start of the second `update`. The first frame, with
//!   the demo workspace laid out, has been painted and presented by then and the
//!   window already accepts input.
//! * `frame_*`: CPU time of one `update` plus tessellation, over a scripted set of
//!   interactions fed into a headless `egui::Context` that renders the same app.
//!   GPU submission is not part of it, so the number does not depend on the
//!   renderer (lavapipe or a real GPU).
//! * `idle_cpu_percent`: process CPU time (`/proc/self/stat`, all threads) over an
//!   idle window with no input, as a share of one core, plus the number of frames
//!   the real window painted meanwhile.
//! * `peak_rss_mb`: `VmHWM` after the script and the idle window.
//!
//! `NEXUS_PERF_QUICK=1` exits right after the cold start measurement;
//! `NEXUS_PERF_IDLE_SECS` overrides the idle window (default 10).

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
use nexus_http::NexusApp;
use nexus_http::state::{RequestTab, ResponseTab};
use nexus_http::views::overlays::Overlay;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SETTLE: Duration = Duration::from_millis(1000);
const ROUNDS: usize = 5;
/// Frames run after every input: the response to it, then the settling ones.
const FRAMES_PER_STEP: usize = 3;

pub struct Perf {
    out: PathBuf,
    quick: bool,
    idle: Duration,
    frames: Arc<AtomicU64>,
    cold_start_ms: Option<f64>,
    script: Option<Script>,
    phase: Phase,
}

enum Phase {
    Start,
    /// `second`: the frame that ended the settling has been painted; start measuring.
    Settle {
        until: Instant,
        second: bool,
    },
    Idle(Arc<Mutex<Option<IdleResult>>>),
    Done,
}

/// Measured by a sampler thread, so the measurement does not need a frame.
struct IdleResult {
    cpu_ms: f64,
    wall_ms: f64,
    frames: u64,
}

struct Script {
    p50_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    samples: usize,
}

impl Perf {
    pub fn from_env() -> Option<Perf> {
        let out = PathBuf::from(std::env::var_os("NEXUS_PERF")?);
        let secs = std::env::var("NEXUS_PERF_IDLE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10.0);
        Some(Perf {
            out,
            quick: std::env::var_os("NEXUS_PERF_QUICK").is_some(),
            idle: Duration::from_secs_f64(secs),
            frames: Arc::new(AtomicU64::new(0)),
            cold_start_ms: None,
            script: None,
            phase: Phase::Start,
        })
    }

    /// Called at the start of every `update` of the real window.
    pub fn update(&mut self, ctx: &egui::Context, app: &mut NexusApp) {
        let frame = self.frames.fetch_add(1, Ordering::Relaxed) + 1;
        match self.phase {
            Phase::Start => match frame {
                1 => ctx.request_repaint(),
                _ => {
                    self.cold_start_ms = cold_start_ms();
                    if self.quick {
                        self.finish(ctx);
                        return;
                    }
                    self.script = Some(run_script(app));
                    // Wake up again after the settling time, then stay quiet.
                    let until = Instant::now() + SETTLE;
                    ctx.request_repaint_after(SETTLE);
                    self.phase = Phase::Settle {
                        until,
                        second: false,
                    };
                }
            },
            Phase::Settle { until, second } => {
                let now = Instant::now();
                if now < until {
                    ctx.request_repaint_after(until - now);
                } else if !second {
                    // The frame painting the final state of the script is this one;
                    // its paint cost lands after this call, so measure from the next.
                    let until = now + Duration::from_millis(200);
                    ctx.request_repaint_after(Duration::from_millis(200));
                    self.phase = Phase::Settle {
                        until,
                        second: true,
                    };
                } else {
                    let result = Arc::new(Mutex::new(None));
                    self.phase = Phase::Idle(result.clone());
                    let (ctx, frames, idle) = (ctx.clone(), self.frames.clone(), self.idle);
                    std::thread::spawn(move || {
                        // The frame that started this is still being painted.
                        std::thread::sleep(Duration::from_millis(500));
                        let at = Instant::now();
                        let (cpu0, frames0) = (process_cpu_ms(), frames.load(Ordering::Relaxed));
                        std::thread::sleep(idle);
                        let cpu1 = process_cpu_ms();
                        *result.lock().expect("idle result") = Some(IdleResult {
                            cpu_ms: cpu1.unwrap_or(0.0) - cpu0.unwrap_or(0.0),
                            wall_ms: at.elapsed().as_secs_f64() * 1000.0,
                            frames: frames.load(Ordering::Relaxed) - frames0,
                        });
                        // Wake the window so it writes the report and closes.
                        ctx.request_repaint();
                    });
                }
            }
            Phase::Idle(ref result) => {
                if result.lock().expect("idle result").is_some() {
                    self.finish(ctx);
                }
            }
            Phase::Done => {}
        }
    }

    fn finish(&mut self, ctx: &egui::Context) {
        let idle = match &self.phase {
            Phase::Idle(result) => result.lock().expect("idle result").take().map(|r| {
                json!({
                    "idle_cpu_percent": r.cpu_ms / r.wall_ms * 100.0,
                    "idle_cpu_ms": r.cpu_ms,
                    "idle_wall_ms": r.wall_ms,
                    "idle_frames": r.frames,
                })
            }),
            _ => None,
        };
        let mut report = json!({
            "cold_start_ms": self.cold_start_ms,
            "peak_rss_mb": peak_rss_mb(),
        });
        let map = report.as_object_mut().expect("object");
        if let Some(s) = &self.script {
            map.insert("frame_samples".into(), s.samples.into());
            map.insert("frame_p50_ms".into(), s.p50_ms.into());
            map.insert("frame_p99_ms".into(), s.p99_ms.into());
            map.insert("frame_max_ms".into(), s.max_ms.into());
        }
        if let Some(serde_json::Value::Object(idle)) = idle {
            map.extend(idle);
        }
        let text = serde_json::to_string_pretty(&report).unwrap_or_default();
        if let Err(e) = std::fs::write(&self.out, text) {
            eprintln!("nexus perf: cannot write {}: {e}", self.out.display());
        }
        self.phase = Phase::Done;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn cold_start_ms() -> Option<f64> {
    let t0: u128 = std::env::var("NEXUS_PERF_T0_US").ok()?.parse().ok()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_micros();
    Some(now.saturating_sub(t0) as f64 / 1000.0)
}

/// CPU time (user + system, all threads) of this process.
fn process_cpu_ms() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the parenthesised command name; utime and stime are 14 and 15.
    let rest = &stat[stat.rfind(')')? + 2..];
    let mut fields = rest.split(' ');
    let utime: f64 = fields.nth(11)?.parse().ok()?;
    let stime: f64 = fields.next()?.parse().ok()?;
    // USER_HZ is 100 on every Linux target we ship.
    Some((utime + stime) * 10.0)
}

fn peak_rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb / 1024.0)
}

/// Feeds scripted input into a headless context and times every frame.
struct Driver {
    ctx: egui::Context,
    start: Instant,
    times: Vec<Duration>,
    record: bool,
}

impl Driver {
    fn frame(&mut self, app: &mut NexusApp, events: Vec<Event>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
            time: Some(self.start.elapsed().as_secs_f64()),
            events,
            focused: true,
            ..Default::default()
        };
        let t = Instant::now();
        let out = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| app.ui(ui));
        });
        let _ = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        if self.record {
            self.times.push(t.elapsed());
        }
    }

    /// One interaction: the input frame and the frames that settle it.
    fn step(&mut self, app: &mut NexusApp, events: Vec<Event>) {
        self.frame(app, events);
        for _ in 1..FRAMES_PER_STEP {
            self.frame(app, vec![]);
            // Advance time so animations make progress.
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn key(key: Key, modifiers: Modifiers) -> Vec<Event> {
    [true, false]
        .into_iter()
        .map(|pressed| Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

fn click(pos: Pos2) -> Vec<Event> {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    vec![Event::PointerMoved(pos), button(true), button(false)]
}

fn run_script(app: &mut NexusApp) -> Script {
    let ctx = egui::Context::default();
    NexusApp::install(&ctx, true);
    let mut d = Driver {
        ctx,
        start: Instant::now(),
        times: Vec::new(),
        record: false,
    };
    // Warm up: font atlas, first layout, a real response in the response pane.
    for _ in 0..5 {
        d.frame(app, vec![]);
    }
    let ids: Vec<_> = app
        .state
        .workspace
        .requests()
        .iter()
        .map(|(_, r)| r.id.clone())
        .collect();
    if let Some(first) = ids.iter().find(|id| {
        app.state
            .workspace
            .find_request(id)
            .is_some_and(|(_, r)| r.name == "List products")
    }) {
        app.state.select(first.clone());
        app.send_selected();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            d.frame(app, vec![]);
            if !app.state.is_sending() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for _ in 0..3 {
            d.frame(app, vec![]);
        }
    }

    d.record = true;
    for _ in 0..ROUNDS {
        // Select requests (state change = what a click on a tree row does).
        for id in &ids {
            app.state.select(id.clone());
            d.step(app, vec![]);
        }
        // Switch request and response tabs.
        for tab in RequestTab::ALL {
            app.state.request_tab = tab;
            d.step(app, vec![]);
        }
        for tab in ResponseTab::ALL {
            app.state.response_tab = tab;
            d.step(app, vec![]);
        }
        // A click into the request pane, as a real pointer.
        d.step(app, click(Pos2::new(700.0, 300.0)));
        // Type in the sidebar filter.
        for prefix in ["p", "pr", "pro", "prod", "pro", "pr", "p", ""] {
            app.state.filter = prefix.into();
            d.step(app, vec![]);
        }
        // Quick switcher: open, type, close, all with real key events.
        d.step(app, key(Key::P, Modifiers::COMMAND));
        for ch in "prod".chars() {
            d.step(app, vec![Event::Text(ch.to_string())]);
        }
        d.step(app, key(Key::Escape, Modifiers::NONE));
        // Environments dialog: open and close.
        app.open_overlay(Overlay::Environments);
        d.step(app, vec![]);
        d.step(app, key(Key::Escape, Modifiers::NONE));
        // Scroll the sidebar tree.
        d.step(app, vec![Event::PointerMoved(Pos2::new(120.0, 400.0))]);
        for dy in [-120.0, -120.0, 240.0] {
            d.step(
                app,
                vec![Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, dy),
                    modifiers: Modifiers::NONE,
                }],
            );
        }
    }

    let mut ms: Vec<f64> = d.times.iter().map(|t| t.as_secs_f64() * 1000.0).collect();
    ms.sort_by(|a, b| a.total_cmp(b));
    let pick = |q: f64| ms[(((ms.len() - 1) as f64) * q).ceil() as usize];
    Script {
        samples: ms.len(),
        p50_ms: pick(0.5),
        p99_ms: pick(0.99),
        max_ms: *ms.last().unwrap_or(&0.0),
    }
}
