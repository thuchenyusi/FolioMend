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

测试使用内存数据或系统临时目录中动态生成的 PDF，不依赖仓库外的本地文件。

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

## 持续集成（CI）

GitHub Actions 配置位于 `.github/workflows/ci.yml`。PR 创建、更新或重新打开时，以及提交推送到 `main` 时（包括 PR 合并后），会自动运行基础测试，也支持在 Actions 页面手动运行。

CI 使用 Windows runner，执行 `npm ci`、JavaScript 语法检查，以及所有 Rust 测试目标的编译与测试。Rust 使用 `--locked`；依赖缓存用于加快后续运行。每次 `main` 推送都会执行检查，PR 的新提交会取消该 PR 尚未完成的旧检查。

本地执行相同的基础检查：

```powershell
npm ci
npm run check
cargo test --locked --all-targets --manifest-path src-tauri/Cargo.toml
```

运行结果可在仓库 Actions 页面查看。若需要在测试失败时禁止合并，可在 GitHub 的 `main` 分支保护规则中将 `Basic tests (Windows)` 设为必需状态检查。基础 CI 不包含桌面界面自动化或安装包构建。
