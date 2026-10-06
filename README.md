# FolioMend

FolioMend 是一款本地运行的扫描 PDF 整理工具，用于批量统一页面的可见宽度、处理裁剪框，并在尽量保留文字层、矢量内容和书签的前提下减小扫描文档体积。

## 功能

- 批量选择或拖放 PDF 文件。
- 以 `CropBox` 可见区域为基准统一页面宽度。
- 目标宽度可选择自动识别或自定义（手动输入，单位 pt）。
- 可选将 `MediaBox` 真正裁剪到可见区域。
- 识别彩色、灰度和纯黑白图像，采用不同策略压缩。
- 同步调整书签等目标坐标，避免页面缩放后定位错乱。
- 输出文件自动添加 `_normalized` 后缀，并避免覆盖已有文件。
- 处理进度精确到当前文件、页面和图像。
- 按页面与尺寸、图像优化、文档修复、书签与导航分类设置处理选项。

FolioMend 不会上传 PDF；文件选择、解析和输出均在本机完成。

## 下载与使用

在本仓库的 GitHub Releases 页面下载 Windows x64 Portable ZIP，解压后运行 `FolioMend.exe`，无需安装 FolioMend。

运行要求：Windows 10/11 x64，已安装 [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。Portable 包不包含该运行时；WebView 缓存仍保存在系统用户目录。

选择或拖入 PDF 后开始处理，输出文件保存在原文件目录，请确保目录可写。

开启“统一页面宽度”后，可选择目标宽度模式：

- **自动识别（默认）**：为每个 PDF 分别计算目标可见页宽；通常取最大宽度，当最大宽度超过 P95 的 1.2 倍时取 P95。
- **自定义**：输入大于 0 的目标宽度，本批次所有 PDF 都按该值等比缩放。单位为 pt（72 pt = 1 英寸，A4 短边约 595.28 pt）。

应用会记住宽度模式和输入值。关闭统一页面宽度后，这些宽度设置不参与处理。

## 开发环境

- [Node.js](https://nodejs.org/) 22.12 或更高版本
- [Rust](https://www.rust-lang.org/tools/install)
- [Tauri 2 系统依赖](https://v2.tauri.app/start/prerequisites/)

## 本地运行

```powershell
npm install
npm run dev
```

也可以使用 PowerShell 构建脚本：

```powershell
.\build.ps1 -Dev
```

## 测试

```powershell
npm run check
npm run frontend:build
cargo test --manifest-path src-tauri/Cargo.toml
```

前端使用 ES Module 和 Vite 构建，Tauri 开发及发布命令会自动启动或构建前端。直接运行 Rust 测试前需先生成 `dist/`。

## 构建安装包

```powershell
npm run build
```

或：

```powershell
.\build.ps1 -Release
```

构建产物位于 `src-tauri/target/release/bundle/`。

## 技术栈

- Tauri 2
- Rust
- HTML / CSS / JavaScript
- `lopdf` 与 `image`

## 项目状态

FolioMend 目前处于早期开发阶段。处理重要文件前，建议保留原始 PDF 备份。
