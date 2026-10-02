#!/usr/bin/env pwsh
# FolioMend 构建脚本
# 用法: .\build.ps1 [-Dev] [-Release]

param(
    [switch]$Dev,
    [switch]$Release
)

$ErrorActionPreference = "Stop"

Write-Host "=== FolioMend 构建脚本 ===" -ForegroundColor Cyan

# 检查依赖
Write-Host "`n[1/3] 检查依赖..." -ForegroundColor Yellow

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    Write-Host "错误: 未找到 cargo，请先安装 Rust: https://rustup.rs" -ForegroundColor Red
    exit 1
}

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Host "错误: 未找到 node，请先安装 Node.js: https://nodejs.org" -ForegroundColor Red
    exit 1
}

Write-Host "  Cargo: $(cargo --version)" -ForegroundColor Gray
Write-Host "  Node:  $(node --version)" -ForegroundColor Gray

# 安装 npm 依赖
Write-Host "`n[2/3] 安装依赖..." -ForegroundColor Yellow
npm install
if ($LASTEXITCODE -ne 0) {
    Write-Host "错误: npm install 失败" -ForegroundColor Red
    exit 1
}

# 构建
Write-Host "`n[3/3] 构建应用..." -ForegroundColor Yellow

if ($Dev) {
    Write-Host "  模式: 开发模式" -ForegroundColor Gray
    npx tauri dev
} else {
    Write-Host "  模式: 发布模式" -ForegroundColor Gray
    npx tauri build
    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n构建失败!" -ForegroundColor Red
        exit 1
    }
    Write-Host "`n构建成功!" -ForegroundColor Green
    Write-Host "输出目录: src-tauri\target\release\bundle\" -ForegroundColor Cyan
}
