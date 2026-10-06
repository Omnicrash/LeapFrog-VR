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

/// Wavefront OBJ: a box per bone, square cross-section of the bone's width.
fn obj(name: &str, bones: &[(Vec3, Vec3, f32)]) -> String {
    let mut out = format!("# LeapFrog hand, generated from a live frame\nmtllib {name}.mtl\nusemtl hand\nvt 0.5 0.5\n");
    let mut faces = String::new();
    let mut v = 0usize; // vertices written so far
    let mut n = 0usize; // normals written so far
    for &(a, b, width) in bones {
        let axis = (b - a).normalize_or_zero();
        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = axis.cross(helper).normalize_or_zero() * (width * 0.5);
        let w = axis.cross(u).normalize_or_zero() * (width * 0.5);
        // Slightly longer than the bone so joints meet: half a width on each end.
        let (a, b) = (a - axis * (width * 0.25), b + axis * (width * 0.25));
        let corners = [a + u + w, a - u + w, a - u - w, a + u - w, b + u + w, b - u + w, b - u - w, b + u - w];
        for c in corners {
            let _ = writeln!(out, "v {:.5} {:.5} {:.5}", c.x, c.y, c.z);
        }
        // Six faces, two triangles each, with one normal per face.
        let quads: [([usize; 4], Vec3); 6] = [
            ([0, 1, 2, 3], -axis),
            ([4, 7, 6, 5], axis),
            ([0, 4, 5, 1], w.normalize_or_zero()),
            ([1, 5, 6, 2], -u.normalize_or_zero()),
            ([2, 6, 7, 3], -w.normalize_or_zero()),
            ([3, 7, 4, 0], u.normalize_or_zero()),
        ];
        for (q, normal) in quads {
            let _ = writeln!(out, "vn {:.4} {:.4} {:.4}", normal.x, normal.y, normal.z);
            n += 1;
            let i = |k: usize| v + q[k] + 1;
            let _ = writeln!(faces, "f {}/1/{n} {}/1/{n} {}/1/{n}", i(0), i(1), i(2));
            let _ = writeln!(faces, "f {}/1/{n} {}/1/{n} {}/1/{n}", i(0), i(2), i(3));
        }
        v += 8;
    }
    out.push_str(&faces);
    out
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

    #[test]
    fn obj_has_a_box_per_bone() {
        let bones = vec![(Vec3::ZERO, Vec3::new(0.05, 0.0, 0.0), 0.015), (Vec3::new(0.05, 0.0, 0.0), Vec3::new(0.08, 0.0, 0.0), 0.012)];
        let text = obj("test", &bones);
        assert_eq!(text.lines().filter(|l| l.starts_with("v ")).count(), 16);
        assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 24);
    }

    #[test]
    fn png_is_well_formed() {
        let png = png_flat(32, 1, 2, 3);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.ends_with(&[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }
}
