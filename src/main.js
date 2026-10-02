const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWebviewWindow } = window.__TAURI__.webviewWindow;

let selectedFiles = [];
let stopProgressListener = null;
let activeOptions = null;

const btnSelect = document.getElementById("btn-select");
const btnProcess = document.getElementById("btn-process");
const btnClear = document.getElementById("btn-clear");
const btnAddPath = document.getElementById("btn-add-path");
const manualPath = document.getElementById("manual-path");
const fileList = document.getElementById("file-list");
const progressSection = document.getElementById("progress-section");
const progressFill = document.getElementById("progress-fill");
const progressText = document.getElementById("progress-text");
const resultsSection = document.getElementById("results-section");
const resultsList = document.getElementById("results-list");
const optionCompressImages = document.getElementById("option-compress-images");
const optionNormalizePages = document.getElementById("option-normalize-pages");
const optionRepairBookmarks = document.getElementById("option-repair-bookmarks");
try { optionRepairBookmarks.checked = localStorage.getItem("repairBookmarks") !== "false"; } catch (_) {}
optionRepairBookmarks.addEventListener("change", () => {
    try { localStorage.setItem("repairBookmarks", String(optionRepairBookmarks.checked)); } catch (_) {}
});
const optionFitBookmarks = document.getElementById("option-fit-bookmarks");
try { optionFitBookmarks.checked = localStorage.getItem("fitBookmarksToWidth") === "true"; } catch (_) {}
optionFitBookmarks.addEventListener("change", () => {
    try { localStorage.setItem("fitBookmarksToWidth", String(optionFitBookmarks.checked)); } catch (_) {}
});
const cropMode = document.getElementById("crop-mode");
const cropModeHint = document.getElementById("crop-mode-hint");

function getProcessingOptions() {
    return {
        compressImages: optionCompressImages.checked,
        normalizePages: optionNormalizePages.checked,
        cropMode: cropMode.value,
        repairBookmarks: optionRepairBookmarks.checked,
        fitBookmarksToWidth: optionFitBookmarks.checked,
    };
}

function updateCropModeHint() {
    cropModeHint.textContent = cropMode.value === "trueCrop"
        ? "把 MediaBox 直接裁到 CropBox；若同时开启页面归一化，还会统一裁剪后的可见宽度。"
        : "保留原始 MediaBox；开启页面归一化时，按 CropBox 的实际可见宽度等比对齐。";
}

cropMode.addEventListener("change", updateCropModeHint);

function updateFileList() {
    if (selectedFiles.length === 0) {
        fileList.innerHTML = '<p class="empty-hint">尚未选择任何文件</p>';
        btnProcess.disabled = true;
    } else {
        fileList.innerHTML = selectedFiles
            .map(
                (f, i) => `
            <div class="file-item">
                <span>${escapeHtml(f)}</span>
                <button class="remove-btn" data-index="${i}" title="移除">&times;</button>
            </div>
        `
            )
            .join("");
        btnProcess.disabled = false;
    }
}

function escapeHtml(str) {
    return String(str).replace(/[&<>'"]/g, (char) => ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        "'": "&#39;",
        '"': "&quot;",
    })[char]);
}

function formatBytes(bytes) {
    if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
    const units = ["B", "KB", "MB", "GB"];
    const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
    return `${(bytes / Math.pow(1024, unitIndex)).toFixed(unitIndex === 0 ? 0 : 1)} ${units[unitIndex]}`;
}

function showMessage(message, type = "error") {
    resultsSection.style.display = "block";
    resultsList.innerHTML = `<div class="result-item ${type}">
        <div class="result-error">${escapeHtml(message)}</div>
    </div>`;
}

function addFiles(paths) {
    for (const p of paths) {
        const trimmed = p.trim();
        if (trimmed && !selectedFiles.includes(trimmed)) {
            selectedFiles.push(trimmed);
        }
    }
    updateFileList();
}

// 选择文件按钮
btnSelect.addEventListener("click", async () => {
    try {
        const files = await invoke("select_files");
        if (files && files.length > 0) {
            addFiles(files);
        }
    } catch (e) {
        showMessage(`选择文件失败: ${e}`);
    }
});

// 手动添加路径
function addManualPath() {
    const path = manualPath.value.trim();
    if (!path) {
        return;
    }
    if (!path.toLowerCase().endsWith(".pdf")) {
        showMessage("只能添加 PDF 文件路径");
        return;
    }

    addFiles([path]);
    manualPath.value = "";
}

btnAddPath.addEventListener("click", addManualPath);
manualPath.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
        addManualPath();
    }
});

// 移除文件
fileList.addEventListener("click", (e) => {
    const btn = e.target.closest(".remove-btn");
    if (btn) {
        const index = parseInt(btn.dataset.index);
        selectedFiles.splice(index, 1);
        updateFileList();
    }
});

// 清空列表
btnClear.addEventListener("click", () => {
    selectedFiles = [];
    updateFileList();
    resultsSection.style.display = "none";
});

// 处理文件
btnProcess.addEventListener("click", async () => {
    if (selectedFiles.length === 0) return;

    btnProcess.disabled = true;
    btnSelect.disabled = true;
    optionCompressImages.disabled = true;
    optionNormalizePages.disabled = true;
    cropMode.disabled = true;
    optionRepairBookmarks.disabled = true;
    optionFitBookmarks.disabled = true;
    progressSection.style.display = "block";
    resultsSection.style.display = "none";
    progressFill.style.width = "0%";
    progressFill.style.background = "#00b894";
    progressText.textContent = `正在处理 ${selectedFiles.length} 个文件...`;

    try {
        activeOptions = getProcessingOptions();
        stopProgressListener = await listen("pdf-progress", (event) => {
            const progress = event.payload || {};
            const percent = Math.round(Math.max(0, Math.min(1, progress.progress || 0)) * 100);
            progressFill.style.width = `${percent}%`;

            const filePart = progress.file_count
                ? `文件 ${progress.file_index}/${progress.file_count}`
                : "准备中";
            const fileName = progress.input_path
                ? progress.input_path.split(/[\\/]/).pop()
                : "";
            const namePart = fileName ? ` | ${fileName}` : "";
            const pagePart = progress.page_count
                ? ` | 页面 ${progress.page}/${progress.page_count}`
                : "";
            const imagePart = progress.image_count
                ? ` | 图像 ${progress.image}/${progress.image_count}`
                : "";
            progressText.textContent = `${filePart}${namePart}${pagePart}${imagePart} | ${progress.phase || "处理中"} | ${percent}%`;
        });

        const results = await invoke("process_pdfs", {
            paths: selectedFiles,
            options: activeOptions,
        });

        progressFill.style.width = "100%";
        progressText.textContent = "处理完成！";

        showResults(results);
    } catch (e) {
        progressText.textContent = `处理出错: ${e}`;
        progressFill.style.width = "100%";
        progressFill.style.background = "#d63031";
    } finally {
        if (stopProgressListener) {
            stopProgressListener();
            stopProgressListener = null;
        }
        btnProcess.disabled = false;
        btnSelect.disabled = false;
        optionCompressImages.disabled = false;
        optionNormalizePages.disabled = false;
        cropMode.disabled = false;
        optionRepairBookmarks.disabled = false;
        optionFitBookmarks.disabled = false;
    }
});

function showResults(results) {
    resultsSection.style.display = "block";
    resultsList.innerHTML = results
        .map((r) => {
            if (r.success) {
                const minW = Math.min(...r.original_widths).toFixed(1);
                const maxW = Math.max(...r.original_widths).toFixed(1);
                const options = activeOptions || getProcessingOptions();
                const sizeChange = r.original_bytes > 0
                    ? (1 - r.output_bytes / r.original_bytes) * 100
                    : 0;
                const sizeChangeText = sizeChange >= 0
                    ? `减少 ${sizeChange.toFixed(1)}%`
                    : `增加 ${Math.abs(sizeChange).toFixed(1)}%`;
                const sizeDetail = r.output_bytes > 0
                    ? `体积: ${formatBytes(r.original_bytes)} → ${formatBytes(r.output_bytes)}（${sizeChangeText}）<br>`
                    : `原始体积: ${formatBytes(r.original_bytes)}<br>`;
                const normalizationDetail = options.normalizePages
                    ? `可见宽度范围: ${minW} ~ ${maxW} pt | 目标宽度: ${r.target_width.toFixed(1)} pt`
                    : "页面归一化：未启用";
                const imageDetail = options.compressImages
                    ? `图像识别: 彩色 ${r.color_images} | 灰度 ${r.grayscale_images} | 纯黑白 ${r.monochrome_images} | 已压缩 ${r.optimized_images} | 跳过 ${r.skipped_images}`
                    : "图片压缩：未启用";
                const cropDetail = options.cropMode === "trueCrop"
                    ? "CropBox：真正裁剪（MediaBox 与可见区域一致）"
                    : "CropBox：按裁剪后的可见宽度对齐";
                return `
                <div class="result-item success">
                    <div class="result-filename">${escapeHtml(r.input_path)}</div>
                    <div class="result-detail">
                        页数: ${r.page_count} | 
                        ${normalizationDetail}<br>
                        ${cropDetail}<br>
                        ${imageDetail}<br>
                        书签检测：${escapeHtml(r.bookmark_status || "未启用")}<br>
                        ${options.fitBookmarksToWidth ? `书签适合宽度：已更新 ${r.bookmarks_fit_width || 0}，跳过 ${r.bookmarks_fit_width_skipped || 0}（外部链接或无法解析的目标）<br>` : ""}
                        ${sizeDetail}
                        输出文件: ${escapeHtml(r.output_path)}
                    </div>
                </div>
            `;
            } else {
                return `
                <div class="result-item error">
                    <div class="result-filename">${escapeHtml(r.input_path)}</div>
                    <div class="result-error">错误: ${escapeHtml(r.error || "未知错误")}</div>
                </div>
            `;
            }
        })
        .join("");
}

// 初始化
updateCropModeHint();
updateFileList();

// 拖拽文件支持
const dropOverlay = document.getElementById("drop-overlay");

getCurrentWebviewWindow().onDragDropEvent((event) => {
    if (event.payload.type === "over") {
        dropOverlay.style.display = "flex";
    } else if (event.payload.type === "drop") {
        dropOverlay.style.display = "none";
        const paths = event.payload.paths || [];
        const pdfPaths = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
        if (pdfPaths.length > 0) {
            addFiles(pdfPaths);
        }
    } else if (event.payload.type === "cancel") {
        dropOverlay.style.display = "none";
    }
});
