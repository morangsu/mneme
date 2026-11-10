#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""下载 mneme 桌面 OCR 所需的模型与运行时（零第三方依赖，纯标准库）。

来源是 PyPI 官方 wheel（rapidocr-onnxruntime 内含 PP-OCRv4 三模型，
onnxruntime 内含 onnxruntime.dll），约 30MB 下载、16MB 落盘：

    python tools/fetch-ocr-models.py            # → data/models/
    python tools/fetch-ocr-models.py <目录>     # 或 MNEME_OCR_MODELS 指向的目录

桌面对应的搜索顺序见 crates/mneme-server/src/ocr.rs：环境变量 MNEME_OCR_MODELS
→ <数据目录>/models → exe 同级 models → 工作目录 models。
"""
import json
import os
import sys
import tempfile
import urllib.request
import zipfile

WHEELS = {
    "rapidocr-onnxruntime": ("rapidocr_onnxruntime-1.4.4-py3-none-any.whl", [
        ("rapidocr_onnxruntime/models/ch_PP-OCRv4_det_infer.onnx", "ch_PP-OCRv4_det_infer.onnx"),
        ("rapidocr_onnxruntime/models/ch_ppocr_mobile_v2.0_cls_infer.onnx", "ch_ppocr_mobile_v2.0_cls_infer.onnx"),
        ("rapidocr_onnxruntime/models/ch_PP-OCRv4_rec_infer.onnx", "ch_PP-OCRv4_rec_infer.onnx"),
    ]),
    "onnxruntime": (None, [  # 文件名按版本取，代码里动态找 capi/onnxruntime.dll
        ("onnxruntime/capi/onnxruntime.dll", "onnxruntime.dll"),
    ]),
}


def pick_ort_url():
    with urllib.request.urlopen("https://pypi.org/pypi/onnxruntime/json", timeout=60) as r:
        d = json.load(r)
    # 任选一个 win_amd64 wheel（dll 在各 cp 标签里是同一份）
    for u in d["urls"]:
        if "win_amd64" in u["filename"] and u["filename"].endswith(".whl"):
            return u["url"]
    raise SystemExit("onnxruntime PyPI 上没有 win_amd64 wheel")


def fetch(url, dest):
    print(f"下载 {url}")
    with urllib.request.urlopen(url, timeout=300) as r, open(dest, "wb") as f:
        while True:
            chunk = r.read(1 << 20)
            if not chunk:
                break
            f.write(chunk)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join("data", "models")
    os.makedirs(out, exist_ok=True)
    got = []
    with tempfile.TemporaryDirectory() as tmp:
        # rapidocr：固定版本 wheel，模型文件名稳定
        url = ("https://files.pythonhosted.org/packages/ba/12/"
               "1e5497183bdbe782dbb91bad1d0d2297dba4d2831b2652657f7517bfc6df/"
               "rapidocr_onnxruntime-1.4.4-py3-none-any.whl")
        whl = os.path.join(tmp, "rapidocr.whl")
        fetch(url, whl)
        with zipfile.ZipFile(whl) as z:
            for src, dst in WHEELS["rapidocr-onnxruntime"][1]:
                with open(os.path.join(out, dst), "wb") as f:
                    f.write(z.read(src))
            got.append(dst)

        # onnxruntime：文件名带版本，动态解析
        ort_whl = os.path.join(tmp, "ort.whl")
        fetch(pick_ort_url(), ort_whl)
        with zipfile.ZipFile(ort_whl) as z2:
            for src, dst in WHEELS["onnxruntime"][1]:
                with open(os.path.join(out, dst), "wb") as f:
                    f.write(z2.read(src))
            got.append(dst)

    print("已写入：")
    for name in got:
        p = os.path.join(out, name)
        print(f"  {p}  ({os.path.getsize(p) / 1e6:.1f} MB)")
    print("完成。桌面端「截图识字」即可用（或设 MNEME_OCR_MODELS 指向该目录）。")


if __name__ == "__main__":
    main()
