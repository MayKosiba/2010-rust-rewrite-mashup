#!/usr/bin/env bash
# Builds the CS2 first-person AK-47 | Case Hardened and karambit for the
# mashup from the local CS2 install, into iw4l-artifacts/cs2 (nothing from
# CS2 is committed; this regenerates it).
#
#   build.sh [seed]        seed: the AK's Case Hardened pattern (default 661)
#
# Uses Source2Viewer's CLI (ValveResourceFormat, MIT; fetched into
# ./source2viewer on first run) and python3 (a venv with numpy and pillow is
# made in ./.venv). Exports go to ./export. None of these are committed.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SEED="${1:-661}"
CS="${CS2_GAME:-$HOME/.local/share/Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo}"
S2V="${S2V:-$HERE/source2viewer/Source2Viewer-CLI}"
S2V_RELEASE="https://github.com/ValveResourceFormat/ValveResourceFormat/releases/download/20.0/cli-linux-x64.zip"
EXPORT="$HERE/export"
OUT="${IW4L_CS2_OUT:-$HOME/Games/iw4l/native/iw4l-artifacts/cs2}"
VPK="$CS/pak01_dir.vpk"

[ -f "$VPK" ] || { echo "CS2 not found at $CS (set CS2_GAME)"; exit 1; }
if [ ! -x "$S2V" ]; then
    echo "fetching Source2Viewer CLI"
    mkdir -p "$(dirname "$S2V")"
    curl -sL "$S2V_RELEASE" -o "$(dirname "$S2V")/cli.zip"
    unzip -o -q "$(dirname "$S2V")/cli.zip" -d "$(dirname "$S2V")"
    chmod +x "$S2V"
fi
if [ ! -x "$HERE/.venv/bin/python" ]; then
    python3 -m venv "$HERE/.venv"
    "$HERE/.venv/bin/pip" install -q numpy pillow
fi
PY="$HERE/.venv/bin/python"
mkdir -p "$EXPORT" "$OUT"

# Run from the export folder: Source2Viewer logs what it cannot read to
# ./exceptions.txt.
export_files() { (cd "$EXPORT" && "$S2V" -i "$VPK" --vpk_filepath "$1" -o "$EXPORT" -d "${@:2}" >/dev/null 2>&1) || true; }
dump_data() { (cd "$EXPORT" && "$S2V" -i "$VPK" --vpk_filepath "$1" -b DATA 2>/dev/null) | grep -v '^Preload\|^Added' > "$EXPORT/$2"; }

echo "exporting models, clips and textures from $CS"
export_files "weapons/models/shared/arms/weapon_arms.vmdl_c,weapons/models/knife/knife_karambit/weapon_knife_karambit.vmdl_c,weapons/models/ak47/weapon_rif_ak47.vmdl_c" \
    --gltf_export_format glb --gltf_export_materials --gltf_export_animations
export_files "animation/anims/viewmodel/knife/knife_karambit/,animation/anims/viewmodel/rifle/rifle_ak/" \
    --gltf_export_format glb --gltf_export_animations
export_files "materials/models/weapons/customization/paints/antiqued/oiled_psd_9f35e709.vtex_c,weapons/models/ak47/materials/"
dump_data "weapons/models/shared/arms/weapon_arms.vmdl_c" weapon_arms.data.txt
dump_data "weapons/models/ak47/weapon_rif_ak47.vmdl_c" weapon_rif_ak47.data.txt

echo "baking AK-47 | Case Hardened, seed $SEED"
"$PY" "$HERE/bake_case_hardened.py" "$EXPORT" "$EXPORT/weapons/models/ak47/ak47_case_hardened.png" "$SEED" 2048

K=animation/anims/viewmodel/knife/knife_karambit
A=animation/anims/viewmodel/rifle/rifle_ak
cat > "$EXPORT/spec.json" <<EOF
{"export": "$EXPORT", "out": "$OUT", "weapons": [
 {"name": "karambit", "reference_clip": "$K/idle1_karambit.glb",
  "models": [
   {"glb": "weapons/models/shared/arms/weapon_arms.glb", "meshes": ["unnamed"], "data": "weapon_arms.data.txt"},
   {"glb": "weapons/models/knife/knife_karambit/weapon_knife_karambit.glb", "meshes": ["body_legacy"]}],
  "clips": {"draw": "$K/draw_karambit.glb", "idle": "$K/idle1_karambit.glb", "idle2": "$K/idle2_karambit.glb",
            "light_hit1": "$K/light_hit1_karambit.glb", "light_hit2": "$K/light_hit2_karambit.glb",
            "light_miss1": "$K/light_miss1_karambit.glb", "light_miss2": "$K/light_miss2_karambit.glb",
            "heavy_hit": "$K/heavy_hit1_karambit.glb", "heavy_miss": "$K/heavy_miss1_karambit.glb",
            "backstab": "$K/light_backstab_karambit.glb", "inspect": "$K/lookat01_karambit.glb"}},
 {"name": "ak47", "reference_clip": "$A/idle_ak.glb", "texture_limit": 2048,
  "models": [
   {"glb": "weapons/models/shared/arms/weapon_arms.glb", "meshes": ["unnamed"], "data": "weapon_arms.data.txt"},
   {"glb": "weapons/models/ak47/weapon_rif_ak47.glb", "meshes": ["body_hd"], "data": "weapon_rif_ak47.data.txt",
    "textures": {"ak47_default_color_psd_5b66a23b.png": "$EXPORT/weapons/models/ak47/ak47_case_hardened.png"}}],
  "clips": {"draw": "$A/draw_ak.glb", "idle": "$A/idle_ak.glb", "shoot": "$A/shoot1_ak.glb", "reload": "$A/reload_ak.glb",
            "inspect": "$A/lookat01_ak.glb", "inspect2": "$A/lookat03_ak.glb"}}
]}
EOF
echo "converting"
"$PY" "$HERE/convert.py" "$EXPORT/spec.json"
echo "done: $OUT"
