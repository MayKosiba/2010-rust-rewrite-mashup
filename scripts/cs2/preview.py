"""Draws a converted weapon at clip frames, flat shaded from three sides, to
check its skinning offline. usage: preview.py <weapon.json> <out.png> <clip> [frame...]"""
import json
import sys

import numpy as np
from PIL import Image, ImageDraw


def quat_mat(q):
    x, y, z, w = q
    return np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])


def trs(v):
    m = np.eye(4)
    m[:3, :3] = quat_mat(v[3:7]) * np.array(v[7:10])
    m[:3, 3] = v[0:3]
    return m


def pose(doc, clip, frame):
    sk = doc["skeleton"]
    tracks = doc["clips"][clip]["bones"] if clip else {}
    world = []
    for i, name in enumerate(sk["names"]):
        local = trs(tracks[name][min(frame, len(tracks[name]) - 1)] if name in tracks else sk["rest"][i])
        p = sk["parents"][i]
        world.append(local if p < 0 else world[p] @ local)
    ibm = [np.array(m).reshape(4, 4, order="F") for m in sk["inverse_bind"]]
    return [w @ b for w, b in zip(world, ibm)]


def skin(surface, mats):
    pos = np.array(surface["positions"]).reshape(-1, 3)
    bones = np.array(surface["bones"]).reshape(-1, 4)
    weights = np.array(surface["weights"]).reshape(-1, 4)
    hom = np.concatenate([pos, np.ones((len(pos), 1))], axis=1)
    out = np.zeros_like(pos)
    stack = np.stack(mats)
    for k in range(4):
        m = stack[bones[:, k]]
        out += weights[:, k:k + 1] * np.einsum("nij,nj->ni", m, hom)[:, :3]
    return out


def draw(doc, clip, frame, size=360):
    mats = pose(doc, clip, frame)
    views = [(0, 1), (2, 1), (0, 2)]
    img = Image.new("RGB", (size * 3, size), (30, 30, 36))
    d = ImageDraw.Draw(img)
    colours = [(200, 170, 140), (90, 110, 90), (180, 190, 210), (200, 120, 60)]
    all_pos = [skin(s, mats) for s in doc["surfaces"]]
    pts = np.concatenate(all_pos)
    lo, hi = pts.min(0), pts.max(0)
    span = (hi - lo).max() * 1.1
    mid = (lo + hi) / 2
    for v, (a, b) in enumerate(views):
        depth_axis = 3 - a - b
        tris = []
        for s, p in zip(doc["surfaces"], all_pos):
            idx = np.array(s["indices"]).reshape(-1, 3)
            for t in idx[:: max(1, len(idx) // 6000)]:
                q = p[t]
                n = np.cross(q[1] - q[0], q[2] - q[0])
                shade = abs(n[depth_axis]) / (np.linalg.norm(n) + 1e-9)
                tris.append((q[:, depth_axis].mean(), q, shade, colours[doc["surfaces"].index(s) % 4]))
        tris.sort(key=lambda t: t[0])
        for _, q, shade, c in tris:
            xy = [(v * size + (pt[a] - mid[a]) / span * size + size / 2, size / 2 - (pt[b] - mid[b]) / span * size) for pt in q]
            f = 0.35 + 0.65 * shade
            d.polygon(xy, fill=tuple(int(x * f) for x in c))
    d.text((5, 5), f"{clip} frame {frame}", fill=(255, 255, 255))
    return img


if __name__ == "__main__":
    doc = json.load(open(sys.argv[1]))
    clip = sys.argv[3]
    frames = [int(f) for f in sys.argv[4:]] or [0]
    rows = [draw(doc, clip, f) for f in frames]
    out = Image.new("RGB", (rows[0].width, rows[0].height * len(rows)))
    for i, r in enumerate(rows):
        out.paste(r, (0, i * r.height))
    out.save(sys.argv[2])
