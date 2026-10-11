//! End-to-end CPU/GPU comparison using the same PDF pipeline as the desktop app.
//! cargo run --release --example benchmark -- <input.pdf> <report.json> [rounds]
#![allow(dead_code)]
#[path = "../src/bookmark_repair.rs"]
mod bookmark_repair;
#[path = "../src/gpu_images.rs"]
mod gpu_images;
#[path = "../src/pdf_processor.rs"]
mod pdf_processor;
#[path = "../src/pdf_repair.rs"]
mod pdf_repair;

use pdf_processor::{ProcessOptions, ProcessingProgress};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

fn run(
    input: &Path,
    gpu: bool,
    measured: bool,
    round: usize,
    folder: &Path,
    preserve: bool,
) -> Value {
    let mut options = ProcessOptions::default();
    options.image_compression.gpu_acceleration = gpu;
    let mode = if gpu { "gpu" } else { "cpu" };
    eprintln!(
        "{} {mode}, round {round}",
        if measured { "Measure" } else { "Warmup" }
    );
    let start = Instant::now();
    let mut image_start = None;
    let mut image_end = None;
    let mut save_start = None;
    let mut progress = |event: ProcessingProgress| {
        match event.phase.as_str() {
            "分析图像" => image_start = Some(start.elapsed().as_secs_f64() * 1000.0),
            "图像处理完成" => image_end = Some(start.elapsed().as_secs_f64() * 1000.0),
            "保存 PDF" => save_start = Some(start.elapsed().as_secs_f64() * 1000.0),
            _ => {}
        }
        if event.image_count > 0 && (event.image % 200 == 0 || event.image == event.image_count) {
            eprintln!("  {mode}: images {}/{}", event.image, event.image_count);
        }
    };
    let result = pdf_processor::process_pdf_with_options_and_progress(
        input.to_str().unwrap(),
        options,
        &mut progress,
    );
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert!(result.success, "{mode}: {:?}", result.error);
    if !gpu {
        assert_eq!(result.gpu_images, 0, "CPU run must bypass the GPU");
    }
    // Validate outside the timed interval so QA does not distort processing time.
    let output = PathBuf::from(&result.output_path);
    let validation = lopdf::Document::load(&output).expect("benchmark output must be readable");
    assert_eq!(validation.get_pages().len(), result.page_count);
    let output_bytes = fs::metadata(&output).unwrap().len();
    let mut kept_path = None;
    if output != input {
        assert_eq!(
            output.parent().unwrap().canonicalize().unwrap(),
            folder.canonicalize().unwrap()
        );
        if preserve {
            let destination = folder.join(format!("{mode}-output.pdf"));
            fs::rename(&output, &destination).unwrap();
            kept_path = Some(destination.to_string_lossy().into_owned());
        } else {
            fs::remove_file(&output).unwrap();
        }
    }
    eprintln!(
        "  {mode}: {:.3}s, {} pages, GPU {} / CPU {}, {} bytes, {}",
        elapsed_ms / 1000.0,
        result.page_count,
        result.gpu_images,
        result.cpu_images,
        output_bytes,
        result.image_acceleration
    );
    json!({"mode": mode, "measured": measured, "round": round, "total_ms": elapsed_ms,
        "read_repair_analyze_ms": image_start,
        "image_processing_ms": image_end.zip(image_start).map(|(end, begin)| end - begin),
        "page_navigation_ms": save_start.zip(image_end).map(|(end, begin)| end - begin),
        "pdf_write_ms": save_start.map(|begin| elapsed_ms - begin),
        "output_bytes": output_bytes, "validated_pages": validation.get_pages().len(),
        "preserved_output": kept_path, "result": result})
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.len() % 2 == 1 {
        sorted[sorted.len() / 2]
    } else {
        (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
    }
}

fn summarize(runs: &[Value], mode: &str) -> Value {
    let selected: Vec<&Value> = runs
        .iter()
        .filter(|run| run["measured"] == true && run["mode"] == mode)
        .collect();
    let totals: Vec<f64> = selected
        .iter()
        .map(|run| run["total_ms"].as_f64().unwrap())
        .collect();
    let images: Vec<f64> = selected
        .iter()
        .filter_map(|run| run["image_processing_ms"].as_f64())
        .collect();
    json!({"runs": selected.len(), "median_total_ms": median(&totals),
        "median_image_processing_ms": if images.is_empty() { None } else { Some(median(&images)) },
        "min_total_ms": totals.iter().copied().reduce(f64::min),
        "max_total_ms": totals.iter().copied().reduce(f64::max),
        "output_bytes": selected.iter().map(|run| &run["output_bytes"]).collect::<Vec<_>>(),
        "gpu_images": selected.iter().map(|run| &run["result"]["gpu_images"]).collect::<Vec<_>>(),
        "cpu_images": selected.iter().map(|run| &run["result"]["cpu_images"]).collect::<Vec<_>>()})
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() >= 3,
        "usage: benchmark <input.pdf> <report.json> [rounds]"
    );
    let original = PathBuf::from(&args[1]).canonicalize().unwrap();
    let report_path = PathBuf::from(&args[2]);
    let rounds = args
        .get(3)
        .map(|n| n.parse::<usize>().unwrap())
        .unwrap_or(3);
    assert!((1..=20).contains(&rounds));
    if let Some(parent) = report_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).unwrap();
        }
    }
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let folder = std::env::current_dir()
        .unwrap()
        .join("tmp")
        .join(format!("gpu-benchmark-{}", epoch.as_millis()));
    fs::create_dir_all(&folder).unwrap();
    let input = folder.join("input.pdf");
    fs::copy(&original, &input).unwrap();
    let mut report = json!({"input": original, "input_bytes": fs::metadata(&original).unwrap().len(),
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "timestamp_unix_seconds": epoch.as_secs(), "rounds_per_mode": rounds,
        "scope": "existing wgpu preprocessing; JPEG/Flate encoding and PDF output on CPU",
        "default_options": ProcessOptions::default(), "working_directory": folder,
        "timing": "wall time including PDF load, GPU initialization (warmup), image processing and output write; copy and output validation excluded",
        "runs": []});
    let mut runs = Vec::new();
    for gpu in [false, true] {
        runs.push(run(&input, gpu, false, 0, &folder, false));
        report["runs"] = json!(runs);
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    for round in 1..=rounds {
        let order = if round % 2 == 1 {
            [false, true]
        } else {
            [true, false]
        };
        for gpu in order {
            runs.push(run(&input, gpu, true, round, &folder, round == 1));
            report["runs"] = json!(runs);
            fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
    }
    let cpu = summarize(&runs, "cpu");
    let gpu = summarize(&runs, "gpu");
    let speedup =
        cpu["median_total_ms"].as_f64().unwrap() / gpu["median_total_ms"].as_f64().unwrap();
    report["summary"] = json!({"cpu": cpu, "gpu": gpu, "speedup": speedup,
        "time_reduction_percent": (1.0 - 1.0 / speedup) * 100.0});
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&report["summary"]).unwrap()
    );
    // Remove only the copied input; keep first measured outputs for inspection.
    fs::remove_file(input).unwrap();
}
