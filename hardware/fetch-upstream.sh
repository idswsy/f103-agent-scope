#!/usr/bin/env bash
#
# hardware/fetch-upstream.sh
#
# 拉取上游立创开源工程的 4 个可下载附件。
# 这些文件体积大（合计约 10 MB）且版权归上游，所以**不入库** ——
# 用这个脚本按需重新拉取。
#
# 用法：
#   ./hardware/fetch-upstream.sh          # 只下缺失的
#   ./hardware/fetch-upstream.sh --force  # 全部重下
#
set -euo pipefail

cd "$(dirname "$0")"
DEST="upstream/downloads"
mkdir -p "$DEST"

FORCE=0
[[ "${1:-}" == "--force" ]] && FORCE=1

# 上游附件直链（来自 oshwhub 项目页 data/href）
declare -a FILES=(
  "简易数字示波器焊接文档.pdf|https://image.lceda.cn/attachments/2024/1/kxWXt8GSAQ8m7qzU4GvzzPgvxt3mFEBf8VAnQUxn.pdf"
  "物料清单-简易数字示波器.xlsx|https://image.lceda.cn/attachments/2024/1/HWVhBxnWjIL06wyudbUSIlaQjCMn1HyjBhldkK2Q.bin"
  "PCB焊接辅助工具-简易数字示波器V1.2.html|https://image.lceda.cn/attachments/2024/1/9NgRZm8f6DeFWEJuZrEAqwWCDmlEEJMf9GHYpRJz.html"
  "简易数字示波器-装配图.pdf|https://image.lceda.cn/oshwhub/project/attachments/4ef4a80fb8bc40ce9fe6de7bde763661.pdf"
)

command -v curl >/dev/null 2>&1 || { echo "需要 curl" >&2; exit 1; }

ok=0; skip=0; fail=0

for entry in "${FILES[@]}"; do
    name="${entry%%|*}"
    url="${entry##*|}"
    out="$DEST/$name"

    if [ -f "$out" ] && [ "$FORCE" -ne 1 ]; then
        echo "跳过（已存在）: $name"
        skip=$((skip + 1))
        continue
    fi

    echo "下载: $name"
    if curl -sSL --max-time 180 -A "Mozilla/5.0" -o "$out" "$url"; then
        size=$(wc -c < "$out")
        if [ "$size" -lt 1000 ]; then
            echo "  ⚠ 文件只有 $size 字节，可能是错误页。URL 可能已失效。" >&2
            fail=$((fail + 1))
        else
            echo "  ✓ $((size / 1024)) KB"
            ok=$((ok + 1))
        fi
    else
        echo "  ✗ 下载失败" >&2
        fail=$((fail + 1))
    fi
done

echo
echo "完成: 下载 $ok / 跳过 $skip / 失败 $fail"

cat <<'EOF'

────────────────────────────────────────────────────────────
注意：原理图 / PCB 源文件 / Gerber **不在**这些附件里。

要改板必须先在立创EDA专业版里克隆在线工程：
  工程 UUID: 5e03b6545745463bb8b64813209ffb8c
  1. 登录 https://pro.lceda.cn
  2. 打开该工程 → 另存为 / 克隆到自己账号
  3. 导出 → Gerber / BOM / 坐标文件

详见 hardware/README.md
────────────────────────────────────────────────────────────
EOF
