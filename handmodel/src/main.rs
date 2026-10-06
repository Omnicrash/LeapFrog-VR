//! `handmodel <rendermodels dir>`: writes `leapfrog_hand_left` and `_right`, the placeholder
//! hand render models: the status icon's outline as a rounded 3D shape. Its flat projection is
//! the outline, by construction: a grid over the outline, each point raised by a profile of its
//! exact distance to the edge (steep at the edge, flat on top), boundary points snapped onto
//! the outline itself. Fingers come out as rounded bars, the palm as a cushion. Fixed size.
//!
//! Pose frame of the LeapFrog hand devices (from the one joint capture, 2026-10-06): fingers
//! point along -Z, the palm plane is XZ, the thumb side is +X for the left hand and -X for the
//! right, thickness along Y.

mod outline;

use std::fmt::Write as _;
use std::path::Path;

use glam::{Vec2, Vec3};

/// Wrist to middle fingertip, meters. A placeholder's size, not anyone's hand.
const LENGTH: f32 = 0.18;
/// Half thickness at the palm's center, as a share of the length.
const HALF_THICKNESS: f32 = 0.07;
/// Grid step, as a share of the length.
const STEP: f32 = 1.0 / 130.0;

fn main() {
    let dir = std::env::args().nth(1).expect("rendermodels directory");
    for (side, thumb_sign) in [("left", 1.0f32), ("right", -1.0)] {
        let mesh = Mesh::build(thumb_sign);
        let name = format!("leapfrog_hand_{side}");
        let folder = Path::new(&dir).join(&name);
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join(format!("{name}.obj")), mesh.obj(&name)).expect("obj");
        std::fs::write(folder.join(format!("{name}.mtl")), format!("newmtl hand\nKd 0.16 0.16 0.17\nKs 0.08 0.08 0.08\nNs 20\nmap_Kd {name}-diff.png\n")).expect("mtl");
        std::fs::write(folder.join(format!("{name}-diff.png")), png_flat(32, 40, 40, 43)).expect("png");
        std::fs::write(
            folder.join(format!("{name}.json")),
            format!("{{\n  \"components\": {{\n    \"raw\": {{ \"filename\": \"{name}.obj\", \"component_local\": {{ \"origin\": [0.0, 0.0, 0.0], \"rotate_xyz\": [0.0, 0.0, 0.0] }} }},\n    \"base\": {{ \"component_local\": {{ \"origin\": [0.0, 0.0, 0.0], \"rotate_xyz\": [0.0, 0.0, 0.0] }} }}\n  }}\n}}\n"),
        )
        .expect("json");
        println!("{}: {} vertices, {} triangles", folder.display(), mesh.vertices.len(), mesh.faces.len());
    }
}

/// The outline in meters: x across (thumb side positive), y along the fingers from the wrist.
fn polygon() -> Vec<Vec2> {
    outline::HAND_OUTLINE.iter().map(|p| Vec2::new(p[0] * LENGTH, p[1] * LENGTH)).collect()
}

fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut odd = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                odd = !odd;
            }
        }
    }
    odd
}

/// Nearest point on the outline and the distance to it.
fn nearest(poly: &[Vec2], p: Vec2) -> (Vec2, f32) {
    let n = poly.len();
    let mut best = (poly[0], f32::INFINITY);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let ab = b - a;
        let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        let q = a + ab * t;
        let d = p.distance(q);
        if d < best.1 {
            best = (q, d);
        }
    }
    best
}

struct Mesh {
    vertices: Vec<Vec3>,
    normals: Vec<Vec3>,
    faces: Vec<[usize; 3]>,
    /// Outline coordinates and height of every vertex, for the tests.
    flat: Vec<(Vec2, f32)>,
}

impl Mesh {
    fn build(thumb_sign: f32) -> Mesh {
        let poly = polygon();
        let step = LENGTH * STEP;
        let (min, max) = poly.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let cols = ((max.x - min.x) / step).ceil() as usize + 3;
        let rows = ((max.y - min.y) / step).ceil() as usize + 3;
        let origin = min - Vec2::splat(step);
        let at = |c: usize, r: usize| origin + Vec2::new(c as f32 * step, r as f32 * step);

        // Node classification: inside with its distance, or a boundary node snapped onto the
        // outline (an outside node touching an inside one, 8-neighborhood), or nothing.
        let mut is_inside = vec![false; cols * rows];
        let mut dist = vec![0.0f32; cols * rows];
        for r in 0..rows {
            for c in 0..cols {
                let p = at(c, r);
                if inside(&poly, p) {
                    is_inside[r * cols + c] = true;
                    dist[r * cols + c] = nearest(&poly, p).1;
                }
            }
        }
        let reach = dist.iter().cloned().fold(0.0, f32::max);
        let half = LENGTH * HALF_THICKNESS;
        let profile = |d: f32| half * (1.0 - (1.0 - (d / reach).min(1.0)).powi(2)).max(0.0).sqrt();

        let mut node: Vec<Option<(Vec2, f32)>> = vec![None; cols * rows]; // outline coords, height
        for r in 0..rows {
            for c in 0..cols {
                let i = r * cols + c;
                if is_inside[i] {
                    node[i] = Some((at(c, r), profile(dist[i])));
                } else {
                    let touches = (-1i32..=1).any(|dr| {
                        (-1i32..=1).any(|dc| {
                            let (rr, cc) = (r as i32 + dr, c as i32 + dc);
                            rr >= 0 && cc >= 0 && (rr as usize) < rows && (cc as usize) < cols && is_inside[rr as usize * cols + cc as usize]
                        })
                    });
                    if touches {
                        node[i] = Some((nearest(&poly, at(c, r)).0, 0.0));
                    }
                }
            }
        }

        // Pose frame placement: x across (thumb side), y along the fingers, z the thickness.
        let place = |p: Vec2, z: f32| Vec3::new(thumb_sign * p.x, z, -p.y);
        let mut mesh = Mesh { vertices: Vec::new(), normals: Vec::new(), faces: Vec::new(), flat: Vec::new() };
        for side in [1.0f32, -1.0] {
            let mut index = vec![usize::MAX; cols * rows];
            for (i, n) in node.iter().enumerate() {
                if let Some((p, z)) = n {
                    index[i] = mesh.vertices.len();
                    mesh.vertices.push(place(*p, z * side));
                    mesh.flat.push((*p, z * side));
                }
            }
            for r in 0..rows - 1 {
                for c in 0..cols - 1 {
                    let corners = [index[r * cols + c], index[r * cols + c + 1], index[(r + 1) * cols + c + 1], index[(r + 1) * cols + c]];
                    let cell_inside = [r * cols + c, r * cols + c + 1, (r + 1) * cols + c + 1, (r + 1) * cols + c].iter().any(|&i| is_inside[i]);
                    if !cell_inside || corners.iter().any(|&i| i == usize::MAX) {
                        continue;
                    }
                    let (a, b, cc, d) = (corners[0], corners[1], corners[2], corners[3]);
                    if side > 0.0 {
                        mesh.faces.push([a, b, cc]);
                        mesh.faces.push([a, cc, d]);
                    } else {
                        mesh.faces.push([a, cc, b]);
                        mesh.faces.push([a, d, cc]);
                    }
                }
            }
        }
        mesh.normals = vec![Vec3::ZERO; mesh.vertices.len()];
        for f in &mesh.faces {
            let (a, b, c) = (mesh.vertices[f[0]], mesh.vertices[f[1]], mesh.vertices[f[2]]);
            let n = (b - a).cross(c - a);
            for &i in f {
                mesh.normals[i] += n;
            }
        }
        for n in &mut mesh.normals {
            *n = n.normalize_or(Vec3::Y);
        }
        mesh
    }

    fn obj(&self, name: &str) -> String {
        let mut out = format!("# LeapFrog placeholder hand: the status icon's outline in 3D (handmodel/)\nmtllib {name}.mtl\nusemtl hand\nvt 0.5 0.5\n");
        for v in &self.vertices {
            let _ = writeln!(out, "v {:.5} {:.5} {:.5}", v.x, v.y, v.z);
        }
        for n in &self.normals {
            let _ = writeln!(out, "vn {:.4} {:.4} {:.4}", n.x, n.y, n.z);
        }
        for f in &self.faces {
            let _ = writeln!(out, "f {}/1/{} {}/1/{} {}/1/{}", f[0] + 1, f[0] + 1, f[1] + 1, f[1] + 1, f[2] + 1, f[2] + 1);
        }
        out
    }
}

/// A flat-color PNG of `size` x `size`, no image library needed (32 keeps mip levels happy).
/// Near-black, matte: the Index controllers' look.
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
        raw.push(0);
        for _ in 0..size {
            raw.extend_from_slice(&[r, g, b]);
        }
    }
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    z.extend_from_slice(&raw);
    z.extend_from_slice(&adler(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass/fail rule: projected flat, the model is the outline. Every vertex lies inside
    /// it or on it, and every point of the outline has a mesh vertex on it.
    #[test]
    fn flat_projection_is_the_outline() {
        let poly = polygon();
        let mesh = Mesh::build(1.0);
        let step = LENGTH * STEP;
        for (p, _) in &mesh.flat {
            let (_, d) = nearest(&poly, *p);
            assert!(inside(&poly, *p) || d < 1e-5, "vertex {p} lies {d} m outside the outline");
        }
        let rim: Vec<Vec2> = mesh.flat.iter().filter(|(_, z)| z.abs() < 1e-7).map(|(p, _)| *p).collect();
        for q in &poly {
            let d = rim.iter().map(|p| p.distance(*q)).fold(f32::INFINITY, f32::min);
            assert!(d < step * 2.0, "outline point {q} is {d} m from the nearest rim vertex");
        }
    }

    #[test]
    fn dome_has_the_intended_thickness_and_size() {
        let mesh = Mesh::build(1.0);
        let top = mesh.vertices.iter().map(|v| v.y).fold(f32::MIN, f32::max);
        let bottom = mesh.vertices.iter().map(|v| v.y).fold(f32::MAX, f32::min);
        assert!((top - LENGTH * HALF_THICKNESS).abs() < 1e-4 && (bottom + LENGTH * HALF_THICKNESS).abs() < 1e-4, "{bottom}..{top}");
        let far = mesh.vertices.iter().map(|v| -v.z).fold(f32::MIN, f32::max);
        assert!((far - LENGTH).abs() < 2e-3, "fingertips at {far}");
        assert!(mesh.vertices.iter().all(|v| v.is_finite()) && mesh.normals.iter().all(|n| n.is_finite()));
    }

    #[test]
    fn right_hand_mirrors_the_thumb_side() {
        let (l, r) = (Mesh::build(1.0), Mesh::build(-1.0));
        let max_x = |m: &Mesh| m.vertices.iter().map(|v| v.x).fold(f32::MIN, f32::max);
        let min_x = |m: &Mesh| m.vertices.iter().map(|v| v.x).fold(f32::MAX, f32::min);
        assert!((max_x(&l) + min_x(&r)).abs() < 1e-5 && (min_x(&l) + max_x(&r)).abs() < 1e-5);
    }
}
