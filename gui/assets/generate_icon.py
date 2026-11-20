#!/usr/bin/env python3
"""Generate assets/icon.ico for orphan-audit-gui（无第三方依赖，仅标准库）。

设计：深海军蓝圆角底 + 蓝色放大镜（审计）+ 镜内三行包清单条目。
配色与 GUI 一致（背景 0x12151c~0x252c3d，强调 0x7aa2f7，前景 0xc8d3f5）。
渲染：1024px SDF 光栅化（1px 解析抗锯齿）→ 面积平均降采样到 256/64/48/32/16
→ 打包为 PNG-in-ICO（Vista+ 原生支持）。重新生成：python generate_icon.py

TSD 注意（深信服 DLP 环境）：python 写出的文件会被透明加密（文件头 %TSD），
其他进程（rustc/git/资源编译器按信任策略而定）可能读到密文。生成后需用
bash 重定向重铸为明文：python -c "import sys; sys.stdout.buffer.write(
open('icon.ico','rb').read())" > icon.ico.tmp && mv -f icon.ico.tmp icon.ico
"""

import struct
import zlib

SIZE = 1024  # 主渲染尺寸


# ---------- SDF 图元（返回带 1px 抗锯齿的覆盖率 [0,1]） ----------

def cov_rect(px, py, cx, cy, hw, hh, r):
    """圆角矩形（中心 cx,cy，半宽 hw，半高 hh，圆角 r）。"""
    dx = abs(px - cx) - (hw - r)
    dy = abs(py - cy) - (hh - r)
    inside = min(max(dx, dy), 0.0)
    d = (inside * inside + max(dx, 0.0) ** 2 + max(dy, 0.0) ** 2) ** 0.5 - r
    return min(max(0.5 - d, 0.0), 1.0)


def cov_circle(px, py, cx, cy, rad):
    d = ((px - cx) ** 2 + (py - cy) ** 2) ** 0.5 - rad
    return min(max(0.5 - d, 0.0), 1.0)


def cov_ring(px, py, cx, cy, rad, half_t):
    """圆环（中心线半径 rad，半厚度 half_t）。"""
    d = abs(((px - cx) ** 2 + (py - cy) ** 2) ** 0.5 - rad)
    return min(max(half_t + 0.5 - d, 0.0), 1.0)


def cov_capsule(px, py, ax, ay, bx, by, rad):
    """线段 (a→b) 两侧扩 rad 的胶囊。"""
    pax, pay = px - ax, py - ay
    bax, bay = bx - ax, by - ay
    h = max(0.0, min(1.0, (pax * bax + pay * bay) / (bax * bax + bay * bay)))
    dx, dy = pax - bax * h, pay - bay * h
    d = (dx * dx + dy * dy) ** 0.5 - rad
    return min(max(0.5 - d, 0.0), 1.0)


# ---------- 渲染 ----------

BG_TOP = (0x25, 0x2C, 0x3D)
BG_BOT = (0x14, 0x18, 0x24)
ACCENT = (0x7A, 0xA2, 0xF7)
BARS = (0xC8, 0xD3, 0xF5)

# 布局（1024 画布）：镜框圆心偏左上，柄向右下
L_CX, L_CY, L_R, L_T = 430, 420, 205, 46
HANDLE_A = (L_CX + 103, L_CY + 103)  # 从圆环 45° 处起笔
HANDLE_B = (800, 790)
LENS_R = 158
BAR = [  # (cx, cy, half_w, half_h)
    (430, 340, 75, 17),
    (430, 415, 52, 17),
    (430, 490, 65, 17),
]


def render():
    buf = bytearray(SIZE * SIZE * 4)
    for y in range(SIZE):
        for x in range(SIZE):
            px, py = x + 0.5, y + 0.5
            # 1. 渐变圆角底
            a = cov_rect(px, py, 512, 512, 480, 480, 200)
            t = y / SIZE
            r = BG_TOP[0] + (BG_BOT[0] - BG_TOP[0]) * t
            g = BG_TOP[1] + (BG_BOT[1] - BG_TOP[1]) * t
            b = BG_TOP[2] + (BG_BOT[2] - BG_TOP[2]) * t
            # 2. 镜片提亮（白色 7%）
            lens = cov_circle(px, py, L_CX, L_CY, LENS_R)
            la = lens * 0.07
            r += (255 - r) * la
            g += (255 - g) * la
            b += (255 - b) * la
            # 3. 镜内清单条（按镜片覆盖率裁剪，营造"镜中看到"）
            for bcx, bcy, bhw, bhh in BAR:
                c = cov_rect(px, py, bcx, bcy, bhw, bhh, bhh) * lens * 0.92
                r += (BARS[0] - r) * c
                g += (BARS[1] - g) * c
                b += (BARS[2] - b) * c
            # 4. 镜框 + 镜柄（强调色，柄压在最上）
            ring = cov_ring(px, py, L_CX, L_CY, L_R, L_T)
            hand = cov_capsule(px, py, *HANDLE_A, *HANDLE_B, L_T)
            c = max(ring, hand)
            r += (ACCENT[0] - r) * c
            g += (ACCENT[1] - g) * c
            b += (ACCENT[2] - b) * c

            i = (y * SIZE + x) * 4
            buf[i] = int(r)
            buf[i + 1] = int(g)
            buf[i + 2] = int(b)
            buf[i + 3] = int(a * 255)
    return buf


# ---------- 面积平均降采样 ----------

def downscale(buf, src, dst):
    out = bytearray(dst * dst * 4)
    for y in range(dst):
        y0, y1 = y * src / dst, (y + 1) * src / dst
        for x in range(dst):
            x0, x1 = x * src / dst, (x + 1) * src / dst
            rs = gs = bs = as_ = ws = 0.0
            for sy in range(int(y0), min(int(y1) + 1, src)):
                wy = min(y1, sy + 1) - max(y0, sy)
                for sx in range(int(x0), min(int(x1) + 1, src)):
                    wx = min(x1, sx + 1) - max(x0, sx)
                    w = wx * wy
                    i = (sy * src + sx) * 4
                    rs += buf[i] * w
                    gs += buf[i + 1] * w
                    bs += buf[i + 2] * w
                    as_ += buf[i + 3] * w
                    ws += w
            o = (y * dst + x) * 4
            out[o] = int(rs / ws)
            out[o + 1] = int(gs / ws)
            out[o + 2] = int(bs / ws)
            out[o + 3] = int(as_ / ws)
    return out


# ---------- PNG / ICO 组装 ----------

def write_png(path, w, rgba):
    raw = bytearray()
    for y in range(w):
        raw.append(0)  # filter: None
        raw += rgba[y * w * 4:(y + 1) * w * 4]

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, w, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def write_ico(path, images):
    """images: [(size, png_bytes)]，PNG-in-ICO（Vista+ 支持）。"""
    hdr = struct.pack("<HHH", 0, 1, len(images))
    entries = b""
    offset = 6 + 16 * len(images)
    blobs = b""
    for size, png in images:
        w = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", w, w, 0, 0, 1, 32, len(png), offset)
        blobs += png
        offset += len(png)
    with open(path, "wb") as f:
        f.write(hdr + entries + blobs)


if __name__ == "__main__":
    import io
    import os

    here = os.path.dirname(os.path.abspath(__file__))
    print("rendering 1024px ...")
    buf = render()
    images = []
    for size in (256, 64, 48, 32, 16):
        small = downscale(buf, SIZE, size) if size != SIZE else buf
        p = os.path.join(here, f"icon_{size}.png")
        write_png(p, size, small)
        with open(p, "rb") as f:
            images.append((size, f.read()))
        print(f"  {size}px -> {os.path.basename(p)}")
    write_ico(os.path.join(here, "icon.ico"), images)
    print("wrote icon.ico")
    # 中间 png 只留 256 一份做预览参考，其余清理
    for size in (64, 48, 32, 16):
        os.remove(os.path.join(here, f"icon_{size}.png"))
