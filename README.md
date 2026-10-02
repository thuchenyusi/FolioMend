# FolioMend

FolioMend 是一款本地运行的扫描 PDF 整理工具，用于批量统一页面的可见宽度、处理裁剪框，并在尽量保留文字层、矢量内容和书签的前提下减小扫描文档体积。

## 功能

- 批量选择或拖放 PDF 文件。
- 以 `CropBox` 可见区域为基准统一页面宽度。
- 可选将 `MediaBox` 真正裁剪到可见区域。
- 识别彩色、灰度和纯黑白图像，采用不同策略压缩。
- 同步调整书签等目标坐标，避免页面缩放后定位错乱。
- 输出文件自动添加 `_normalized` 后缀，并避免覆盖已有文件。
- 处理进度精确到当前文件、页面和图像。

FolioMend 不会上传 PDF；文件选择、解析和输出均在本机完成。

## 下载与使用

在本仓库的 GitHub Releases 页面下载 Windows x64 Portable ZIP，解压后运行 `FolioMend.exe`，无需安装 FolioMend。

运行要求：Windows 10/11 x64，已安装 [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。Portable 包不包含该运行时；WebView 缓存仍保存在系统用户目录。

选择或拖入 PDF 后开始处理，输出文件保存在原文件目录，请确保目录可写。

## 开发环境

- [Node.js](https://nodejs.org/)
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
cargo test --manifest-path src-tauri/Cargo.toml
```

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
