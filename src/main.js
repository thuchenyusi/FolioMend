import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

let selectedFiles = [];
let stopProgressListener = null;
let activeOptions = null;
let isProcessing = false;

const btnSelect = document.getElementById("btn-select");
const btnProcess = document.getElementById("btn-process");
const btnClear = document.getElementById("btn-clear");
const btnAddPath = document.getElementById("btn-add-path");
const manualPath = document.getElementById("manual-path");
const fileList = document.getElementById("file-list");
const progressFill = document.getElementById("progress-fill");
const progressText = document.getElementById("progress-text");
const resultsSection = document.getElementById("results-section");
const resultsList = document.getElementById("results-list");
const resultsSummary = document.getElementById("results-summary");
const resultsControls = document.getElementById("results-controls");
const btnResultsExpandAll = document.getElementById("btn-results-expand-all");
const btnResultsCollapseAll = document.getElementById("btn-results-collapse-all");
const tabFiles = document.getElementById("tab-files");
const tabResults = document.getElementById("tab-results");
const fileCount = document.getElementById("file-count");
const progressBar = document.getElementById("progress-bar");
const optionHelp = document.getElementById("btn-option-help");
const optionsGrid = document.getElementById("options-grid");

for (const label of optionsGrid.querySelectorAll(".check-option")) {
    label.title = label.querySelector("small").textContent;
}
optionHelp.addEventListener("click", () => {
    const expanded = optionsGrid.classList.toggle("show-descriptions");
    optionHelp.setAttribute("aria-expanded", String(expanded));
    optionHelp.textContent = expanded ? "收起说明" : "显示说明";
});

// Help lives above the scroll panels so opening it never changes their geometry.
let activeHelpButton = null;
let activeHelpPopover = null;
let helpPinned = false;
let helpCloseTimer = null;

function closeHelp() {
    clearTimeout(helpCloseTimer);
    if (!activeHelpButton) return;
    activeHelpButton.setAttribute("aria-expanded", "false");
    if (typeof activeHelpPopover.hidePopover === "function") activeHelpPopover.hidePopover();
    activeHelpPopover.classList.remove("is-open");
    activeHelpButton = null;
    activeHelpPopover = null;
    helpPinned = false;
}

function openHelp(button, pin = false) {
    clearTimeout(helpCloseTimer);
    if (activeHelpButton !== button) closeHelp();
    activeHelpButton = button;
    activeHelpPopover = document.getElementById(button.getAttribute("aria-controls"));
    helpPinned = pin || helpPinned;
    button.setAttribute("aria-expanded", "true");
    if (typeof activeHelpPopover.showPopover === "function") activeHelpPopover.showPopover();
    else activeHelpPopover.classList.add("is-open");

    const anchor = button.getBoundingClientRect();
    const popup = activeHelpPopover.getBoundingClientRect();
    const margin = 12;
    const left = Math.max(margin, Math.min(anchor.left, window.innerWidth - popup.width - margin));
    const below = anchor.bottom + 8;
    const top = Math.max(margin, Math.min(
        below + popup.height <= window.innerHeight - margin ? below : anchor.top - popup.height - 8,
        window.innerHeight - popup.height - margin,
    ));
    activeHelpPopover.style.left = `${left}px`;
    activeHelpPopover.style.top = `${top}px`;
}

function queueHelpClose() {
    clearTimeout(helpCloseTimer);
    helpCloseTimer = setTimeout(() => {
        if (!activeHelpButton || helpPinned) return;
        if (activeHelpButton.matches(":hover") || activeHelpPopover.matches(":hover")) return;
        if (document.activeElement === activeHelpButton) return;
        closeHelp();
    }, 150);
}

for (const button of document.querySelectorAll(".info-button")) {
    const popup = document.getElementById(button.getAttribute("aria-controls"));
    button.addEventListener("pointerenter", () => {
        if (!helpPinned || activeHelpButton === button) openHelp(button);
    });
    button.addEventListener("pointerleave", queueHelpClose);
    button.addEventListener("focus", () => openHelp(button));
    button.addEventListener("blur", queueHelpClose);
    button.addEventListener("click", () => {
        if (activeHelpButton === button && helpPinned) closeHelp();
        else openHelp(button, true);
    });
    popup.addEventListener("pointerenter", () => clearTimeout(helpCloseTimer));
    popup.addEventListener("pointerleave", queueHelpClose);
}
document.addEventListener("pointerdown", (event) => {
    if (activeHelpButton && !activeHelpButton.contains(event.target)
        && !activeHelpPopover.contains(event.target)) closeHelp();
});
document.addEventListener("focusin", (event) => {
    if (activeHelpButton && !activeHelpButton.contains(event.target)
        && !activeHelpPopover.contains(event.target)) closeHelp();
});
document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && activeHelpButton) {
        event.preventDefault();
        closeHelp();
    }
});
document.addEventListener("scroll", (event) => {
    if (!activeHelpButton || activeHelpPopover.contains(event.target)) return;
    // Keyboard focus can scroll a help button into view; keep its explanation attached.
    if (event.target === optionsGrid && !helpPinned && document.activeElement === activeHelpButton) {
        const anchor = activeHelpButton.getBoundingClientRect();
        const panel = optionsGrid.getBoundingClientRect();
        if (anchor.bottom > panel.top && anchor.top < panel.bottom) {
            openHelp(activeHelpButton);
            return;
        }
    }
    closeHelp();
}, true);
window.addEventListener("resize", closeHelp);

function showWorkspace(view) {
    closeHelp();
    const showFiles = view === "files";
    fileList.hidden = !showFiles;
    resultsSection.hidden = showFiles;
    tabFiles.setAttribute("aria-selected", String(showFiles));
    tabResults.setAttribute("aria-selected", String(!showFiles));
    tabFiles.tabIndex = showFiles ? 0 : -1;
    tabResults.tabIndex = showFiles ? -1 : 0;
}

for (const [tab, view] of [[tabFiles, "files"], [tabResults, "results"]]) {
    tab.addEventListener("click", () => showWorkspace(view));
    tab.addEventListener("keydown", (event) => {
        if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        const target = event.key === "Home" ? tabFiles
            : event.key === "End" ? tabResults
                : tab === tabFiles ? tabResults : tabFiles;
        showWorkspace(target === tabFiles ? "files" : "results");
        target.focus();
    });
}

function setProgress(percent, text, isError = false) {
    progressFill.style.width = `${percent}%`;
    progressFill.style.background = isError ? "#d63031" : "";
    progressBar.setAttribute("aria-valuenow", String(percent));
    progressText.textContent = text;
    progressText.title = text;
}

btnResultsExpandAll.addEventListener("click", () => {
    for (const file of resultsList.querySelectorAll("details.result-item")) file.open = true;
});
btnResultsCollapseAll.addEventListener("click", () => {
    for (const file of resultsList.querySelectorAll("details.result-item")) file.open = false;
});
const optionCompressImages = document.getElementById("option-compress-images");
const optionNormalizePages = document.getElementById("option-normalize-pages");
const optionRepairPdf = document.getElementById("option-repair-pdf");
try { optionRepairPdf.checked = localStorage.getItem("repairPdf") !== "false"; } catch (_) {}
optionRepairPdf.addEventListener("change", () => {
    try { localStorage.setItem("repairPdf", String(optionRepairPdf.checked)); } catch (_) {}
});
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
const widthMode = document.getElementById("width-mode");
const customWidth = document.getElementById("custom-width");
const widthModeHint = document.getElementById("width-mode-hint");
let rememberedCustomWidth = "";

try {
    widthMode.value = localStorage.getItem("widthMode") === "userDefined" ? "userDefined" : "auto";
    rememberedCustomWidth = localStorage.getItem("customWidth") || "";
} catch (_) {}

function updateWidthOptions() {
    const isCustom = widthMode.value === "userDefined";
    widthMode.disabled = isProcessing || !optionNormalizePages.checked;
    customWidth.disabled = widthMode.disabled || !isCustom;
    customWidth.value = isCustom ? rememberedCustomWidth : "";
    customWidth.placeholder = isCustom ? "例如 595.28" : "自动识别";
    widthModeHint.textContent = !optionNormalizePages.checked
        ? "开启“统一页面宽度”后，可设置目标宽度。"
        : isCustom
            ? "本批次全部 PDF 使用同一宽度；A4 短边约 595.28 pt。"
            : "为每个 PDF 分别识别合适的宽度，自动排除异常宽页的影响。";
}

optionNormalizePages.addEventListener("change", updateWidthOptions);
widthMode.addEventListener("change", () => {
    try { localStorage.setItem("widthMode", widthMode.value); } catch (_) {}
    updateWidthOptions();
});
customWidth.addEventListener("input", () => {
    rememberedCustomWidth = customWidth.value;
    try { localStorage.setItem("customWidth", rememberedCustomWidth); } catch (_) {}
});

function getProcessingOptions() {
    return {
        compressImages: optionCompressImages.checked,
        normalizePages: optionNormalizePages.checked,
        widthMode: widthMode.value,
        customWidth: optionNormalizePages.checked && widthMode.value === "userDefined"
            ? customWidth.valueAsNumber
            : null,
        cropMode: cropMode.value,
        repairPdf: optionRepairPdf.checked,
        repairBookmarks: optionRepairBookmarks.checked,
        fitBookmarksToWidth: optionFitBookmarks.checked,
    };
}

function updateCropModeHint() {
    cropModeHint.textContent = cropMode.value === "trueCrop"
        ? "将页面边界裁到当前可见区域；启用统一宽度时，同时等比缩放。"
        : "保留页面边界与裁剪设置；启用统一宽度时，以可见区域为准缩放。";
}

cropMode.addEventListener("change", updateCropModeHint);

function updateFileList() {
    fileCount.textContent = selectedFiles.length;
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
        btnProcess.disabled = isProcessing;
    }
    if (!isProcessing) setProgress(0, selectedFiles.length ? `已添加 ${selectedFiles.length} 个文件，准备就绪` : "等待添加 PDF 文件");
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
    showWorkspace("results");
    resultsControls.hidden = true;
    resultsSummary.textContent = "提示信息";
    resultsList.innerHTML = `<div class="result-message ${type}">
        <div class="result-error">${escapeHtml(message)}</div>
    </div>`;
}

function addFiles(paths) {
    if (isProcessing) return;
    for (const p of paths) {
        const trimmed = p.trim();
        if (trimmed && !selectedFiles.includes(trimmed)) {
            selectedFiles.push(trimmed);
        }
    }
    updateFileList();
    showWorkspace("files");
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
    if (isProcessing) return;
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
    if (isProcessing) return;
    const btn = e.target.closest(".remove-btn");
    if (btn) {
        const index = parseInt(btn.dataset.index);
        selectedFiles.splice(index, 1);
        updateFileList();
    }
});

// 清空列表
btnClear.addEventListener("click", () => {
    if (isProcessing) return;
    selectedFiles = [];
    updateFileList();
    resultsSummary.textContent = "暂无处理结果";
    resultsList.innerHTML = '<p class="empty-hint">处理完成后，在这里查看文件结果</p>';
    resultsControls.hidden = true;
    showWorkspace("files");
});

// 处理文件
btnProcess.addEventListener("click", async () => {
    if (isProcessing || selectedFiles.length === 0) return;

    const options = getProcessingOptions();
    if (options.normalizePages && options.widthMode === "userDefined"
        && (!Number.isFinite(options.customWidth) || options.customWidth <= 0)) {
        showMessage("请输入大于 0 的有效目标宽度（单位 pt）。");
        customWidth.focus();
        return;
    }

    isProcessing = true;
    updateWidthOptions();
    btnProcess.disabled = true;
    btnSelect.disabled = true;
    btnClear.disabled = true;
    btnAddPath.disabled = true;
    manualPath.disabled = true;
    for (const button of fileList.querySelectorAll(".remove-btn")) button.disabled = true;
    optionCompressImages.disabled = true;
    optionNormalizePages.disabled = true;
    cropMode.disabled = true;
    optionRepairPdf.disabled = true;
    optionRepairBookmarks.disabled = true;
    optionFitBookmarks.disabled = true;
    showWorkspace("files");
    resultsSummary.textContent = "处理中，完成后将在此显示结果";
    resultsList.innerHTML = '<p class="empty-hint">正在处理 PDF，请稍候...</p>';
    resultsControls.hidden = true;
    setProgress(0, `正在处理 ${selectedFiles.length} 个文件...`);

    try {
        activeOptions = options;
        stopProgressListener = await listen("pdf-progress", (event) => {
            const progress = event.payload || {};
            const percent = Math.round(Math.max(0, Math.min(1, progress.progress || 0)) * 100);

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
            setProgress(percent, `${filePart}${namePart}${pagePart}${imagePart} | ${progress.phase || "处理中"} | ${percent}%`);
        });

        const results = await invoke("process_pdfs", {
            paths: selectedFiles,
            options: activeOptions,
        });

        setProgress(100, "处理完成！");

        showResults(results);
    } catch (e) {
        setProgress(100, `处理出错: ${e}`, true);
        showMessage(`处理出错: ${e}`);
    } finally {
        isProcessing = false;
        if (stopProgressListener) {
            stopProgressListener();
            stopProgressListener = null;
        }
        btnProcess.disabled = selectedFiles.length === 0;
        btnSelect.disabled = false;
        btnClear.disabled = false;
        btnAddPath.disabled = false;
        manualPath.disabled = false;
        for (const button of fileList.querySelectorAll(".remove-btn")) button.disabled = false;
        optionCompressImages.disabled = false;
        optionNormalizePages.disabled = false;
        cropMode.disabled = false;
        optionRepairPdf.disabled = false;
        optionRepairBookmarks.disabled = false;
        optionFitBookmarks.disabled = false;
        updateWidthOptions();
    }
});

function showResults(results) {
    showWorkspace("results");
    resultsControls.hidden = results.length === 0;
    const succeeded = results.filter((result) => result.success).length;
    const failed = results.length - succeeded;
    resultsSummary.innerHTML = results.length
        ? `<span>共 ${results.length} 个文件</span><span class="results-count-success">成功 ${succeeded}</span><span class="${failed ? "results-count-error" : ""}">失败 ${failed}</span>`
        : "暂无处理结果";
    const options = activeOptions || getProcessingOptions();
    resultsList.innerHTML = results.map((r) => {
        const filename = r.input_path.split(/[\\/]/).pop() || r.input_path;
        const status = r.success ? (r.output_bytes > 0 ? "已输出" : "无需处理") : "失败";
        const statusClass = r.success ? "success" : "error";
        let preview;
        let detail;
        if (r.success) {
            const minW = Math.min(...r.original_widths).toFixed(1);
            const maxW = Math.max(...r.original_widths).toFixed(1);
            const sizeChange = r.original_bytes > 0
                ? (1 - r.output_bytes / r.original_bytes) * 100
                : 0;
            const sizeChangeText = sizeChange >= 0
                ? `减少 ${sizeChange.toFixed(1)}%`
                : `增加 ${Math.abs(sizeChange).toFixed(1)}%`;
            preview = r.output_bytes > 0
                ? `${r.page_count} 页 · ${formatBytes(r.original_bytes)} → ${formatBytes(r.output_bytes)} · ${sizeChangeText}`
                : `${r.page_count} 页 · 无需生成新文件`;
            const sizeDetail = r.output_bytes > 0
                ? `体积: ${formatBytes(r.original_bytes)} → ${formatBytes(r.output_bytes)}（${sizeChangeText}）<br>`
                : `原始体积: ${formatBytes(r.original_bytes)}<br>`;
            const normalizationDetail = options.normalizePages
                ? `可见宽度范围: ${minW} ~ ${maxW} pt | 目标宽度（${options.widthMode === "userDefined" ? "自定义" : "自动识别"}）: ${r.target_width.toFixed(2)} pt`
                : "页面归一化：未启用";
            const imageDetail = options.compressImages
                ? `图像识别: 彩色 ${r.color_images} | 灰度 ${r.grayscale_images} | 纯黑白 ${r.monochrome_images} | 已压缩 ${r.optimized_images} | 跳过 ${r.skipped_images}`
                : "图片压缩：未启用";
            const cropDetail = options.cropMode === "trueCrop"
                ? "CropBox：真正裁剪（MediaBox 与可见区域一致）"
                : "CropBox：按裁剪后的可见宽度对齐";
            detail = `
                页数: ${r.page_count} | ${normalizationDetail}<br>
                ${cropDetail}<br>
                ${imageDetail}<br>
                PDF 自动修复：${escapeHtml(r.pdf_repair_status || "未启用")}<br>
                书签检测：${escapeHtml(r.bookmark_status || "未启用")}<br>
                ${options.fitBookmarksToWidth ? `书签适合宽度：已更新 ${r.bookmarks_fit_width || 0}，跳过 ${r.bookmarks_fit_width_skipped || 0}（外部链接或无法解析的目标）<br>` : ""}
                ${sizeDetail}
                输出文件: ${escapeHtml(r.output_path)}
            `;
        } else {
            preview = escapeHtml(r.error || "未知错误");
            detail = `<div class="result-error">错误: ${escapeHtml(r.error || "未知错误")}</div>`;
        }
        const previewTitle = r.success ? "" : ` title="${escapeHtml(r.error || "未知错误")}"`;
        return `
            <details class="result-item ${statusClass}">
                <summary class="result-file-summary">
                    <span class="result-summary-main">
                        <span class="result-filename" title="${escapeHtml(r.input_path)}">${escapeHtml(filename)}</span>
                        <span class="result-preview ${r.success ? "" : "result-error-preview"}"${previewTitle}>${preview}</span>
                    </span>
                    <span class="result-status ${statusClass}">${status}</span>
                </summary>
                <div class="result-detail">
                    <div class="result-source">输入文件: ${escapeHtml(r.input_path)}</div>
                    ${detail}
                </div>
            </details>
        `;
    }).join("") || '<p class="empty-hint">本批次没有处理结果</p>';
}

// 初始化
updateCropModeHint();
updateWidthOptions();
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
