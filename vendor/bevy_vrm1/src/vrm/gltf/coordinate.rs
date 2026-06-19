//! VRM 0.0 support (Ferra-VRM addition; not in upstream bevy_vrm1 0.7.1).
//!
//! VRM 0.0 stores its data under the legacy `VRM` glTF extension (Unity-era), with a different
//! shape than VRM 1.0's `VRMC_vrm`, and is authored in Unity's LEFT-handed coordinate system.
//! `convert_vrm0_glb` runs on the raw `.glb` bytes BEFORE Bevy's `GltfLoader` builds the meshes:
//!
//!   Stage 1: migrate the legacy `VRM` extension → `VRMC_vrm` so init fires + expressions work
//!   (humanoid `humanBones` array→map; `blendShapeMaster`→`expressions` with the VRM-0.0
//!   `mesh`-index binds remapped to VRM-1.0 `node`-index).
//!
//!   Stage 2: negate the X axis across all geometry + transforms. VRM 0.0 is authored in Unity's
//!   LEFT-handed space; loaded as-is it is X-mirrored — invisible at rest (symmetric T-pose) but
//!   every asymmetric VRMA plays back mirrored (arms/legs wrong). UniVRM's `ConvertCoordinate`
//!   negates X across all model data; we do the same. Bails gracefully (model renders, just
//!   mirrored) if any accessor is un-flippable (sparse / non-float / external / compressed).
//!
//! Returns `None` (→ load unchanged) for anything that isn't a migratable VRM 0.0 `.glb`.

use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

const GLB_MAGIC: &[u8; 4] = b"glTF";
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

/// Convert a VRM 0.0 `.glb` to a VRM-1.0-compatible one. `None` = not a VRM 0.0 glb / leave as-is.
pub fn convert_vrm0_glb(bytes: &[u8]) -> Option<Vec<u8>> {
    let (json_bytes, bin) = parse_glb(bytes)?;
    let mut json: Value = serde_json::from_slice(&json_bytes).ok()?;

    let ext = json.get("extensions")?.as_object()?;
    // Only act on VRM 0.0: has the legacy `VRM` key and not already `VRMC_vrm`.
    if ext.contains_key("VRMC_vrm") || !ext.contains_key("VRM") {
        return None;
    }
    let vrm0 = ext.get("VRM")?.clone();

    let human_bones = migrate_human_bones(&vrm0)?;
    let preset = migrate_expressions(&vrm0, &json);

    let vrmc_vrm = json!({
        "specVersion": "0.0",
        "humanoid": { "humanBones": human_bones },
        "expressions": { "preset": preset },
    });

    json.get_mut("extensions")?
        .as_object_mut()?
        .insert("VRMC_vrm".to_string(), vrmc_vrm);
    if let Some(used) = json.get_mut("extensionsUsed").and_then(|u| u.as_array_mut()) {
        if !used.iter().any(|v| v.as_str() == Some("VRMC_vrm")) {
            used.push(json!("VRMC_vrm"));
        }
    }

    // Stage 2: negate the X axis across geometry + transforms so right-handed VRMA animations
    // don't play back mirrored. Operates on the embedded BIN; if any accessor is un-flippable the
    // whole pass bails (leaving the model mirrored but still rendering) — never a partial flip.
    let bin = match bin {
        Some(mut b) => {
            negate_x_geometry(&mut json, &mut b);
            Some(b)
        }
        None => None,
    };
    // NB: after the X-flip the model faces −Z. The 180° Y facing correction is applied as a runtime
    // root transform on the avatar entity (see `spawn_vrm`), NOT baked here — baking it into the
    // scene roots would make VRMA retargeting read it as rest pose and mangle the animation.

    let new_json = serde_json::to_vec(&json).ok()?;
    Some(build_glb(&new_json, bin.as_deref()))
}

/// Byte layout of a float accessor's data in the BIN buffer: `(base, stride, count)`.
type FloatAccessor = (usize, usize, usize);
/// Byte layout of an index accessor: `(base, component_type, count)`.
type IndexAccessor = (usize, u32, usize);

/// Everything that must be negated/reversed for an X-flip, resolved to concrete BIN byte offsets.
struct FlipPlan {
    /// VEC3 float accessors (POSITION, NORMAL, morph deltas) — negate component 0 (x).
    vec3: Vec<FloatAccessor>,
    /// VEC4 float accessors (TANGENT) — negate component 0 (x) and 3 (w, the handedness sign).
    tangent: Vec<FloatAccessor>,
    /// MAT4 float accessors (inverseBindMatrices) — negate col-major flat indices 1,2,3,4,8,12.
    ibm: Vec<FloatAccessor>,
    /// Triangle index accessors — reverse each triangle's winding (swap 2nd & 3rd index).
    indices: Vec<IndexAccessor>,
    /// POSITION accessor indices — swap+negate their `min.x`/`max.x`.
    positions: Vec<usize>,
}

/// Negate the X axis across all geometry + transforms, in place. No-op if anything is un-flippable
/// (the model is left mirrored but still renders).
fn negate_x_geometry(json: &mut Value, bin: &mut [u8]) {
    let Some(plan) = build_flip_plan(json, bin.len()) else {
        return;
    };
    for &(base, stride, count) in &plan.vec3 {
        for i in 0..count {
            negate_f32(bin, base + i * stride);
        }
    }
    for &(base, stride, count) in &plan.tangent {
        for i in 0..count {
            negate_f32(bin, base + i * stride); // x
            negate_f32(bin, base + i * stride + 12); // w
        }
    }
    for &(base, stride, count) in &plan.ibm {
        for i in 0..count {
            let e = base + i * stride;
            for idx in [1usize, 2, 3, 4, 8, 12] {
                negate_f32(bin, e + idx * 4);
            }
        }
    }
    for &(base, ctype, count) in &plan.indices {
        reverse_winding(bin, base, ctype, count);
    }
    negate_nodes(json);
    negate_position_minmax(json, &plan.positions);
}

/// Resolve every accessor that needs flipping to BIN byte offsets, validating each. Returns `None`
/// (→ skip the whole flip) if any required accessor is sparse / non-float / external / out of bounds
/// / a compressed (no-bufferView) primitive — so the result is never a partial, broken flip.
fn build_flip_plan(json: &Value, bin_len: usize) -> Option<FlipPlan> {
    let accs = json.get("accessors").and_then(Value::as_array)?;
    let bvs = json.get("bufferViews").and_then(Value::as_array)?;
    let meshes = json.get("meshes").and_then(Value::as_array)?;

    let mut vec3_acc = HashSet::new();
    let mut tan_acc = HashSet::new();
    let mut idx_acc = HashSet::new();
    let mut pos_acc = HashSet::new();
    let mut ibm_acc = HashSet::new();

    for mesh in meshes {
        let Some(prims) = mesh.get("primitives").and_then(Value::as_array) else {
            continue;
        };
        for prim in prims {
            if let Some(attrs) = prim.get("attributes") {
                if let Some(p) = attrs.get("POSITION").and_then(Value::as_u64) {
                    vec3_acc.insert(p);
                    pos_acc.insert(p);
                }
                if let Some(n) = attrs.get("NORMAL").and_then(Value::as_u64) {
                    vec3_acc.insert(n);
                }
                if let Some(t) = attrs.get("TANGENT").and_then(Value::as_u64) {
                    tan_acc.insert(t);
                }
            }
            if let Some(targets) = prim.get("targets").and_then(Value::as_array) {
                for tg in targets {
                    if let Some(p) = tg.get("POSITION").and_then(Value::as_u64) {
                        vec3_acc.insert(p);
                    }
                    if let Some(n) = tg.get("NORMAL").and_then(Value::as_u64) {
                        vec3_acc.insert(n);
                    }
                }
            }
            // Default primitive mode is 4 (TRIANGLES); only triangles need winding reversal.
            if prim.get("mode").and_then(Value::as_u64).unwrap_or(4) == 4 {
                if let Some(i) = prim.get("indices").and_then(Value::as_u64) {
                    idx_acc.insert(i);
                }
            }
        }
    }
    if let Some(skins) = json.get("skins").and_then(Value::as_array) {
        for s in skins {
            if let Some(i) = s.get("inverseBindMatrices").and_then(Value::as_u64) {
                ibm_acc.insert(i);
            }
        }
    }

    let mut vec3 = Vec::new();
    for a in &vec3_acc {
        vec3.push(float_accessor(accs.get(*a as usize)?, bvs, "VEC3", bin_len)?);
    }
    let mut tangent = Vec::new();
    for a in &tan_acc {
        tangent.push(float_accessor(accs.get(*a as usize)?, bvs, "VEC4", bin_len)?);
    }
    let mut ibm = Vec::new();
    for a in &ibm_acc {
        ibm.push(float_accessor(accs.get(*a as usize)?, bvs, "MAT4", bin_len)?);
    }
    let mut indices = Vec::new();
    for a in &idx_acc {
        indices.push(index_accessor(accs.get(*a as usize)?, bvs, bin_len)?);
    }
    let positions = pos_acc.iter().map(|p| *p as usize).collect();

    Some(FlipPlan {
        vec3,
        tangent,
        ibm,
        indices,
        positions,
    })
}

/// Resolve a float accessor of the expected `type` to `(base, stride, count)`. `None` if it's sparse,
/// the wrong type, not FLOAT (5126), references a non-embedded buffer, or runs past the BIN end.
fn float_accessor(acc: &Value, bvs: &[Value], want_type: &str, bin_len: usize) -> Option<FloatAccessor> {
    if acc.get("sparse").is_some() {
        return None;
    }
    if acc.get("type").and_then(Value::as_str)? != want_type {
        return None;
    }
    if acc.get("componentType").and_then(Value::as_u64)? != 5126 {
        return None;
    }
    let ncomp = match want_type {
        "VEC3" => 3,
        "VEC4" => 4,
        "MAT4" => 16,
        _ => return None,
    };
    let bv = bvs.get(acc.get("bufferView").and_then(Value::as_u64)? as usize)?;
    if bv.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
        return None; // only the embedded BIN (buffer 0) is editable
    }
    let count = acc.get("count").and_then(Value::as_u64)? as usize;
    let stride = bv
        .get("byteStride")
        .and_then(Value::as_u64)
        .map(|s| s as usize)
        .unwrap_or(4 * ncomp);
    let base = bv.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize
        + acc.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    if count > 0 && base + stride * (count - 1) + 4 * ncomp > bin_len {
        return None;
    }
    Some((base, stride, count))
}

/// Resolve a SCALAR index accessor to `(base, component_type, count)`. `None` if sparse, not SCALAR,
/// not an 8/16/32-bit unsigned int, external, or out of bounds.
fn index_accessor(acc: &Value, bvs: &[Value], bin_len: usize) -> Option<IndexAccessor> {
    if acc.get("sparse").is_some() {
        return None;
    }
    if acc.get("type").and_then(Value::as_str)? != "SCALAR" {
        return None;
    }
    let ctype = acc.get("componentType").and_then(Value::as_u64)? as u32;
    let csize = match ctype {
        5121 => 1,
        5123 => 2,
        5125 => 4,
        _ => return None,
    };
    let bv = bvs.get(acc.get("bufferView").and_then(Value::as_u64)? as usize)?;
    if bv.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
        return None;
    }
    let count = acc.get("count").and_then(Value::as_u64)? as usize;
    let base = bv.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize
        + acc.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    if base + csize * count > bin_len {
        return None;
    }
    Some((base, ctype, count))
}

/// Reverse each triangle's winding (swap the 2nd & 3rd index) so faces aren't culled after the X-flip.
fn reverse_winding(bin: &mut [u8], base: usize, ctype: u32, count: usize) {
    let cs = match ctype {
        5121 => 1,
        5123 => 2,
        5125 => 4,
        _ => return,
    };
    for t in 0..count / 3 {
        let a = base + (3 * t + 1) * cs;
        let b = base + (3 * t + 2) * cs;
        if b + cs <= bin.len() {
            for k in 0..cs {
                bin.swap(a + k, b + k);
            }
        }
    }
}

/// Negate X on every node transform: translation.x, rotation → (x,−y,−z,w), matrix flat 1,2,3,4,8,12.
fn negate_nodes(json: &mut Value) {
    let Some(nodes) = json.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    for node in nodes {
        if let Some(m) = node.get_mut("matrix").and_then(Value::as_array_mut) {
            for idx in [1usize, 2, 3, 4, 8, 12] {
                if let Some(v) = m.get(idx).and_then(Value::as_f64) {
                    m[idx] = json!(-v);
                }
            }
            continue; // TRS is ignored when a matrix is present
        }
        if let Some(t) = node.get_mut("translation").and_then(Value::as_array_mut) {
            if let Some(x) = t.first().and_then(Value::as_f64) {
                t[0] = json!(-x);
            }
        }
        if let Some(r) = node.get_mut("rotation").and_then(Value::as_array_mut) {
            for idx in [1usize, 2] {
                if let Some(v) = r.get(idx).and_then(Value::as_f64) {
                    r[idx] = json!(-v);
                }
            }
        }
    }
}

/// Negating X flips each POSITION accessor's bounds: `min.x = −old max.x`, `max.x = −old min.x`.
fn negate_position_minmax(json: &mut Value, positions: &[usize]) {
    let Some(accs) = json.get_mut("accessors").and_then(Value::as_array_mut) else {
        return;
    };
    for &p in positions {
        let Some(acc) = accs.get_mut(p) else { continue };
        let min0 = acc.get("min").and_then(|m| m.get(0)).and_then(Value::as_f64);
        let max0 = acc.get("max").and_then(|m| m.get(0)).and_then(Value::as_f64);
        if let (Some(mn), Some(mx)) = (min0, max0) {
            if let Some(m) = acc.get_mut("min").and_then(Value::as_array_mut) {
                m[0] = json!(-mx);
            }
            if let Some(m) = acc.get_mut("max").and_then(Value::as_array_mut) {
                m[0] = json!(-mn);
            }
        }
    }
}

fn read_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn negate_f32(b: &mut [u8], o: usize) {
    if o + 4 <= b.len() {
        let v = read_f32(b, o);
        b[o..o + 4].copy_from_slice(&(-v).to_le_bytes());
    }
}

/// VRM 0.0 `humanoid.humanBones: [{bone, node}]` → VRM 1.0 `{ <bone>: { node } }`.
fn migrate_human_bones(vrm0: &Value) -> Option<Map<String, Value>> {
    let arr = vrm0
        .get("humanoid")?
        .get("humanBones")?
        .as_array()?;
    let mut bones = Map::new();
    for hb in arr {
        if let (Some(bone), Some(node)) = (
            hb.get("bone").and_then(Value::as_str),
            hb.get("node").and_then(Value::as_u64),
        ) {
            bones.insert(bone.to_string(), json!({ "node": node }));
        }
    }
    (!bones.is_empty()).then_some(bones)
}

/// VRM 0.0 `blendShapeMaster.blendShapeGroups` → VRM 1.0 `expressions.preset`.
/// VRM 0.0 binds reference a `mesh` index; VRM 1.0 `morphTargetBind` references a `node` index.
fn migrate_expressions(vrm0: &Value, gltf: &Value) -> Map<String, Value> {
    let mesh_to_node = build_mesh_to_node(gltf);
    let mut preset = Map::new();
    let Some(groups) = vrm0
        .get("blendShapeMaster")
        .and_then(|b| b.get("blendShapeGroups"))
        .and_then(Value::as_array)
    else {
        return preset;
    };
    for g in groups {
        let key = map_preset_name(g.get("presetName").and_then(Value::as_str).unwrap_or(""));
        if key.is_empty() {
            continue;
        }
        let mut binds = Vec::new();
        if let Some(bs) = g.get("binds").and_then(Value::as_array) {
            for b in bs {
                let mesh = b.get("mesh").and_then(Value::as_u64);
                let index = b.get("index").and_then(Value::as_u64);
                let weight = b.get("weight").and_then(Value::as_f64).unwrap_or(100.0);
                if let (Some(mesh), Some(index)) = (mesh, index) {
                    if let Some(&node) = mesh_to_node.get(&(mesh as usize)) {
                        binds.push(json!({ "index": index, "node": node, "weight": weight / 100.0 }));
                    }
                }
            }
        }
        preset.insert(
            key.to_string(),
            json!({
                "isBinary": g.get("isBinary").and_then(Value::as_bool).unwrap_or(false),
                "morphTargetBinds": binds,
                "overrideBlink": "none",
                "overrideLookAt": "none",
                "overrideMouth": "none",
            }),
        );
    }
    preset
}

/// First node referencing each mesh index (VRM 0.0 binds → VRM 1.0 node binds).
fn build_mesh_to_node(gltf: &Value) -> HashMap<usize, usize> {
    let mut map = HashMap::new();
    if let Some(nodes) = gltf.get("nodes").and_then(Value::as_array) {
        for (i, node) in nodes.iter().enumerate() {
            if let Some(mesh) = node.get("mesh").and_then(Value::as_u64) {
                map.entry(mesh as usize).or_insert(i);
            }
        }
    }
    map
}

/// VRM 0.0 preset name → VRM 1.0 expression key. Empty = drop (unknown/custom).
fn map_preset_name(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "a" => "aa",
        "i" => "ih",
        "u" => "ou",
        "e" => "ee",
        "o" => "oh",
        "blink" => "blink",
        "blink_l" => "blinkLeft",
        "blink_r" => "blinkRight",
        "joy" => "happy",
        "angry" => "angry",
        "sorrow" => "sad",
        "fun" => "relaxed",
        "lookup" => "lookUp",
        "lookdown" => "lookDown",
        "lookleft" => "lookLeft",
        "lookright" => "lookRight",
        "neutral" => "neutral",
        _ => "",
    }
}

/// Parse a binary glTF into (JSON chunk bytes, optional BIN chunk bytes).
fn parse_glb(bytes: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    if bytes.len() < 12 || &bytes[0..4] != GLB_MAGIC {
        return None;
    }
    let mut off = 12;
    let mut json = None;
    let mut bin = None;
    while off + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[off..off + 4].try_into().ok()?) as usize;
        let ty = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().ok()?);
        let start = off + 8;
        let end = start.checked_add(len)?;
        if end > bytes.len() {
            return None;
        }
        match ty {
            CHUNK_JSON => json = Some(bytes[start..end].to_vec()),
            CHUNK_BIN => bin = Some(bytes[start..end].to_vec()),
            _ => {}
        }
        off = end;
    }
    Some((json?, bin))
}

/// Re-emit a binary glTF from a (modified) JSON chunk and the original BIN chunk.
fn build_glb(json: &[u8], bin: Option<&[u8]>) -> Vec<u8> {
    let mut json_chunk = json.to_vec();
    while json_chunk.len() % 4 != 0 {
        json_chunk.push(b' '); // JSON chunks pad with spaces
    }
    let bin_chunk = bin.map(|b| {
        let mut v = b.to_vec();
        while v.len() % 4 != 0 {
            v.push(0); // BIN chunks pad with zeros
        }
        v
    });

    let mut total = 12 + 8 + json_chunk.len();
    if let Some(b) = &bin_chunk {
        total += 8 + b.len();
    }

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(GLB_MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json_chunk);
    if let Some(b) = &bin_chunk {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(b);
    }
    out
}
