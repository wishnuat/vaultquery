//! Headless CLI over the same engine the app uses.
//!
//!   cargo run --release --example cli -- <folder> <query> [--regex] [--case] [--word]
//!   cargo run --release --example cli -- <folder> --terms <file-with-one-term-per-line>
//!
//! Env: VQ_KINDS=text,spreadsheet,...   VQ_DAYS=30 (modified in the last N days)

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use vaultquery_lib::engine::{self, SearchEvent, SearchOpts};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: cli <folder> <query> [--regex] [--case] [--word]\n       cli <folder> --terms <file>");
        std::process::exit(2);
    }
    let flag = |f: &str| args.iter().any(|a| a == f);
    let terms: Vec<String> = match args.iter().position(|a| a == "--terms") {
        Some(i) => std::fs::read_to_string(&args[i + 1]).expect("terms file").lines().map(String::from).collect(),
        None => Vec::new(),
    };
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let opts = SearchOpts {
        root: args[0].clone(),
        query: if terms.is_empty() { args[1].clone() } else { String::new() },
        terms: terms.clone(),
        case_sensitive: flag("--case"),
        regex: flag("--regex"),
        whole_word: flag("--word"),
        kinds: std::env::var("VQ_KINDS")
            .unwrap_or_else(|_| "text,spreadsheet,document,pdf,archive".into())
            .split(',')
            .map(String::from)
            .collect(),
        max_hits_per_file: 20,
        max_file_mb: 200,
        modified_after: std::env::var("VQ_DAYS").ok().and_then(|d| d.parse::<u64>().ok()).map(|d| now_ms - d * 86_400_000),
        ..Default::default()
    };
    let out = Mutex::new(());
    let found: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
    let json = std::env::var("VQ_JSON").is_ok();
    engine::run(opts, Arc::new(AtomicBool::new(false)), &|ev| {
        let _g = out.lock().unwrap();
        if json {
            // raw event stream, one JSON object per line (used to record the demo video)
            println!("{}", serde_json::to_string(&ev).unwrap());
            return;
        }
        match ev {
            SearchEvent::Match { file } => {
                println!("\n{}", file.path);
                for h in file.hits {
                    if let Some(t) = &h.term {
                        *found.lock().unwrap().entry(t.clone()).or_default() += 1;
                    }
                    let fields = h
                        .fields
                        .map(|f| f.iter().map(|f| format!("{}={}", f.name, f.value)).collect::<Vec<_>>().join(" | "))
                        .unwrap_or_default();
                    println!("   [{}] {}>>{}<<{}   {}", h.loc, h.pre, h.mat, h.post, fields);
                }
            }
            SearchEvent::Unreadable { path, reason } => eprintln!("  ! {reason}: {path}"),
            SearchEvent::Done { scanned, matched, hits, elapsed_ms, errors, .. } => {
                eprintln!("\n{matched} files / {hits} hits in {scanned} files scanned, {elapsed_ms} ms ({errors} unreadable)")
            }
            SearchEvent::Failed { message } => eprintln!("error: {message}"),
            _ => {}
        }
    });
    if !terms.is_empty() {
        let found = found.lock().unwrap();
        let missing: Vec<&String> = terms.iter().filter(|t| !t.trim().is_empty() && !found.keys().any(|k| k.eq_ignore_ascii_case(t.trim()))).collect();
        eprintln!("terms found: {} / not found: {} {:?}", found.len(), missing.len(), missing);
    }
}
