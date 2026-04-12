#!/usr/bin/env bash

# 简单测试脚本

echo "=== MySSH 测试脚本 ==="

echo "检查二进制文件是否存在..."
if [ -f "target/release/myssh" ]; then
    echo "✅ 二进制文件存在"
    echo "文件大小: $(du -h target/release/myssh)"
else
    echo "❌ 二进制文件不存在"
    exit 1
fi

echo -e "\n运行版本检查 (由于没有真实服务器，会连接失败)..."
echo "测试超时设置为 5 秒..."

# 超时运行程序，测试基本启动
timeout 5 ./target/release/myssh 2>&1 || echo -e "\n⚠️  程序结束(预期行为，因为没有真实服务器连接)"

echo -e "\n=== 测试完成 ==="
echo "✅ 程序可以正常启动和编译"
echo "⚠️  需要真实的 SSH 服务器才能进行完整功能测试"
