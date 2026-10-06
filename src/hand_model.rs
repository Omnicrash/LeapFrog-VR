//! Writes the hand render model from a live frame: one bar per bone, the user's own lengths and
//! widths, in the pose's frame, so SteamVR's model of the hand sits exactly where the driver
//! puts the hand. Triggered by `driver_leapfrog.writeHandModel`; SteamVR loads render models at
//! start, so the result shows on the next run.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use glam::{Vec2, Vec3};

use crate::log::log;

/// Resources folder of the installed driver, from the DLL's location (bin/win64 -> root).
pub fn rendermodels_dir(dll_dir: &Path) -> Option<PathBuf> {
    let root = dll_dir.parent()?.parent()?;
    Some(root.join("resources").join("rendermodels"))
}

pub fn write(dir: &Path, side: &str, bones: &[(Vec3, Vec3, f32)]) {
    let name = format!("leapfrog_hand_{side}");
    let folder = dir.join(&name);
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join(format!("{name}.obj")), obj(&name, bones))?;
        std::fs::write(folder.join(format!("{name}.mtl")), format!("newmtl hand\nKd 0.82 0.82 0.85\nmap_Kd {name}-diff.png\n"))?;
        std::fs::write(folder.join(format!("{name}-diff.png")), png_flat(32, 210, 210, 216))?;
        std::fs::write(
            folder.join(format!("{name}.json")),
            format!(
                "{{\n  \"components\": {{\n    \"raw\": {{ \"filename\": \"{name}.obj\", \"component_local\": {{ \"origin\": [0.0, 0.0, 0.0], \"rotate_xyz\": [0.0, 0.0, 0.0] }} }},\n    \"base\": {{ \"component_local\": {{ \"origin\": [0.0, 0.0, 0.0], \"rotate_xyz\": [0.0, 0.0, 0.0] }} }}\n  }}\n}}\n"
            ),
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => log!("{side} hand model written to {} ({} bones); loads on the next SteamVR start", folder.display(), bones.len()),
        Err(e) => log!("{side} hand model not written to {}: {e}", folder.display()),
    }
}

/// Wavefront OBJ of the hand: the status icon's silhouette (`hand_outline`) extruded into a
/// flat slab with fully rounded edges, scaled to the measured hand (wrist to middle fingertip)
/// and laid in the palm plane, wrist at the pose origin, thumb on the index side.
fn obj(name: &str, bones: &[(Vec3, Vec3, f32)]) -> String {
    let mut mesh = Mesh::default();
    let fingers = fingers_of(bones);
    let knuckles: Vec<Vec3> = fingers.iter().skip(1).filter_map(|f| f.first().map(|b| b.1)).collect();
    let middle_tip = fingers.get(2).and_then(|f| f.last()).map(|b| b.1).unwrap_or(Vec3::new(0.0, 0.0, -0.18));
    let finger_width = fingers.iter().skip(1).filter_map(|f| f.get(1).map(|b| b.2)).sum::<f32>() / 4.0;
    let finger_width = if finger_width > 0.005 { finger_width } else { 0.018 };

    // Palm frame: along = wrist to knuckles, across = pinky to index (the thumb side), normal.
    let knuckle_mid = if knuckles.is_empty() { middle_tip * 0.5 } else { knuckles.iter().sum::<Vec3>() / knuckles.len() as f32 };
    let along = knuckle_mid.normalize_or_zero();
    let across_raw = if knuckles.len() >= 2 { knuckles[0] - knuckles[knuckles.len() - 1] } else { Vec3::X };
    let across = (across_raw - along * across_raw.dot(along)).normalize_or_zero();
    let normal = along.cross(across).normalize_or_zero();
    let length = middle_tip.length();
    let _ = finger_width;
    // Flat slab: the mid slice is exactly the outline; the edge is rounded with a radius well
    // under the icon's finger half-width (about 1/16 of the length), so no finger collapses.
    let half_thickness = length * 0.045;
    let radius = (length * 0.025).min(half_thickness);

    let outline: Vec<Vec2> = crate::hand_outline::HAND_OUTLINE.iter().map(|p| Vec2::new(p[0] * length, p[1] * length)).collect();
    mesh.rounded_slab(&outline, half_thickness, radius, |p, z| across * p.x + along * p.y + normal * z, across, along, normal);
    mesh.to_obj(name)
}

/// Groups the flat bone list back into fingers: a new finger starts where a bone does not
/// begin at the previous bone's end.
fn fingers_of(bones: &[(Vec3, Vec3, f32)]) -> Vec<Vec<(Vec3, Vec3, f32)>> {
    let mut fingers: Vec<Vec<(Vec3, Vec3, f32)>> = Vec::new();
    for &bone in bones {
        match fingers.last_mut() {
            Some(f) if f.last().is_some_and(|prev| prev.1.distance(bone.0) < 0.004) => f.push(bone),
            _ => fingers.push(vec![bone]),
        }
    }
    fingers
}

/// Signed area: positive for counterclockwise.
fn signed_area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    (0..n).map(|i| poly[i].perp_dot(poly[(i + 1) % n])).sum::<f32>() * 0.5
}

/// Resamples a closed polygon to points spaced about `step` apart along it.
fn resample(poly: &[Vec2], step: f32) -> Vec<Vec2> {
    let n = poly.len();
    let total: f32 = (0..n).map(|i| poly[i].distance(poly[(i + 1) % n])).sum();
    let count = ((total / step).round() as usize).max(8);
    let spacing = total / count as f32;
    let mut out = Vec::with_capacity(count);
    let (mut i, mut along) = (0usize, 0.0f32); // current edge and distance into it
    for _ in 0..count {
        loop {
            let len = poly[i].distance(poly[(i + 1) % n]);
            if along <= len || len == 0.0 && along <= 0.0 {
                let t = if len > 0.0 { along / len } else { 0.0 };
                out.push(poly[i].lerp(poly[(i + 1) % n], t));
                along += spacing;
                break;
            }
            along -= len;
            i = (i + 1) % n;
        }
    }
    out
}

/// Moves every vertex of a counterclockwise polygon inward by `d` (miter, limited). Edges that
/// would flip direction, where the outline is thinner than 2d, collapse to their midpoint
/// instead of crossing, so the point count stays and nothing turns inside out.
fn offset_inward(poly: &[Vec2], d: f32) -> Vec<Vec2> {
    let n = poly.len();
    let mut out: Vec<Vec2> = (0..n)
        .map(|i| {
            let (prev, cur, next) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let n1 = (cur - prev).normalize_or_zero().perp(); // left normal = inward for CCW
            let n2 = (next - cur).normalize_or_zero().perp();
            let bisector = (n1 + n2).normalize_or_zero();
            let cos_half = bisector.dot(n1).max(0.5); // miter limit 2x
            cur + bisector * (d / cos_half)
        })
        .collect();
    for _ in 0..8 {
        let mut changed = false;
        for i in 0..n {
            let j = (i + 1) % n;
            if (poly[j] - poly[i]).dot(out[j] - out[i]) < 0.0 {
                let mid = (out[i] + out[j]) * 0.5;
                out[i] = mid;
                out[j] = mid;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// Ear clipping of a simple counterclockwise polygon; indices into it.
fn triangulate(poly: &[Vec2]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    let mut tris = Vec::new();
    let inside = |p: Vec2, a: Vec2, b: Vec2, c: Vec2| {
        let s1 = (b - a).perp_dot(p - a);
        let s2 = (c - b).perp_dot(p - b);
        let s3 = (a - c).perp_dot(p - c);
        s1 >= -1e-9 && s2 >= -1e-9 && s3 >= -1e-9
    };
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if (b - a).perp_dot(c - b) <= 1e-9 {
                continue; // reflex or degenerate corner
            }
            let blocked = idx.iter().any(|&k| k != ia && k != ib && k != ic && inside(poly[k], a, b, c));
            if blocked {
                continue;
            }
            tris.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Numerically stuck: fan the rest rather than loop forever.
            for i in 1..idx.len() - 1 {
                tris.push([idx[0], idx[i], idx[i + 1]]);
            }
            return tris;
        }
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

#[derive(Default)]
struct Mesh {
    vertices: Vec<(Vec3, Vec3)>, // position, normal
    faces: Vec<[usize; 3]>,       // 0-based
}

impl Mesh {
    /// The outline extruded to a slab of half-thickness `half`: a straight wall from the mid
    /// slice up to `half - radius`, then a quarter-circle edge into the flat top; mirrored
    /// below. `place` maps outline coordinates and a height to 3D.
    fn rounded_slab(&mut self, outline: &[Vec2], half: f32, radius: f32, place: impl Fn(Vec2, f32) -> Vec3, across: Vec3, along: Vec3, normal: Vec3) {
        let mut outline: Vec<Vec2> = resample(outline, radius * 0.6);
        if signed_area(&outline) < 0.0 {
            outline.reverse();
        }
        const STEPS: usize = 6;
        let n = outline.len();
        let to3 = |v: Vec2, z: f32| (across * v.x + along * v.y + normal * z).normalize_or_zero();
        for side in [1.0f32, -1.0] {
            // Rings from the equator (edge, z = 0) inward and up to the flat cap at d = radius.
            let first = self.vertices.len();
            let mut rings = Vec::new();
            // Ring 0: the mid slice (z = 0), the outline itself; ring 1: top of the straight
            // wall; then the quarter circle in STEPS - 1 more rings.
            for k in 0..=STEPS {
                let (d, z, theta) = if k == 0 {
                    (0.0, 0.0, 0.0)
                } else {
                    let theta = (k - 1) as f32 / (STEPS - 1) as f32 * std::f32::consts::FRAC_PI_2;
                    (radius * (1.0 - theta.cos()), (half - radius) + radius * theta.sin(), theta)
                };
                let ring = offset_inward(&outline, d);
                for (i, p) in ring.iter().enumerate() {
                    let (prev, next) = (ring[(i + n - 1) % n], ring[(i + 1) % n]);
                    let outward = -((next - prev).normalize_or_zero().perp());
                    let nrm = to3(outward * theta.cos(), theta.sin() * side);
                    self.vertices.push((place(*p, z * side), nrm));
                }
                rings.push(ring);
            }
            // Quads between consecutive rings; winding flipped for the bottom side.
            for k in 0..STEPS {
                for i in 0..n {
                    let (a, b) = (first + k * n + i, first + k * n + (i + 1) % n);
                    let (c, d) = (first + (k + 1) * n + (i + 1) % n, first + (k + 1) * n + i);
                    if side > 0.0 {
                        self.faces.push([a, b, c]);
                        self.faces.push([a, c, d]);
                    } else {
                        self.faces.push([a, c, b]);
                        self.faces.push([a, d, c]);
                    }
                }
            }
            // The flat cap over the innermost ring.
            let cap_first = first + STEPS * n;
            for t in triangulate(&rings[STEPS]) {
                if side > 0.0 {
                    self.faces.push([cap_first + t[0], cap_first + t[1], cap_first + t[2]]);
                } else {
                    self.faces.push([cap_first + t[0], cap_first + t[2], cap_first + t[1]]);
                }
            }
        }
    }

    fn to_obj(&self, name: &str) -> String {
        let mut out = format!("# LeapFrog hand, generated from a live frame\nmtllib {name}.mtl\nusemtl hand\nvt 0.5 0.5\n");
        for (p, _) in &self.vertices {
            let _ = writeln!(out, "v {:.5} {:.5} {:.5}", p.x, p.y, p.z);
        }
        for (_, n) in &self.vertices {
            let _ = writeln!(out, "vn {:.4} {:.4} {:.4}", n.x, n.y, n.z);
        }
        for f in &self.faces {
            let _ = writeln!(out, "f {}/1/{} {}/1/{} {}/1/{}", f[0] + 1, f[0] + 1, f[1] + 1, f[1] + 1, f[2] + 1, f[2] + 1);
        }
        out
    }
}

/// Reads the joints back out of an OBJ written by the previous bar-per-bone generator (eight
/// vertices per bone, four at each end, ends padded by a quarter width), for regenerating.
pub fn bones_from_bar_obj(text: &str) -> Vec<(Vec3, Vec3, f32)> {
    let vs: Vec<Vec3> = text
        .lines()
        .filter(|l| l.starts_with("v "))
        .filter_map(|l| {
            let n: Vec<f32> = l[2..].split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (n.len() == 3).then(|| Vec3::new(n[0], n[1], n[2]))
        })
        .collect();
    vs.chunks_exact(8)
        .map(|c| {
            let a = (c[0] + c[1] + c[2] + c[3]) / 4.0;
            let b = (c[4] + c[5] + c[6] + c[7]) / 4.0;
            let width = c[0].distance(c[1]);
            let axis = (b - a).normalize_or_zero();
            (a + axis * (width * 0.25), b - axis * (width * 0.25), width)
        })
        .collect()
}

/// A flat-color PNG of `size` x `size`, no image library needed. 32 and up keeps the
/// compositor's mip levels happy (a 4x4 texture logged "Failed to create SRV" per mip).
fn png_flat(size: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &byte in data {
            c ^= byte as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn adler(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for _ in 0..size {
        raw.push(0); // filter: none
        for _ in 0..size {
            raw.extend_from_slice(&[r, g, b]);
        }
    }
    // zlib with one stored (uncompressed) deflate block
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    z.extend_from_slice(&adler(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_hand() -> Vec<(Vec3, Vec3, f32)> {
        // Five fingers along +X from the wrist at the origin, spread along Z; thumb shorter.
        let mut bones = Vec::new();
        for f in 0..5 {
            let z = (f as f32 - 2.0) * 0.02;
            let mut x = if f == 0 { 0.02 } else { 0.0 };
            let count = if f == 0 { 3 } else { 4 };
            for j in 0..count {
                let len = if j == 0 { 0.07 } else { 0.03 };
                bones.push((Vec3::new(x, 0.0, z), Vec3::new(x + len, 0.0, z), 0.015));
                x += len;
            }
        }
        bones
    }

    #[test]
    fn fingers_are_regrouped_from_the_flat_list() {
        let fingers = fingers_of(&test_hand());
        assert_eq!(fingers.iter().map(Vec::len).collect::<Vec<_>>(), vec![3, 4, 4, 4, 4]);
    }

    #[test]
    fn obj_is_a_closed_smooth_mesh() {
        let text = obj("test", &test_hand());
        let v = text.lines().filter(|l| l.starts_with("v ")).count();
        let vn = text.lines().filter(|l| l.starts_with("vn ")).count();
        let f = text.lines().filter(|l| l.starts_with("f ")).count();
        assert_eq!(v, vn);
        assert!(f > 1000, "faces: {f}");
        assert!(!text.contains("NaN") && !text.contains("inf"));
    }

    #[test]
    fn outline_triangulates_completely() {
        let poly: Vec<Vec2> = crate::hand_outline::HAND_OUTLINE.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let mut poly = poly;
        if signed_area(&poly) < 0.0 {
            poly.reverse();
        }
        let tris = triangulate(&poly);
        assert_eq!(tris.len(), poly.len() - 2);
        // Total triangle area equals the polygon's.
        let area: f32 = tris.iter().map(|t| (poly[t[1]] - poly[t[0]]).perp_dot(poly[t[2]] - poly[t[0]]) * 0.5).sum();
        assert!((area - signed_area(&poly)).abs() < 1e-3, "area {area} vs {}", signed_area(&poly));
    }

    #[test]
    fn offset_rings_keep_their_shape_at_the_model_radius() {
        let length = 0.16f32;
        let radius = length * 0.025;
        let raw: Vec<Vec2> = crate::hand_outline::HAND_OUTLINE.iter().map(|p| Vec2::new(p[0] * length, p[1] * length)).collect();
        let mut poly = resample(&raw, radius * 0.6);
        if signed_area(&poly) < 0.0 {
            poly.reverse();
        }
        let base = signed_area(&poly);
        let mut last = base;
        for k in 1..=6 {
            let ring = offset_inward(&poly, radius * k as f32 / 6.0);
            let area = signed_area(&ring);
            assert!(area > 0.0 && area < last, "ring {k}: area {area} vs {last}");
            // No edge may point against the original outline (collapsed edges are fine).
            for i in 0..poly.len() {
                let (a, b) = (poly[(i + 1) % poly.len()] - poly[i], ring[(i + 1) % poly.len()] - ring[i]);
                assert!(a.dot(b) >= -1e-9, "edge {i} flipped at ring {k}");
            }
            last = area;
        }
    }

    #[test]
    fn mid_slice_is_the_outline() {
        let text = obj("test", &test_hand());
        // The first ring written is the outline at z = 0 in the palm plane: for the test hand
        // the plane is y = 0, so those vertices have |y| ~ 0.
        let ys: Vec<f32> = text.lines().filter(|l| l.starts_with("v ")).take(40)
            .filter_map(|l| l[2..].split_whitespace().nth(1)?.parse().ok()).collect();
        assert!(ys.iter().all(|y| y.abs() < 1e-4), "mid slice off the palm plane: {:?}", &ys[..4]);
    }

    #[test]
    fn hand_is_scaled_to_the_measured_length() {
        let text = obj("test", &test_hand());
        // Middle finger tip in the test hand is at x = 0.16: the model must reach about there.
        let max_x = text.lines().filter(|l| l.starts_with("v ")).filter_map(|l| l[2..].split_whitespace().next()?.parse::<f32>().ok()).fold(0.0, f32::max);
        assert!((max_x - 0.16).abs() < 0.01, "max x {max_x}");
    }

    #[test]
    fn bars_round_trip_to_joints() {
        let bones = test_hand();
        // Emulate the old generator's eight corners per bone.
        let mut text = String::new();
        for &(a, b, w) in &bones {
            let axis = (b - a).normalize_or_zero();
            let (a, b) = (a - axis * (w * 0.25), b + axis * (w * 0.25));
            let (u, ww) = (Vec3::Y * (w * 0.5), Vec3::Z * (w * 0.5));
            for c in [a + u + ww, a - u + ww, a - u - ww, a + u - ww, b + u + ww, b - u + ww, b - u - ww, b + u - ww] {
                text.push_str(&format!("v {} {} {}\n", c.x, c.y, c.z));
            }
        }
        let back = bones_from_bar_obj(&text);
        assert_eq!(back.len(), bones.len());
        for (x, y) in back.iter().zip(&bones) {
            assert!(x.0.distance(y.0) < 1e-4 && x.1.distance(y.1) < 1e-4 && (x.2 - y.2).abs() < 1e-4);
        }
    }

    #[test]
    fn png_is_well_formed() {
        let png = png_flat(32, 1, 2, 3);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.ends_with(&[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }
}
