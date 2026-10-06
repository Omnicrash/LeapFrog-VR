//! Writes the hand render model from a live frame: one bar per bone, the user's own lengths and
//! widths, in the pose's frame, so SteamVR's model of the hand sits exactly where the driver
//! puts the hand. Triggered by `driver_leapfrog.writeHandModel`; SteamVR loads render models at
//! start, so the result shows on the next run.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use glam::Vec3;

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

/// Wavefront OBJ of a smooth hand: a flattened rounded slab for the palm (wrist to knuckles),
/// capsules for the finger bones. Smooth normals, one texture coordinate.
fn obj(name: &str, bones: &[(Vec3, Vec3, f32)]) -> String {
    let mut mesh = Mesh::default();
    let fingers = fingers_of(bones);

    // Palm: wrist (the pose origin) to the knuckles of index..pinky.
    let knuckles: Vec<Vec3> = fingers.iter().skip(1).filter_map(|f| f.first().map(|b| b.1)).collect();
    if knuckles.len() >= 2 {
        let wrist = Vec3::ZERO;
        let knuckle_mid = knuckles.iter().sum::<Vec3>() / knuckles.len() as f32;
        let finger_width = fingers.iter().skip(1).filter_map(|f| f.get(1).map(|b| b.2)).sum::<f32>() / 4.0;
        let along = (knuckle_mid - wrist).normalize_or_zero();
        let across_raw = knuckles[0] - knuckles[knuckles.len() - 1];
        let across = (across_raw - along * across_raw.dot(along)).normalize_or_zero();
        let normal = along.cross(across).normalize_or_zero();
        let length = (knuckle_mid - wrist).length() + finger_width * 0.8;
        let width = across_raw.length() + finger_width * 1.2;
        let thickness = finger_width * 1.1;
        let center = (wrist + knuckle_mid) * 0.5 + along * (finger_width * 0.1);
        mesh.slab(center, along * (length * 0.5), across * (width * 0.5), normal * (thickness * 0.5));
    }

    // Fingers: every bone outside the palm, the thumb's whole chain.
    for (i, finger) in fingers.iter().enumerate() {
        for (j, &(a, b, width)) in finger.iter().enumerate() {
            if i != 0 && j == 0 {
                continue; // metacarpal, inside the palm
            }
            mesh.capsule(a, b, width * 0.5);
        }
    }
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

#[derive(Default)]
struct Mesh {
    vertices: Vec<(Vec3, Vec3)>, // position, normal
    faces: Vec<[usize; 3]>,       // 0-based
}

impl Mesh {
    const SEGMENTS: usize = 14;
    const RINGS: usize = 6;

    /// A grid of (RINGS+1) x SEGMENTS points, closed around the segments, into triangles.
    fn grid_faces(&mut self, first: usize, rows: usize, cols: usize) {
        for r in 0..rows - 1 {
            for c in 0..cols {
                let (c1, r1) = ((c + 1) % cols, r + 1);
                let i = |rr: usize, cc: usize| first + rr * cols + cc;
                self.faces.push([i(r, c), i(r1, c), i(r1, c1)]);
                self.faces.push([i(r, c), i(r1, c1), i(r, c1)]);
            }
        }
    }

    /// Rounded cylinder from a to b.
    fn capsule(&mut self, a: Vec3, b: Vec3, radius: f32) {
        let axis = (b - a).normalize_or_zero();
        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = axis.cross(helper).normalize_or_zero();
        let w = axis.cross(u).normalize_or_zero();
        let first = self.vertices.len();
        let mut rows = 0;
        // Hemisphere at a (pointing back), cylinder, hemisphere at b.
        for half in 0..2 {
            for r in 0..=Self::RINGS {
                let t = r as f32 / Self::RINGS as f32; // 0 at the pole, 1 at the equator
                let (lat_sin, lat_cos) = ((t * std::f32::consts::FRAC_PI_2).sin(), (t * std::f32::consts::FRAC_PI_2).cos());
                let (t_pole, t_eq) = if half == 0 { (a, a) } else { (b, b) };
                let (center, dir) = if half == 0 { (t_pole, -axis) } else { (t_eq, axis) };
                // Rings run pole -> equator for the a cap, equator -> pole for the b cap.
                let (rs, rc) = if half == 0 { (lat_sin, lat_cos) } else { (lat_cos, lat_sin) };
                for c in 0..Self::SEGMENTS {
                    let ang = c as f32 / Self::SEGMENTS as f32 * std::f32::consts::TAU;
                    let radial = u * ang.cos() + w * ang.sin();
                    let n = (radial * rs + dir * rc).normalize_or_zero();
                    self.vertices.push((center + n * radius, n));
                }
                rows += 1;
            }
        }
        self.grid_faces(first, rows, Self::SEGMENTS);
    }

    /// Flattened, squarish ellipsoid (a superellipsoid): half-axes along, across, thick.
    fn slab(&mut self, center: Vec3, along: Vec3, across: Vec3, thick: Vec3) {
        let first = self.vertices.len();
        let rows = Self::RINGS * 2 + 1;
        let cols = Self::SEGMENTS * 2;
        // Exponent below 1 squares the outline (palm), 1 is a plain ellipsoid.
        let sq = |v: f32, e: f32| v.signum() * v.abs().powf(e);
        for r in 0..rows {
            let lat = (r as f32 / (rows - 1) as f32 - 0.5) * std::f32::consts::PI; // -pi/2 .. pi/2 (thickness)
            for c in 0..cols {
                let lon = c as f32 / cols as f32 * std::f32::consts::TAU;
                let (x, y, z) = (sq(lon.cos(), 0.75) * lat.cos(), sq(lon.sin(), 0.75) * lat.cos(), lat.sin());
                let p = center + along * x + across * y + thick * z;
                // Normal of the scaled surface: component-wise divide by the squared half-axes.
                let n = (along.normalize_or_zero() * (x / along.length())
                    + across.normalize_or_zero() * (y / across.length())
                    + thick.normalize_or_zero() * (z / thick.length()))
                .normalize_or_zero();
                self.vertices.push((p, n));
            }
        }
        self.grid_faces(first, rows, cols);
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
        assert!(!text.contains("NaN"));
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
