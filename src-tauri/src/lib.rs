mod pdf_processor;

use pdf_processor::{ProcessOptions, ProcessResult, ProcessingProgress};
use serde::Serialize;
use tauri::Emitter;
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Clone, Serialize)]
struct BatchProgress {
    input_path: String,
    file_index: usize,
    file_count: usize,
    phase: String,
    page: usize,
    page_count: usize,
    image: usize,
    image_count: usize,
    progress: f32,
}

#[tauri::command]
async fn select_files(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let files = app
        .dialog()
        .file()
        .add_filter("PDF 文件", &["pdf"])
        .set_title("选择 PDF 文件")
        .blocking_pick_files();

    match files {
        Some(paths) => Ok(paths
            .iter()
            .filter_map(|p| p.as_path().map(|path| path.to_string_lossy().to_string()))
            .collect()),
        None => Ok(vec![]),
    }
}

#[tauri::command]
async fn process_pdfs(
    app: tauri::AppHandle,
    paths: Vec<String>,
    options: ProcessOptions,
) -> Vec<ProcessResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let file_count = paths.len();
        let _ = app.emit(
            "pdf-progress",
            BatchProgress {
                input_path: String::new(),
                file_index: 0,
                file_count,
                phase: "准备处理".to_string(),
                page: 0,
                page_count: 0,
                image: 0,
                image_count: 0,
                progress: 0.0,
            },
        );

        paths
            .iter()
            .enumerate()
            .map(|(file_index, path)| {
                let mut report_progress = |progress: ProcessingProgress| {
                    let overall_progress =
                        (file_index as f32 + progress.progress) / file_count.max(1) as f32;
                    let _ = app.emit(
                        "pdf-progress",
                        BatchProgress {
                            input_path: progress.input_path,
                            file_index: file_index + 1,
                            file_count,
                            phase: progress.phase,
                            page: progress.page,
                            page_count: progress.page_count,
                            image: progress.image,
                            image_count: progress.image_count,
                            progress: overall_progress.clamp(0.0, 1.0),
                        },
                    );
                };

                let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    pdf_processor::process_pdf_with_options_and_progress(
                        path,
                        options,
                        &mut report_progress,
                    )
                })) {
                    Ok(result) => result,
                    Err(_) => ProcessResult {
                        input_path: path.clone(),
                        output_path: String::new(),
                        page_count: 0,
                        original_widths: Vec::new(),
                        target_width: 0.0,
                        color_images: 0,
                        grayscale_images: 0,
                        monochrome_images: 0,
                        optimized_images: 0,
                        skipped_images: 0,
                        original_bytes: 0,
                        output_bytes: 0,
                        success: false,
                        error: Some("处理该 PDF 时发生内部异常，已跳过此文件".to_string()),
                    },
                };

                let _ = app.emit(
                    "pdf-progress",
                    BatchProgress {
                        input_path: path.clone(),
                        file_index: file_index + 1,
                        file_count,
                        phase: if result.success {
                            "文件完成".to_string()
                        } else {
                            "文件失败，继续下一个".to_string()
                        },
                        page: result.page_count,
                        page_count: result.page_count,
                        image: 0,
                        image_count: 0,
                        progress: (file_index + 1) as f32 / file_count.max(1) as f32,
                    },
                );
                result
            })
            .collect()
    })
    .await
    .unwrap_or_else(|e| {
        vec![ProcessResult {
            input_path: String::new(),
            output_path: String::new(),
            page_count: 0,
            original_widths: Vec::new(),
            target_width: 0.0,
            color_images: 0,
            grayscale_images: 0,
            monochrome_images: 0,
            optimized_images: 0,
            skipped_images: 0,
            original_bytes: 0,
            output_bytes: 0,
            success: false,
            error: Some(format!("处理任务失败: {}", e)),
        }]
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![select_files, process_pdfs])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
