"""Renders the AK-47's play side (barrel left), textured, for comparing a
bake against pattern references. usage: render_side.py <export dir> <texture.png> <out.png>"""
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from convert import Glb  # noqa: E402


def load_mesh(root):
    glb = Glb(f"{root}/weapons/models/ak47/weapon_rif_ak47.glb")
    tris, uvs, pos_all = [], [], []
    for mesh in glb.json["meshes"]:
        if "body_hd" not in mesh["name"]:
            continue
        for prim in mesh["primitives"]:
            mat = glb.json["materials"][prim["material"]].get("name", "")
            if "sticker" in mat:
                continue
            p = glb.accessor(prim["attributes"]["POSITION"])
            t = glb.accessor(prim["attributes"]["TEXCOORD_0"])
            n = glb.accessor(prim["attributes"]["NORMAL"])
            i = glb.accessor(prim["indices"]).reshape(-1, 3)
            tris.append((p, t, n, i))
    return tris


def render(tris, texture, size=(900, 300), light=np.array([0.3, 0.5, 0.8]), other_side=True):
    tex = np.asarray(texture.convert("RGB"), dtype=np.float32) / 255.0
    th, tw = tex.shape[:2]
    allp = np.concatenate([p for p, _, _, _ in tris])
    lo, hi = allp.min(0), allp.max(0)
    ext = hi - lo
    # The gun's length is its longest axis; up is the second. Barrel left.
    axes = np.argsort(-ext)
    ax_len, ax_up = axes[0], axes[1]
    depth_axis = axes[2]
    W, H = size
    scale = min((W - 20) / ext[ax_len], (H - 20) / ext[ax_up])
    img = np.zeros((H, W, 3), np.float32) + np.array([0.08, 0.1, 0.2])
    zbuf = np.full((H, W), -1e9)
    light = light / np.linalg.norm(light)
    for p, t, n, idx in tris:
        sx = (p[:, ax_len] - lo[ax_len]) * scale + 10
        if other_side:
            sx = W - sx
        sy = H - ((p[:, ax_up] - lo[ax_up]) * scale + 10)
        sz = -p[:, depth_axis] if other_side else p[:, depth_axis]
        for tri in idx:
            xs, ys = sx[tri], sy[tri]
            x0, x1 = int(max(0, xs.min())), int(min(W - 1, xs.max() + 1))
            y0, y1 = int(max(0, ys.min())), int(min(H - 1, ys.max() + 1))
            if x1 < x0 or y1 < y0:
                continue
            gx, gy = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
            (ax, bx, cx), (ay, by, cy) = xs, ys
            den = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy)
            if abs(den) < 1e-9:
                continue
            w0 = ((by - cy) * (gx - cx) + (cx - bx) * (gy - cy)) / den
            w1 = ((cy - ay) * (gx - cx) + (ax - cx) * (gy - cy)) / den
            w2 = 1 - w0 - w1
            inside = (w0 >= 0) & (w1 >= 0) & (w2 >= 0)
            if not inside.any():
                continue
            z = w0 * sz[tri[0]] + w1 * sz[tri[1]] + w2 * sz[tri[2]]
            yy, xx = np.nonzero(inside)
            zz = z[inside]
            px, py = xx + x0, yy + y0
            closer = zz > zbuf[py, px]
            if not closer.any():
                continue
            px, py, zz = px[closer], py[closer], zz[closer]
            b = np.stack([w0[inside][closer], w1[inside][closer], w2[inside][closer]], 1)
            uv = b @ t[tri]
            nn = b @ n[tri]
            nn /= np.linalg.norm(nn, axis=1, keepdims=True) + 1e-9
            u = np.clip((uv[:, 0] % 1.0) * tw, 0, tw - 1).astype(int)
            v = np.clip((uv[:, 1] % 1.0) * th, 0, th - 1).astype(int)
            shade = 0.45 + 0.55 * np.abs(nn @ light)
            img[py, px] = tex[v, u] * shade[:, None]
            zbuf[py, px] = zz
    return Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8))


if __name__ == "__main__":
    tris = load_mesh(sys.argv[1])
    render(tris, Image.open(sys.argv[2])).save(sys.argv[3])
