"""Bakes an AK-47 | Case Hardened colour texture for one paint seed.

CS2 composites the finish at runtime (`csgo_customweapon`, paint style
antiqued, from weapons/paints/legacy/aq_oiled): the pattern texture laid
over the weapon's UVs at the seed's offset and rotation, scaled by the
weapon's UV scale, where the weapon's paint mask allows it. The placement is
Valve's (pattern_seed.py, checked against published seeds) with
pattern.wiki's template transform; the shading of the paint over the base is
an approximation of the shader, which is not readable.

usage: bake_case_hardened.py <exported cs2 dir> <out.png> [seed] [size]
"""
import math
import sys

import numpy as np
from PIL import Image

from pattern_seed import placement

# Case Hardened (aq_oiled) is a legacy paint kit (items_game:
# use_legacy_model 1): it goes on the AK's CS:GO body with that body's UVs
# and composite inputs (materials/models/weapons/customization/rif_ak47).
AK_UV_SCALE = 0.549  # rif_ak47_composite_inputs: g_flUvScale1
EXTRA = "wiki"  # csgo_customweapon reuses extra_x likewise


def load(path, size, mode="RGBA"):
    # Channels resize apart: an alpha that carries data (not coverage) must
    # not blank the colour where it is zero.
    image = Image.open(path)
    bands = [b.resize((size, size), Image.LANCZOS) for b in image.convert("RGBA").split()]
    if mode == "L":
        bands = [Image.open(path).convert("L").resize((size, size), Image.LANCZOS)]
    elif mode == "RGB":
        bands = bands[:3]
    return np.stack([np.asarray(b, dtype=np.float32) / 255.0 for b in bands], axis=-1).squeeze(-1 if mode == "L" else None) if mode == "L" else np.stack([np.asarray(b, dtype=np.float32) / 255.0 for b in bands], axis=-1)


def pattern_coords(size, seed, scale, flip_u=False, flip_v=False, rot_sign=1.0):
    """Where each texel of the weapon's UV square falls on the pattern."""
    off_x, off_y, rotation = placement(seed)
    # csgo_customweapon's g_vPatternTexCoordXform: each input rounded to
    # hundredths, the scale being the pattern's times the weapon's UV scale.
    hundredths = lambda x: math.floor((x + 0.005) * 100) / 100
    off_x, off_y, rotation, scale = hundredths(off_x), hundredths(off_y), hundredths(rotation), hundredths(scale)
    rotation *= rot_sign
    # pattern.wiki's transform: translate(off - 0.5) scale(s) rotate(r)
    # translate(extra), applied right to left to a template point.
    inv = 0.5 / scale
    rad = -rotation * math.pi / 180.0
    c, s = math.cos(rad), math.sin(rad)
    extra_x = inv * c - inv * s
    # pattern.wiki's formula reuses extra_x (it matches their example); the
    # clean rotation of (inv, inv) uses inv.
    extra_y = (extra_x if EXTRA == "wiki" else inv) * s + inv * c
    v, u = np.mgrid[0:size, 0:size].astype(np.float64)
    u = (u + 0.5) / size + extra_x
    v = (v + 0.5) / size + extra_y
    r = rotation * math.pi / 180.0
    cr, sr = math.cos(r), math.sin(r)
    pu = (u * cr - v * sr) * scale + (off_x - 0.5)
    pv = (u * sr + v * cr) * scale + (off_y - 0.5)
    pu, pv = np.mod(pu, 1.0), np.mod(pv, 1.0)
    return (1.0 - pu if flip_u else pu), (1.0 - pv if flip_v else pv)


def sample(image, pu, pv):
    h, w = image.shape[:2]
    x = np.clip((pu * w).astype(np.int64), 0, w - 1)
    y = np.clip((pv * h).astype(np.int64), 0, h - 1)
    return image[y, x]


def srgb_to_linear(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(c):
    c = np.clip(c, 0.0, 1.0)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)


BRIGHTNESS = 1.8  # aq_oiled: g_flColorBrightness


def bake(root, out, seed=661, size=2048, variant=(False, False, 1.0)):
    ak = f"{root}/weapons/models/ak47"
    legacy = f"{root}/materials/models/weapons/customization/rif_ak47"
    base = load(f"{ak}/ak47_color_psd_1f318532.png", size)
    ao = load(f"{legacy}/rif_ak47_ao_psd_3cdda94d.png", size, "L")
    masks = load(f"{legacy}/rif_ak47_masks_psd_cc08789a.png", size)
    pattern = load(f"{root}/materials/models/weapons/customization/paints/antiqued/oiled_psd_9f35e709.png", 2048, "RGB")
    pu, pv = pattern_coords(size, seed, AK_UV_SCALE, *variant)
    paint = sample(pattern, pu, pv)
    # The finish brightened in linear light (g_flColorBrightness), then
    # shaded by the weapon's ambient occlusion.
    paint = linear_to_srgb(srgb_to_linear(paint) * BRIGHTNESS * ao[..., None])
    painted = np.clip(paint, 0.0, 1.0)
    paintable = masks[..., 0:1]
    rgb = base[..., :3] * (1.0 - paintable) + painted * paintable
    result = np.concatenate([rgb, base[..., 3:4]], axis=-1)
    Image.fromarray((result * 255.0 + 0.5).astype(np.uint8), "RGBA").save(out)


if __name__ == "__main__":
    root, out = sys.argv[1], sys.argv[2]
    seed = int(sys.argv[3]) if len(sys.argv) > 3 else 661
    size = int(sys.argv[4]) if len(sys.argv) > 4 else 2048
    bake(root, out, seed, size)
