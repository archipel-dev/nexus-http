//! `cargo xtask perf`: measures the speed budgets of `nexus` and enforces them.
//!
//! Builds `nexus` in release mode, runs it with `--demo` under a private Xvfb with
//! `NEXUS_PERF` set (see `crates/nexus/src/perf.rs` for what is measured), compares
//! the numbers with `perf-budgets.toml`, writes `target/perf/perf.{json,md}`, prints
//! the table, appends it to `$GITHUB_STEP_SUMMARY` and fails when a budget is broken.

use anyhow::{Context, Result, bail};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BUDGETS: &str = "perf-budgets.toml";
const RUN_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, PartialEq)]
pub struct Options {
    /// Cold-start runs; the first one also measures everything else.
    pub runs: usize,
    pub build: bool,
    /// Start a private Xvfb; otherwise use the current `DISPLAY`.
    pub xvfb: bool,
    pub out: PathBuf,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options> {
        let mut options = Options {
            runs: 5,
            build: true,
            xvfb: true,
            out: PathBuf::from("target/perf"),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--runs" => {
                    options.runs = args
                        .next()
                        .and_then(|n| n.parse().ok())
                        .filter(|n| *n > 0)
                        .context("--runs needs a positive number")?
                }
                "--out" => options.out = args.next().context("--out needs a directory")?.into(),
                "--no-build" => options.build = false,
                "--no-xvfb" => options.xvfb = false,
                other => bail!("unknown perf option `{other}`"),
            }
        }
        Ok(options)
    }
}

/// Limits from `perf-budgets.toml`.
#[derive(Debug, PartialEq)]
pub struct Budgets {
    pub cold_start_ms: f64,
    pub frame_p99_ms: f64,
    pub idle_cpu_percent: f64,
    pub idle_frames: f64,
    pub peak_rss_mb: f64,
}

impl Budgets {
    /// Reads the flat `key = number` pairs of the `[budgets]` table.
    pub fn parse(text: &str) -> Result<Budgets> {
        let mut values = std::collections::HashMap::new();
        let mut in_table = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                in_table = line == "[budgets]";
            } else if in_table && let Some((key, value)) = line.split_once('=') {
                let value: f64 = value
                    .trim()
                    .parse()
                    .with_context(|| format!("{BUDGETS}: `{line}` is not a number"))?;
                values.insert(key.trim().to_owned(), value);
            }
        }
        let get = |key: &str| {
            values
                .get(key)
                .copied()
                .with_context(|| format!("{BUDGETS}: missing `{key}` under [budgets]"))
        };
        Ok(Budgets {
            cold_start_ms: get("cold_start_ms")?,
            frame_p99_ms: get("frame_p99_ms")?,
            idle_cpu_percent: get("idle_cpu_percent")?,
            idle_frames: get("idle_frames")?,
            peak_rss_mb: get("peak_rss_mb")?,
        })
    }
}

/// What one measurement run reports.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub cold_start_ms: f64,
    pub frame_p50_ms: f64,
    pub frame_p99_ms: f64,
    pub frame_max_ms: f64,
    pub frame_samples: f64,
    pub idle_cpu_percent: f64,
    pub idle_frames: f64,
    pub peak_rss_mb: f64,
}

pub fn perf(options: Options) -> Result<()> {
    let budgets =
        Budgets::parse(&fs::read_to_string(BUDGETS).with_context(|| BUDGETS.to_owned())?)?;
    if options.build {
        let status = Command::new("cargo")
            .args(["build", "--release", "-p", "nexus"])
            .status()
            .context("run cargo build")?;
        if !status.success() {
            bail!("release build of nexus failed");
        }
    }
    let binary = Path::new("target/release/nexus")
        .canonicalize()
        .context("target/release/nexus")?;
    fs::create_dir_all(&options.out)?;

    let xvfb = if options.xvfb {
        Some(Xvfb::start()?)
    } else {
        None
    };
    let display = match &xvfb {
        Some(x) => x.display.clone(),
        None => std::env::var("DISPLAY").context("DISPLAY is not set (drop --no-xvfb)")?,
    };

    let mut cold = Vec::new();
    let mut full = None;
    for run in 0..options.runs {
        let out = options.out.join(format!("run-{run}.json"));
        let _ = fs::remove_file(&out);
        let quick = run > 0;
        eprintln!(
            "perf: run {}/{}{}",
            run + 1,
            options.runs,
            if quick { " (start only)" } else { "" }
        );
        let json = run_nexus(&binary, &display, &out, quick)?;
        cold.push(num(&json, "cold_start_ms")?);
        if run == 0 {
            full = Some(json);
        }
    }
    let full = full.context("no run")?;
    let report = Report {
        cold_start_ms: median(&mut cold),
        frame_p50_ms: num(&full, "frame_p50_ms")?,
        frame_p99_ms: num(&full, "frame_p99_ms")?,
        frame_max_ms: num(&full, "frame_max_ms")?,
        frame_samples: num(&full, "frame_samples")?,
        idle_cpu_percent: num(&full, "idle_cpu_percent")?,
        idle_frames: num(&full, "idle_frames")?,
        peak_rss_mb: num(&full, "peak_rss_mb")?,
    };
    drop(xvfb);

    let (table, failed) = summary(&report, &budgets, &cold);
    fs::write(
        options.out.join("perf.json"),
        serde_json::to_string_pretty(&to_json(&report, &budgets, &cold))?,
    )?;
    fs::write(options.out.join("perf.md"), &table)?;
    println!("{table}");
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?;
        writeln!(file, "{table}")?;
    }
    if failed > 0 {
        bail!("{failed} performance budget(s) exceeded");
    }
    Ok(())
}

fn num(json: &serde_json::Value, key: &str) -> Result<f64> {
    json.get(key)
        .and_then(|v| v.as_f64())
        .with_context(|| format!("nexus report has no `{key}`"))
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn run_nexus(binary: &Path, display: &str, out: &Path, quick: bool) -> Result<serde_json::Value> {
    let t0 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
    let mut command = Command::new(binary);
    command
        .arg("--demo")
        .env("DISPLAY", display)
        .env_remove("WAYLAND_DISPLAY")
        .env("NEXUS_PERF", out)
        .env("NEXUS_PERF_T0_US", t0.to_string())
        .stdin(Stdio::null());
    if quick {
        command.env("NEXUS_PERF_QUICK", "1");
    }
    let mut child = command.spawn().context("start nexus")?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("nexus exited with {status}");
            }
            break;
        }
        if started.elapsed() > RUN_TIMEOUT {
            let _ = child.kill();
            bail!("nexus did not finish within {RUN_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let text =
        fs::read_to_string(out).with_context(|| format!("nexus wrote no {}", out.display()))?;
    Ok(serde_json::from_str(&text)?)
}

/// A private X server so the run does not depend on the caller's display.
struct Xvfb {
    child: Child,
    display: String,
}

impl Xvfb {
    fn start() -> Result<Xvfb> {
        let mut child = Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                "1280x800x24",
                "-nolisten",
                "tcp",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("start Xvfb (install xvfb, or use --no-xvfb)")?;
        let mut line = String::new();
        BufReader::new(child.stdout.take().context("Xvfb stdout")?).read_line(&mut line)?;
        let number = line.trim();
        if number.is_empty() {
            bail!("Xvfb did not report a display");
        }
        Ok(Xvfb {
            child,
            display: format!(":{number}"),
        })
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Row {
    name: &'static str,
    value: f64,
    limit: f64,
    unit: &'static str,
    note: String,
}

fn rows(report: &Report, budgets: &Budgets, cold: &[f64]) -> Vec<Row> {
    let list: Vec<String> = cold.iter().map(|c| format!("{c:.0}")).collect();
    vec![
        Row {
            name: "Cold start (median)",
            value: report.cold_start_ms,
            limit: budgets.cold_start_ms,
            unit: "ms",
            note: format!("{} runs: {} ms", cold.len(), list.join(", ")),
        },
        Row {
            name: "Frame time p99",
            value: report.frame_p99_ms,
            limit: budgets.frame_p99_ms,
            unit: "ms",
            note: format!(
                "p50 {:.2} ms, max {:.2} ms, {} frames",
                report.frame_p50_ms, report.frame_max_ms, report.frame_samples
            ),
        },
        Row {
            name: "Idle CPU",
            value: report.idle_cpu_percent,
            limit: budgets.idle_cpu_percent,
            unit: "%",
            note: "10 s idle, share of one core".into(),
        },
        Row {
            name: "Idle repaints",
            value: report.idle_frames,
            limit: budgets.idle_frames,
            unit: "frames",
            note: "frames painted by the idle window".into(),
        },
        Row {
            name: "Peak memory",
            value: report.peak_rss_mb,
            limit: budgets.peak_rss_mb,
            unit: "MB",
            note: "VmHWM, software rendering".into(),
        },
    ]
}

/// The markdown table and the number of exceeded budgets.
fn summary(report: &Report, budgets: &Budgets, cold: &[f64]) -> (String, usize) {
    let mut table = String::from(
        "### Nexus performance budgets\n\n| Metric | Result | Budget | Status | Details |\n|---|---:|---:|:---:|---|\n",
    );
    let mut failed = 0;
    for row in rows(report, budgets, cold) {
        let ok = row.value <= row.limit;
        failed += usize::from(!ok);
        let precision = if row.unit == "frames" { 0 } else { 2 };
        table.push_str(&format!(
            "| {} | {:.p$} {} | ≤ {} {} | {} | {} |\n",
            row.name,
            row.value,
            row.unit,
            row.limit,
            row.unit,
            if ok { "✅ pass" } else { "❌ over" },
            row.note,
            p = precision,
        ));
    }
    (table, failed)
}

fn to_json(report: &Report, budgets: &Budgets, cold: &[f64]) -> serde_json::Value {
    serde_json::json!({
        "cold_start_ms": report.cold_start_ms,
        "cold_start_runs_ms": cold,
        "frame_p50_ms": report.frame_p50_ms,
        "frame_p99_ms": report.frame_p99_ms,
        "frame_max_ms": report.frame_max_ms,
        "frame_samples": report.frame_samples,
        "idle_cpu_percent": report.idle_cpu_percent,
        "idle_frames": report.idle_frames,
        "peak_rss_mb": report.peak_rss_mb,
        "budgets": {
            "cold_start_ms": budgets.cold_start_ms,
            "frame_p99_ms": budgets.frame_p99_ms,
            "idle_cpu_percent": budgets.idle_cpu_percent,
            "idle_frames": budgets.idle_frames,
            "peak_rss_mb": budgets.peak_rss_mb,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# c\n[budgets]\ncold_start_ms = 400.0 # x\nframe_p99_ms = 16\nidle_cpu_percent = 1.0\nidle_frames = 2\npeak_rss_mb = 150\n";

    #[test]
    fn parses_budgets() {
        let b = Budgets::parse(FILE).unwrap();
        assert_eq!(b.cold_start_ms, 400.0);
        assert_eq!(b.idle_frames, 2.0);
        assert!(Budgets::parse("[budgets]\ncold_start_ms = 1\n").is_err());
    }

    #[test]
    fn repo_budgets_file_parses() {
        let text =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../perf-budgets.toml"))
                .unwrap();
        Budgets::parse(&text).unwrap();
    }

    #[test]
    fn median_and_failures() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
        let b = Budgets::parse(FILE).unwrap();
        let r = Report {
            cold_start_ms: 500.0,
            idle_cpu_percent: 0.5,
            ..Default::default()
        };
        let (table, failed) = summary(&r, &b, &[500.0]);
        assert_eq!(failed, 1);
        assert!(table.contains("❌ over"));
    }

    #[test]
    fn parses_options() {
        let o =
            Options::parse(["--runs".to_owned(), "3".to_owned(), "--no-build".to_owned()]).unwrap();
        assert_eq!((o.runs, o.build, o.xvfb), (3, false, true));
        assert!(Options::parse(["--bogus".to_owned()]).is_err());
    }
}
