//! Append synthetic log4j2-shaped records to a file at a fixed rate. Used
//! for the P4 demo and the Playwright tail-survives-rotation smoke test.
//!
//! Usage:
//!     cargo run --example `fake_tailer` -- PATH [--rate N] [--rotate]
//!
//! `--rate N` appends N lines per second (default 10). `--rotate` truncates
//! the file once before starting, then appends, so the running clog instance
//! has to handle a rotation event.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

fn print_usage() {
    eprintln!("usage: fake_tailer <path> [--rate N] [--rotate] [--count N]");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        std::process::exit(2);
    }

    let mut path: Option<PathBuf> = None;
    let mut rate: u32 = 10;
    let mut rotate = false;
    let mut count: Option<u64> = None;
    let mut iter = args.into_iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--rate" => {
                let Some(v) = iter.next() else {
                    print_usage();
                    std::process::exit(2);
                };
                rate = v.parse().unwrap_or(10).max(1);
            }
            "--count" => {
                let Some(v) = iter.next() else {
                    print_usage();
                    std::process::exit(2);
                };
                count = Some(v.parse().unwrap_or(0));
            }
            "--rotate" => rotate = true,
            "--help" | "-h" => {
                print_usage();
                return;
            }
            other if other.starts_with("--") => {
                eprintln!("unknown flag: {other}");
                print_usage();
                std::process::exit(2);
            }
            other => path = Some(PathBuf::from(other)),
        }
    }
    let Some(path) = path else {
        print_usage();
        std::process::exit(2);
    };

    if rotate {
        // Truncate the file (mimics OnStartupTriggeringPolicy).
        File::create(&path).expect("truncate target file");
        eprintln!("fake_tailer: rotated {}", path.display());
    } else if !path.exists() {
        File::create(&path).expect("create target file");
    }

    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open append");

    let interval = Duration::from_secs_f64(1.0 / f64::from(rate));
    let started = Instant::now();
    let mut seq: u64 = 0;
    let levels = ["INFO ", "DEBUG", "WARN ", "ERROR"];
    loop {
        if let Some(c) = count {
            if seq >= c {
                break;
            }
        }
        let level = levels[usize::try_from(seq).unwrap_or(usize::MAX) % levels.len()];
        let elapsed_ms = started.elapsed().as_millis();
        // wsl-oink pattern shape so a freshly-opened clog auto-detects it.
        let line = format!(
            "[{level}] 2026-05-23 12:00:{:02}.{:03} [tail-{}] play - synthetic line #{seq}\n",
            (seq % 60),
            (elapsed_ms % 1000),
            seq % 4,
        );
        if let Err(e) = file.write_all(line.as_bytes()) {
            eprintln!("fake_tailer: write failed: {e}");
            break;
        }
        // Occasional multi-line records so continuation handling gets
        // exercised.
        if seq.is_multiple_of(17) {
            let _ = file.write_all(b"    at com.example.Foo.bar(Foo.java:42)\n");
            let _ = file.write_all(b"    at com.example.Foo.baz(Foo.java:99)\n");
        }
        let _ = file.flush();
        seq += 1;
        thread::sleep(interval);
    }
}
