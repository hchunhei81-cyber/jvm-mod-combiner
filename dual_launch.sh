#!/bin/bash
pkill -f "KnotClient" 2>/dev/null
pkill -f "neoforge" 2>/dev/null

echo "=================================================="
echo "   JVM Mod Combiner - Dual Sandbox Fusion Mode"
echo "=================================================="

echo "[Fusion] 启动 Sandbox A (Fabric 1.21.4)..."
# 切换进项目目录运行，这样 rust 才能找到 ./sandbox/1.21.4.json
(cd ~/jvm-mod-combiner && cargo run -- --mode fabric 2>&1 | sed 's/^/[Fabric] /') &

sleep 3

echo "[Fusion] 启动 Sandbox B (NeoForge 1.21.1)..."
(cd ~/jvm-mod-combiner && cargo run -- --mode neoforge 2>&1 | sed 's/^/[NeoForge] /') &

echo "[Fusion] 双沙盒已下发指令，等待窗口弹出..."
wait
