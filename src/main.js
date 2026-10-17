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
const optionGpuAcceleration = document.getElementById("option-gpu-acceleration");
const imageSettingsDialog = document.getElementById("image-settings-dialog");
const imageSettingsForm = document.getElementById("image-settings-form");
const btnImageSettings = document.getElementById("btn-image-settings");
const defaultImageOptions = Object.freeze({
    gpuAcceleration: true,
    protectVisualImages: true,
    jpegEnabled: true, jpegQuality: 80, downsample: true, maxLongEdge: 2000,
    allowColorToGray: false, colorDifference: 20, colorPixelPercent: 5,
    grayscaleReduction: "preserve", colorReduction: "preserve",
});
const imageFields = {
    protectVisualImages: document.getElementById("advanced-protect-visual"),
    jpegEnabled: document.getElementById("advanced-jpeg-enabled"),
    jpegQuality: document.getElementById("advanced-jpeg-quality"),
    downsample: document.getElementById("advanced-downsample"),
    maxLongEdge: document.getElementById("advanced-max-edge"),
    allowColorToGray: document.getElementById("advanced-allow-gray"),
    colorDifference: document.getElementById("advanced-color-difference"),
    colorPixelPercent: document.getElementById("advanced-color-percent"),
    grayscaleReduction: document.getElementById("advanced-gray-reduction"),
    colorReduction: document.getElementById("advanced-color-reduction"),
};
const grayReductionModes = {
    auto: { label: "自动（自适应）", hint: "逐图分析灰阶分布和量化误差，自动选择 4–128 级；保护浅色层次和连续渐变。不会自动将 8-bit 图像转为纯黑白，原始 1-bit 图像保持黑白。" },
    preserve: { label: "保留原始灰阶（默认）", hint: "保留灰阶，不主动二值化；原本的 1-bit 黑白图保持黑白。" },
    ...Object.fromEntries([128, 64, 32, 16, 8, 4].map(levels => [`levels${levels}`, {
        label: `${levels} 级灰阶`,
        hint: `编码前保留 ${levels} 级灰阶，不自动转为纯黑白。${levels >= 64 ? "保留较多明暗层次，适合照片和带阴影的扫描。" : levels >= 16 ? "简化明暗层次，渐变可能出现色带。" : "仅保留少量明暗层次，照片、阴影和浅色笔迹可能损失细节。"}`,
        group: "灰阶减色 · 逐档减少层次",
    }])),
    monochrome: { label: "纯黑白（2 色，阈值 160）", hint: "按阈值 160 转为纯黑白，不保留中间灰阶；适合黑字白底，照片、阴影和浅色笔迹可能丢失。", group: "纯黑白 · 二值化" },
};
const colorReductionModes = {
    auto: { label: "自动（自适应）", hint: "逐图分析颜色分布和量化误差，自动选择每通道 2–64 级；保护少量浅色像素和连续渐变。是否转灰度仍由上方开关控制。" },
    preserve: { label: "保留原始颜色（默认）", hint: "保留颜色层次，仍会按 JPEG 质量和分辨率设置压缩。" },
    ...Object.fromEntries([64, 32, 16, 8, 4, 2].map(levels => [`levels${levels}`, {
        label: `${levels} 级 / 通道 · 最多 ${(levels ** 3).toLocaleString("zh-CN")} 色`,
        hint: `编码前每个 RGB 通道保留 ${levels} 级，最多 ${(levels ** 3).toLocaleString("zh-CN")} 种颜色；这是通道色阶，不是整张图的调色板大小。${levels >= 32 ? "较多保留颜色层次。" : levels >= 8 ? "渐变可能出现明显色带。" : "颜色会大幅简化，适合颜色较少的图表，照片可能失真。"}`,
    }])),
};
const reductionModes = { grayscaleReduction: grayReductionModes, colorReduction: colorReductionModes };
function normalizeReduction(key, value) {
    if (value === "mild") return "levels32";
    if (value === "aggressive") return key === "grayscaleReduction" ? "monochrome" : "levels8";
    return value;
}
function reductionLabel(key, value) {
    if (key === "grayscaleReduction" && value === "mild") return "32 级灰阶（旧版自动黑白）";
    return reductionModes[key][normalizeReduction(key, value)]?.label || "—";
}
for (const [key, modes] of Object.entries(reductionModes)) {
    const select = imageFields[key];
    const groups = new Map();
    for (const [value, mode] of Object.entries(modes)) {
        let parent = select;
        if (mode.group) {
            if (!groups.has(mode.group)) {
                const group = document.createElement("optgroup");
                group.label = mode.group;
                select.append(group);
                groups.set(mode.group, group);
            }
            parent = groups.get(mode.group);
        }
        parent.append(new Option(mode.label, value));
    }
}
const jpegSlider = document.getElementById("advanced-jpeg-slider");
let imageOptions = { ...defaultImageOptions };
function validStoredImageOption(key, value) {
    const defaultValue = defaultImageOptions[key];
    if (typeof defaultValue === "boolean") return typeof value === "boolean";
    if (typeof defaultValue === "string") return Object.hasOwn(reductionModes[key], value);
    if (!Number.isFinite(value)) return false;
    if (key === "colorPixelPercent") return value >= 0 && value <= 100;
    const ranges = { jpegQuality: [1, 100], maxLongEdge: [256, 20000], colorDifference: [1, 255] };
    return Number.isInteger(value) && value >= ranges[key][0] && value <= ranges[key][1];
}
try {
    const saved = JSON.parse(localStorage.getItem("imageCompression") || "{}");
    for (const key of Object.keys(defaultImageOptions)) {
        const value = reductionModes[key] ? normalizeReduction(key, saved?.[key]) : saved?.[key];
        if (validStoredImageOption(key, value)) imageOptions[key] = value;
    }
} catch (_) {}
optionGpuAcceleration.checked = imageOptions.gpuAcceleration;
optionGpuAcceleration.addEventListener("change", () => {
    imageOptions.gpuAcceleration = optionGpuAcceleration.checked;
    try { localStorage.setItem("imageCompression", JSON.stringify(imageOptions)); } catch (_) {}
});
function updateImageSettingsSummary() {
    optionGpuAcceleration.disabled = isProcessing || !optionCompressImages.checked;
    const pieces = [imageOptions.jpegEnabled ? `JPEG 质量 ${imageOptions.jpegQuality}` : "无损编码",
        imageOptions.downsample ? `长边 ${imageOptions.maxLongEdge} px` : "原始分辨率",
        imageOptions.allowColorToGray ? "允许转灰度" : "保留彩色"];
    if (imageOptions.protectVisualImages) pieces.push("封面／图片保护");
    if (imageOptions.grayscaleReduction !== "preserve") pieces.push(reductionLabel("grayscaleReduction", imageOptions.grayscaleReduction));
    if (imageOptions.colorReduction !== "preserve") pieces.push(`彩色 ${reductionLabel("colorReduction", imageOptions.colorReduction)}`);
    document.getElementById("image-settings-summary").textContent = optionCompressImages.checked
        ? pieces.join(" · ") : "压缩已关闭，开启后使用高级设置";
    btnImageSettings.disabled = isProcessing;
}
function updateAdvancedControls() {
    imageFields.jpegQuality.disabled = jpegSlider.disabled = !imageFields.jpegEnabled.checked;
    imageFields.maxLongEdge.disabled = !imageFields.downsample.checked;
    imageFields.colorDifference.disabled = imageFields.colorPixelPercent.disabled = !imageFields.allowColorToGray.checked;
    document.getElementById("gray-reduction-description").textContent = grayReductionModes[imageFields.grayscaleReduction.value].hint;
    document.getElementById("color-reduction-description").textContent = colorReductionModes[imageFields.colorReduction.value].hint;
}
function fillImageSettings(options) {
    for (const [key, field] of Object.entries(imageFields)) {
        if (field.type === "checkbox") field.checked = options[key];
        else field.value = options[key];
    }
    jpegSlider.value = options.jpegQuality;
    updateAdvancedControls();
}
btnImageSettings.addEventListener("click", () => {
    if (isProcessing) return;
    closeHelp();
    fillImageSettings(imageOptions);
    imageSettingsDialog.showModal();
});
for (const id of ["btn-image-settings-close", "btn-image-settings-cancel"]) {
    document.getElementById(id).addEventListener("click", () => imageSettingsDialog.close());
}
document.getElementById("btn-image-settings-reset").addEventListener("click", () => fillImageSettings(defaultImageOptions));
imageSettingsDialog.addEventListener("close", () => btnImageSettings.focus());
imageSettingsDialog.addEventListener("click", (event) => {
    if (event.target !== imageSettingsDialog) return;
    const bounds = imageSettingsDialog.getBoundingClientRect();
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) imageSettingsDialog.close();
});
jpegSlider.addEventListener("input", () => { imageFields.jpegQuality.value = jpegSlider.value; });
imageFields.jpegQuality.addEventListener("input", () => {
    if (imageFields.jpegQuality.validity.valid) jpegSlider.value = imageFields.jpegQuality.value;
});
imageSettingsForm.addEventListener("change", updateAdvancedControls);
imageSettingsForm.addEventListener("submit", (event) => {
    event.preventDefault();
    if (!imageSettingsForm.reportValidity()) return;
    const next = { ...imageOptions };
    for (const [key, field] of Object.entries(imageFields)) {
        next[key] = field.type === "checkbox" ? field.checked
            : field.type === "number" ? (field.disabled && !validStoredImageOption(key, field.valueAsNumber)
                ? imageOptions[key] : field.valueAsNumber) : field.value;
    }
    imageOptions = next;
    try { localStorage.setItem("imageCompression", JSON.stringify(imageOptions)); } catch (_) {}
    updateImageSettingsSummary();
    imageSettingsDialog.close();
});
optionCompressImages.addEventListener("change", updateImageSettingsSummary);
updateImageSettingsSummary();

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
        imageCompression: { ...imageOptions },
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
    updateImageSettingsSummary();
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
        updateImageSettingsSummary();
        optionNormalizePages.disabled = false;
        cropMode.disabled = false;
        optionRepairPdf.disabled = false;
        optionRepairBookmarks.disabled = false;
        optionFitBookmarks.disabled = false;
        updateWidthOptions();
    }
});

function formatCount(value) {
    return (Number.isFinite(value) ? value : 0).toLocaleString("zh-CN", { maximumFractionDigits: 1 });
}

function reductionText(before, after) {
    if (!(before > 0)) return "—";
    const percent = (1 - after / before) * 100;
    return `${percent >= 0 ? "减少" : "增加"} ${Math.abs(percent).toFixed(1)}%`;
}

function adaptiveReductionLabel(decision) {
    if (decision.levels === 256) return "保留原始层次";
    if (decision.per_channel) return `每通道 ${decision.levels} 级（最多 ${formatCount(decision.levels ** 3)} 色）`;
    return decision.levels === 2 ? "保持原始纯黑白" : `${decision.levels} 级灰阶`;
}

function renderImageReport(result, options) {
    if (!result.image_settings && !options.compressImages) return '<p>图片压缩：未启用</p>';
    const stats = result.image_statistics;
    const settings = result.image_settings || options.imageCompression;
    const changes = stats?.changes || [];
    const protections = stats?.visual_protections || [];
    const protectionById = new Map(protections.map(item => [item.object_id.join(":"), item]));
    const classification = `输入识别：彩色 ${formatCount(result.color_images)} · 灰度 ${formatCount(result.grayscale_images)} · 纯黑白 ${formatCount(result.monochrome_images)}`;
    if (!stats) return `<p>${classification} · 已压缩 ${formatCount(result.optimized_images)} · 跳过 ${formatCount(result.skipped_images)}</p>`;
    if (!stats.total_images) return '<section class="image-report"><h4>图像优化</h4><p>未发现可引用的扫描图像。</p></section>';
    const written = result.output_bytes > 0 ? changes.length : 0;
    const colors = changes.filter(change => change.color_converted || change.binarized || change.quantized);
    const beforeColors = colors.reduce((total, change) => total + change.sampled_colors_before, 0);
    const afterColors = colors.reduce((total, change) => total + change.sampled_colors_after, 0);
    const adaptiveChanges = changes.filter(change => change.adaptive_reduction);
    const actionCounts = [
        ["彩色转灰度／黑白", stats.color_converted_images],
        ["二值化", stats.binarized_images], ["色阶量化", stats.quantized_images],
        ["降低分辨率", stats.downsampled_images], ["JPEG 编码", stats.jpeg_images], ["Flate 编码", stats.flate_images],
    ];
    if (adaptiveChanges.length) actionCounts.push(["自适应选择", adaptiveChanges.length]);
    const reasons = [
        ["收益未超过 2%", stats.insufficient_savings_images],
        ["已是黑白编码", stats.already_monochrome_images],
        ["蒙版或特殊解码参数", stats.protected_images],
        ["编码或尺寸不支持", stats.unsupported_images],
        ["重新编码失败", stats.encoding_failed_images],
        ["整份文件未变小，放弃候选结果", stats.discarded_images],
    ].filter(([, count]) => count > 0);
    const settingsText = settings ? [
        settings.jpegEnabled ? `JPEG 质量 ${settings.jpegQuality}` : "无损 Flate",
        settings.downsample ? `长边 ≤ ${settings.maxLongEdge} px` : "原始分辨率",
        `黑白／灰度：${reductionLabel("grayscaleReduction", settings.grayscaleReduction)}`,
        `彩色：${reductionLabel("colorReduction", settings.colorReduction)}`,
        settings.allowColorToGray ? `转灰度：彩色比例 ≤ ${settings.colorPixelPercent}%，差异阈值 ${settings.colorDifference}` : "关闭彩色转灰度",
        settings.protectVisualImages ? "封面／图片保护开启（触发时采用更保守参数）" : "封面／图片保护关闭",
    ].map(escapeHtml).join(" · ") : "";
    const rows = changes.map(change => {
        const actions = [change.color_converted && "转灰度", change.binarized && "二值化", change.quantized && "量化", change.downsampled && "缩小"].filter(Boolean);
        const automatic = change.adaptive_reduction;
        const protection = protectionById.get(change.object_id.join(":"));
        const protectionText = protection ? `<small>保护：${escapeHtml(protection.reason)}</small>` : "";
        const adaptiveText = automatic ? `<small>自动 → ${escapeHtml(adaptiveReductionLabel(automatic))} · ${escapeHtml(automatic.reason)}</small>` : "";
        const encoding = change.encoding === "DCTDecode" ? "JPEG" : "Flate";
        const sampled = change.color_converted || change.binarized || change.quantized
            ? `${formatCount(change.sampled_colors_before)} → ${formatCount(change.sampled_colors_after)}` : "—";
        return `<tr><td>${formatCount(change.object_id[0])} ${formatCount(change.object_id[1])} R</td>
            <td>${actions.join("、") || "重新编码"}<small>${encoding} · ${change.original_bits_per_pixel} → ${change.bits_per_pixel} bit/像素</small>${adaptiveText}${protectionText}</td>
            <td>${change.original_width} × ${change.original_height}<small>→ ${change.width} × ${change.height}</small></td>
            <td>${sampled}</td><td>${formatBytes(change.original_bytes)} → ${formatBytes(change.output_bytes)}<small>${reductionText(change.original_bytes, change.output_bytes)}</small></td></tr>`;
    }).join("");
    const protectionRows = protections.map(item => {
        const policy = [item.jpeg_quality == null ? "无损 Flate" : `JPEG ${item.jpeg_quality}`,
            item.max_long_edge == null ? "原始分辨率" : `长边 ≤ ${item.max_long_edge} px`, "保留颜色与灰阶"];
        return `<tr><td>${formatCount(item.object_id[0])} ${formatCount(item.object_id[1])} R</td><td>${escapeHtml(item.reason)}</td><td>${escapeHtml(policy.join(" · "))}</td><td>${item.retained_original ? "保留原图" : "已按保护参数压缩"}</td></tr>`;
    }).join("");
    return `<section class="image-report" aria-label="图像优化统计">
        <h4>图像优化 · 实际结果</h4>
        <div class="image-metrics">
            <div><span>实际写回</span><strong>${formatCount(written)} / ${formatCount(stats.total_images)} 张</strong><small>保留原图 ${formatCount(stats.total_images - written)} 张</small></div>
            <div><span>图像流节省</span><strong>${formatBytes(stats.original_image_bytes - stats.output_image_bytes)}</strong><small>${formatBytes(stats.original_image_bytes)} → ${formatBytes(stats.output_image_bytes)} · ${reductionText(stats.original_image_bytes, stats.output_image_bytes)}</small></div>
            <div><span>图像像素</span><strong>${reductionText(stats.original_pixels, stats.output_pixels)}</strong><small>${formatCount(stats.original_pixels / 1e6)} → ${formatCount(stats.output_pixels / 1e6)} 百万像素</small></div>
        </div>
        <p class="image-report-types">${classification}</p>
        ${protections.length ? `<details class="image-change-details"><summary>封面／图片保护：${formatCount(protections.length)} 张（含保留原图 ${formatCount(protections.filter(item => item.retained_original).length)} 张）</summary><p class="image-report-note">首尾页位置和连续色调区域为启发式判断，可能包含正文页；共享图像在任一首尾页出现时统一采用保护参数。普通线条插图仍可能使用正文参数。</p><div class="image-table-scroll" tabindex="0" role="region" aria-label="封面与图片保护明细"><table class="image-change-table"><thead><tr><th scope="col">图像对象</th><th scope="col">判断依据</th><th scope="col">候选压缩参数</th><th scope="col">结果</th></tr></thead><tbody>${protectionRows}</tbody></table></div></details>` : ""}
        <dl class="image-actions">${actionCounts.map(([label, count]) => `<div><dt>${label}</dt><dd>${formatCount(count)} 张</dd></div>`).join("")}</dl>
        ${colors.length ? `<p class="palette-summary">颜色处理 ${formatCount(colors.length)} 张 · 每张平均采样颜色 <strong>${formatCount(beforeColors / colors.length)} → ${formatCount(afterColors / colors.length)}</strong>（${reductionText(beforeColors, afterColors)}）</p>` : '<p class="palette-summary">没有实际写回的减色或颜色转换；保留颜色层次仍可通过缩小和重新编码节省体积。</p>'}
        ${reasons.length ? `<p class="retention-reasons"><strong>保留原图原因：</strong>${reasons.map(([reason, count]) => `${reason} ${formatCount(count)} 张`).join("；")}</p>` : ""}
        <p class="image-report-note">按唯一图像对象统计，共享图像只计一次；同一图像可同时缩小、转换和重新编码。颜色数为每张最多 50,000 个像素的采样值，比较输入与编码前结果，包含缩放、转换和量化的影响；JPEG 输出可能产生近似颜色。</p>
        ${rows ? `<details class="image-change-details"><summary>逐图处理明细（${formatCount(changes.length)} 张）</summary><div class="image-table-scroll" tabindex="0" role="region" aria-label="逐图处理明细"><table class="image-change-table"><thead><tr><th scope="col">图像对象</th><th scope="col">实际处理</th><th scope="col">像素尺寸</th><th scope="col">采样颜色</th><th scope="col">图像体积</th></tr></thead><tbody>${rows}</tbody></table></div></details>` : ""}
        <details class="image-settings-used"><summary>本次使用的图像设置</summary><p>${settingsText}</p></details>
        <p class="image-report-acceleration">预处理：${escapeHtml(result.image_acceleration || "CPU")} · GPU ${formatCount(result.gpu_images)} 张 / CPU ${formatCount(result.cpu_images)} 张（含未采用的候选）</p>
    </section>`;
}

function showResults(results) {
    showWorkspace("results");
    resultsControls.hidden = results.length === 0;
    const succeeded = results.filter((result) => result.success).length;
    const failed = results.length - succeeded;
    resultsSummary.innerHTML = results.length
        ? `<span>共 ${results.length} 个文件</span><span class="results-count-success">成功 ${succeeded}</span><span class="${failed ? "results-count-error" : ""}">失败 ${failed}</span>`
        : "暂无处理结果";
    const options = activeOptions || getProcessingOptions();
    const successful = results.filter(result => result.success);
    const originalTotal = successful.reduce((total, result) => total + result.original_bytes, 0);
    const outputTotal = successful.reduce((total, result) => total + (result.output_bytes > 0 ? result.output_bytes : result.original_bytes), 0);
    if (originalTotal > 0) {
        const saved = originalTotal - outputTotal;
        resultsSummary.innerHTML += `<span>合计${saved >= 0 ? "节省" : "增加"} ${formatBytes(Math.abs(saved))} · ${reductionText(originalTotal, outputTotal)}</span>`;
    }
    resultsList.innerHTML = results.map((r) => {
        const filename = r.input_path.split(/[\\/]/).pop() || r.input_path;
        const status = r.success ? (r.output_bytes > 0 ? "已输出" : "无需处理") : "失败";
        const statusClass = r.success ? "success" : "error";
        let preview;
        let detail;
        if (r.success) {
            const minW = r.original_widths.length ? Math.min(...r.original_widths).toFixed(1) : "—";
            const maxW = r.original_widths.length ? Math.max(...r.original_widths).toFixed(1) : "—";
            const sizeChange = r.original_bytes > 0
                ? (1 - r.output_bytes / r.original_bytes) * 100
                : 0;
            const sizeChangeText = sizeChange >= 0
                ? `减少 ${sizeChange.toFixed(1)}%`
                : `增加 ${Math.abs(sizeChange).toFixed(1)}%`;
            preview = r.output_bytes > 0
                ? `${r.page_count} 页 · ${formatBytes(r.original_bytes)} → ${formatBytes(r.output_bytes)} · ${sizeChangeText}`
                : `${r.page_count} 页 · 无需生成新文件`;
            if (r.output_bytes > 0 && r.image_statistics) {
                const colors = r.image_statistics.changes.filter(change => change.color_converted || change.binarized || change.quantized).length;
                preview += ` · 颜色处理 ${formatCount(colors)} 张 · 缩小 ${formatCount(r.image_statistics.downsampled_images)} 张`;
            }
            const sizeDetail = r.output_bytes > 0
                ? `体积: ${formatBytes(r.original_bytes)} → ${formatBytes(r.output_bytes)}（${sizeChangeText}）<br>`
                : `原始体积: ${formatBytes(r.original_bytes)}<br>`;
            const normalizationDetail = options.normalizePages
                ? `可见宽度范围: ${minW} ~ ${maxW} pt | 目标宽度（${options.widthMode === "userDefined" ? "自定义" : "自动识别"}）: ${r.target_width.toFixed(2)} pt`
                : "页面归一化：未启用";
            const cropDetail = options.cropMode === "trueCrop"
                ? "CropBox：真正裁剪（MediaBox 与可见区域一致）"
                : "CropBox：按裁剪后的可见宽度对齐";
            detail = `
                页数: ${r.page_count} | ${normalizationDetail}<br>
                ${cropDetail}<br>
                ${renderImageReport(r, options)}
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
