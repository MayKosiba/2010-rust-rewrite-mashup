"""Converts Source2Viewer's glTF exports of CS2 first-person weapons into the
game's runtime files (iw4l-artifacts/cs2): one skeleton (the viewmodel arms'
bones and the weapon's, as the animation clips pose them together), skinned
meshes bound to it by bone name, and each clip sampled at 30 frames a second.

Source2Viewer leaves some models' joint tables out of the glTF (the arms,
the AK): their vertices' blend indices then go through the model's
`m_remappingTable` to its `m_modelSkeleton` bones, read from a dump of the
model's DATA block. Bones the clips' skeleton lacks (the arms' twist bones)
bind to their nearest ancestor that it has.
"""
import json
import math
import re
import struct
import sys
from pathlib import Path

import numpy as np

FPS = 30.0
COMPONENTS = {5120: np.int8, 5121: np.uint8, 5122: np.int16, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
WIDTH = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


class Glb:
    def __init__(self, path):
        self.path = Path(path)
        data = Path(path).read_bytes()
        length = struct.unpack("<I", data[12:16])[0]
        self.json = json.loads(data[20:20 + length])
        rest = data[20 + length:]
        self.bin = rest[8:8 + struct.unpack("<I", rest[:4])[0]] if rest else b""
        self.nodes = self.json["nodes"]
        self.parent = {c: i for i, n in enumerate(self.nodes) for c in n.get("children", [])}

    def accessor(self, index):
        acc = self.json["accessors"][index]
        view = self.json["bufferViews"][acc["bufferView"]]
        dtype = np.dtype(COMPONENTS[acc["componentType"]])
        width = WIDTH[acc["type"]]
        start = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
        stride = view.get("byteStride", 0) or dtype.itemsize * width
        count = acc["count"]
        raw = np.frombuffer(self.bin, dtype=np.uint8, count=stride * (count - 1) + dtype.itemsize * width, offset=start)
        rows = np.lib.stride_tricks.as_strided(raw, shape=(count, dtype.itemsize * width), strides=(stride, 1))
        out = np.frombuffer(rows.copy().tobytes(), dtype=dtype).reshape(count, width)
        if acc.get("normalized"):
            out = out.astype(np.float32) / np.iinfo(dtype).max
        return out

    def image_of(self, material_index, key="baseColorTexture"):
        mat = self.json["materials"][material_index]
        tex = mat.get("pbrMetallicRoughness", {}).get(key)
        if not tex:
            return None
        return self.json["images"][self.json["textures"][tex["index"]]["source"]].get("uri")


def kv3_array(text, name):
    """A top-level numeric or string array field of a KV3 text dump."""
    m = re.search(rf"\b{name} =\s*\[(.*?)\n\s*\]", text, re.S)
    if not m:
        return None
    body = m.group(1)
    strings = re.findall(r'"([^"]*)"', body)
    if strings:
        return strings
    return [float(x) for x in re.findall(r"-?\d+(?:\.\d+)?(?:e-?\d+)?", body)]


def kv3_rows(text, name):
    """A top-level array of numeric arrays (`m_bonePosParent`)."""
    m = re.search(rf"\b{name} =\s*\[(.*?)\n\t\t\]", text, re.S)
    return [[float(x) for x in row.split(",")] for row in re.findall(r"\[([^\[\]]*)\]", m.group(1))]


# Source's model space (inches, z up) as Source2Viewer's glTF has it
# (metres, y up): glTF = (y, z, x) * 0.0254.
SOURCE_AXES = np.array([[0, 1, 0], [0, 0, 1], [1, 0, 0]], dtype=np.float64)
INCH = 0.0254
# The inverse of the root bones' turn, (-0.5, -0.5, -0.5, 0.5) (x, y, z, w).
TO_SOURCE_AXES = [0.5, 0.5, 0.5, 0.5]


def model_skeleton(data_dump):
    """A model's own bones from a dump of its DATA block: names, parents and
    each bone's rest pose in the glTF export's space. Source2Viewer writes
    blend indices as indices into these bones (already through the model's
    remapping table)."""
    text = Path(data_dump).read_text()
    names = kv3_array(text, "m_boneName")
    parents = [int(p) for p in kv3_array(text, "m_nParent")]
    positions = kv3_rows(text, "m_bonePosParent")
    rotations = kv3_rows(text, "m_boneRotParent")
    world = []
    for i in range(len(names)):
        local = np.eye(4)
        local[:3, :3] = quat_matrix(rotations[i])
        local[:3, 3] = positions[i]
        world.append(local if parents[i] < 0 else world[parents[i]] @ local)
    # Only the world side changes axes: each bone keeps Source's local frame
    # (bones run along their x), as the clips' skeleton has it.
    rest = []
    for w in world:
        g = np.eye(4)
        g[:3, :3] = SOURCE_AXES @ w[:3, :3]
        g[:3, 3] = INCH * (SOURCE_AXES @ w[:3, 3])
        rest.append(g)
    return names, parents, rest


def quat_matrix(q):
    x, y, z, w = q
    return np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])


class Skeleton:
    """Every node of a clip's glTF under its skeletons, with rest pose and
    inverse bind matrices."""

    def __init__(self, clip):
        self.names, self.parents, self.rest, self.ibm = [], [], [], []
        index = {}
        order = []

        def visit(i):
            order.append(i)
            for c in clip.nodes[i].get("children", []):
                visit(c)

        for root in clip.json["scenes"][0]["nodes"]:
            if "mesh" not in clip.nodes[root]:
                visit(root)
        ibms = {}
        for skin in clip.json.get("skins", []):
            mats = clip.accessor(skin["inverseBindMatrices"])
            for j, node in enumerate(skin["joints"]):
                ibms[clip.nodes[node]["name"]] = mats[j].tolist()
        for i in order:
            node = clip.nodes[i]
            index[i] = len(self.names)
            self.names.append(node["name"])
            parent = index.get(clip.parent.get(i), -1)
            # A weapon's skeleton is a root of its own in the clips; its pose
            # is relative to the arms' `wpn` bone, which carries it.
            rest = node.get("translation", [0, 0, 0]) + node.get("rotation", [0, 0, 0, 1]) + node.get("scale", [1, 1, 1])
            if parent < 0 and "skeletons/weapons/" in node["name"] and "wpn" in self.names:
                parent = self.names.index("wpn")
                # Each skeleton's root bone carries the export's turn from
                # Source's axes to glTF's; under `wpn` (already turned) the
                # weapon's must be turned back.
                rest = [0, 0, 0] + TO_SOURCE_AXES + [1, 1, 1]
            self.parents.append(parent)
            self.rest.append(rest)
            self.ibm.append(ibms.get(node["name"], np.eye(4).flatten(order="F").tolist()))
        self.index = {n: i for i, n in enumerate(self.names)}

    def to_json(self):
        return {"names": self.names, "parents": self.parents, "rest": self.rest, "inverse_bind": self.ibm}


def convert_mesh(glb, mesh_filter, skeleton, texture_names, data_dump=None):
    """The model's primitives in `mesh_filter` meshes, bound by bone name."""
    blend_names = parents_by_name = None
    if data_dump:
        blend_names, model_parents, model_rest = model_skeleton(data_dump)
        parents_by_name = {n: (blend_names[p] if p >= 0 else None) for n, p in zip(blend_names, model_parents)}
        # The mesh is in the model's own rest pose, not the clips': its bones
        # bind from there.
        for name, rest in zip(blend_names, model_rest):
            if name in skeleton.index:
                skeleton.ibm[skeleton.index[name]] = np.linalg.inv(rest).flatten(order="F").tolist()

    def resolve(name):
        # A bone the clips do not pose follows its nearest posed ancestor,
        # bound where that ancestor rests.
        while name is not None and name not in skeleton.index:
            name = parents_by_name.get(name) if parents_by_name else None
        return skeleton.index.get(name, 0)

    surfaces = []
    for node_index, node in enumerate(glb.nodes):
        if "mesh" not in node:
            continue
        mesh = glb.json["meshes"][node["mesh"]]
        if not any(f in mesh.get("name", "") for f in mesh_filter):
            continue
        skin = glb.json["skins"][node["skin"]] if "skin" in node else None
        for prim in mesh["primitives"]:
            attrs = prim["attributes"]
            joints = glb.accessor(attrs["JOINTS_0"]).astype(np.int64)
            weights = glb.accessor(attrs["WEIGHTS_0"]).astype(np.float32)
            if skin is not None:
                names = [glb.nodes[j]["name"] for j in skin["joints"]]
                table = [resolve(n) for n in names]
            else:
                table = [resolve(n) for n in blend_names]
            bones = np.vectorize(lambda j: table[j] if j < len(table) else 0)(joints)
            total = weights.sum(axis=1, keepdims=True)
            weights = np.where(total > 0, weights / np.maximum(total, 1e-6), np.array([1, 0, 0, 0], np.float32))
            texture = glb.image_of(prim["material"]) if "material" in prim else None
            material = glb.json["materials"][prim["material"]].get("name", "") if "material" in prim else ""
            if texture is None or "sticker" in material:
                continue
            texture = texture_names.get(texture, texture)
            source = Path(texture) if Path(texture).is_absolute() else Path(glb.path).parent / texture
            surfaces.append({
                "material": glb.json["materials"][prim["material"]].get("name") if "material" in prim else None,
                "texture": str(source),
                "positions": np.round(glb.accessor(attrs["POSITION"]), 6).flatten().tolist(),
                "normals": np.round(glb.accessor(attrs["NORMAL"]), 4).flatten().tolist(),
                "uvs": np.round(glb.accessor(attrs["TEXCOORD_0"]), 6).flatten().tolist(),
                "bones": bones.flatten().tolist(),
                "weights": np.round(weights, 4).flatten().tolist(),
                "indices": glb.accessor(prim["indices"]).flatten().tolist(),
            })
    return surfaces


def slerp(a, b, t):
    a, b = np.asarray(a, float), np.asarray(b, float)
    d = float(np.dot(a, b))
    if d < 0:
        b, d = -b, -d
    if d > 0.9995:
        q = a + (b - a) * t
    else:
        th = math.acos(d)
        q = (math.sin((1 - t) * th) * a + math.sin(t * th) * b) / math.sin(th)
    return q / np.linalg.norm(q)


def sample(times, values, t, path):
    if len(times) == 1 or t <= times[0]:
        return values[0]
    if t >= times[-1]:
        return values[-1]
    k = int(np.searchsorted(times, t)) - 1
    f = (t - times[k]) / (times[k + 1] - times[k])
    if path == "rotation":
        return slerp(values[k], values[k + 1], f)
    return values[k] + (values[k + 1] - values[k]) * f


def convert_clip(path, skeleton):
    clip = Glb(path)
    anim = clip.json["animations"][0]
    tracks = {}
    duration = 0.0
    for ch in anim["channels"]:
        sampler = anim["samplers"][ch["sampler"]]
        times = clip.accessor(sampler["input"]).flatten()
        values = clip.accessor(sampler["output"])
        duration = max(duration, float(times[-1]))
        name = clip.nodes[ch["target"]["node"]]["name"]
        if name in skeleton.index:
            tracks.setdefault(name, {})[ch["target"]["path"]] = (times, values)
    frames = max(1, int(round(duration * FPS)) + 1)
    bones = {}
    for name, paths in tracks.items():
        rest = skeleton.rest[skeleton.index[name]]
        out = []
        for f in range(frames):
            t = f / FPS
            tr = sample(*paths["translation"], t, "translation") if "translation" in paths else rest[0:3]
            ro = sample(*paths["rotation"], t, "rotation") if "rotation" in paths else rest[3:7]
            sc = sample(*paths["scale"], t, "scale") if "scale" in paths else rest[7:10]
            out.append([round(float(x), 6) for x in list(tr) + list(ro) + list(sc)])
        bones[name] = out
    return {"fps": FPS, "frames": frames, "bones": bones}


ATLAS_CELL = 1024
ATLAS_SIZE = (4096, 2048)


def pack_atlas(documents, out):
    """Every surface texture in one atlas on a grid of 1024 cells: weapon
    textures up to their weapon's `texture_limit`, the arms' and gloves' up
    to 1024. Rewrites
    each surface's UVs into its texture's place and drops its texture."""
    from PIL import Image

    arms = ("bare_arm", "glove")
    wanted = {}
    for doc in documents:
        for surface in doc["surfaces"]:
            limit = ATLAS_CELL if any(a in (surface["material"] or "") for a in arms) else surface.pop("texture_limit")
            wanted[surface["texture"]] = min(wanted.get(surface["texture"], limit), limit)
    cols, rows = ATLAS_SIZE[0] // ATLAS_CELL, ATLAS_SIZE[1] // ATLAS_CELL
    used = [[False] * cols for _ in range(rows)]
    atlas = Image.new("RGBA", ATLAS_SIZE, (0, 0, 0, 255))
    regions = {}
    for path, limit in sorted(wanted.items(), key=lambda kv: -kv[1]):
        image = Image.open(path).convert("RGB")
        scale = min(1.0, limit / max(image.size))
        w, h = max(1, round(image.width * scale)), max(1, round(image.height * scale))
        cw, ch = -(-w // ATLAS_CELL), -(-h // ATLAS_CELL)
        spot = next(((r, c) for r in range(rows - ch + 1) for c in range(cols - cw + 1)
                     if all(not used[r + i][c + j] for i in range(ch) for j in range(cw))), None)
        if spot is None:
            raise SystemExit(f"atlas full at {path}")
        r, c = spot
        for i in range(ch):
            for j in range(cw):
                used[r + i][c + j] = True
        x, y = c * ATLAS_CELL, r * ATLAS_CELL
        atlas.paste(image.resize((w, h), Image.LANCZOS).convert("RGBA"), (x, y))
        # Half a texel in from the edges, so filtering stays inside.
        regions[path] = ((x + 0.5) / ATLAS_SIZE[0], (y + 0.5) / ATLAS_SIZE[1],
                         (w - 1.0) / ATLAS_SIZE[0], (h - 1.0) / ATLAS_SIZE[1])
    for doc in documents:
        for surface in doc["surfaces"]:
            x, y, w, h = regions[surface.pop("texture")]
            uv = np.array(surface["uvs"]).reshape(-1, 2)
            uv = np.clip(uv, 0.0, 1.0) * np.array([w, h]) + np.array([x, y])
            surface["uvs"] = np.round(uv, 6).flatten().tolist()
    atlas.save(out)
    print(f"{out}: {len(regions)} textures")


def main(spec_path):
    """`spec` names the reference clip, the models and the clips of one
    weapon (see weapons.json)."""
    spec = json.loads(Path(spec_path).read_text())
    root = Path(spec["export"])
    out = Path(spec["out"])
    out.mkdir(parents=True, exist_ok=True)
    documents = []
    for weapon in spec["weapons"]:
        skeleton = Skeleton(Glb(root / weapon["reference_clip"]))
        surfaces = []
        for model in weapon["models"]:
            converted = convert_mesh(Glb(root / model["glb"]), model["meshes"], skeleton, model.get("textures", {}),
                                     root / model["data"] if model.get("data") else None)
            for surface in converted:
                surface["texture_limit"] = weapon.get("texture_limit", ATLAS_CELL)
            surfaces += converted
        clips = {}
        for name, clip in weapon["clips"].items():
            clips[name] = convert_clip(root / clip, skeleton)
        documents.append((weapon["name"], {"skeleton": skeleton.to_json(), "surfaces": surfaces, "clips": clips}))
    pack_atlas([doc for _, doc in documents], out / "atlas.png")
    for name, doc in documents:
        path = out / f"{name}.json"
        path.write_text(json.dumps(doc, separators=(",", ":")))
        print(f"{path}: {len(doc['skeleton']['names'])} bones, {len(doc['surfaces'])} surfaces, "
              f"{sum(len(s['indices']) // 3 for s in doc['surfaces'])} triangles, clips {list(doc['clips'])}")


if __name__ == "__main__":
    main(sys.argv[1])
