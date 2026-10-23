//! The icon a Windows program carries (its biggest one), for games with no cover: the program's
//! resources hold icon groups (RT_GROUP_ICON) listing images (RT_ICON), each a PNG or a bitmap
//! that, given an icon file's header, the image crate reads.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The resource section's bytes and the address (RVA) it starts at.
fn resources(path: &Path) -> Option<(Vec<u8>, u32)> {
    let mut f = File::open(path).ok()?;
    let mut head = vec![0u8; 4096];
    let n = f.read(&mut head).ok()?;
    head.truncate(n);
    let pe = u32_at(&head, 0x3c)? as usize;
    if head.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(&head, pe + 6)? as usize;
    let optional = pe + 24;
    let opt_size = u16_at(&head, pe + 20)? as usize;
    let dirs = match u16_at(&head, optional)? {
        0x10b => optional + 96,
        0x20b => optional + 112,
        _ => return None,
    };
    let rva = u32_at(&head, dirs + 2 * 8)?;
    if rva == 0 {
        return None;
    }
    let table = optional + opt_size;
    for i in 0..sections {
        let s = table + i * 40;
        let (va, raw_size, raw_at) = (u32_at(&head, s + 12)?, u32_at(&head, s + 16)?, u32_at(&head, s + 20)?);
        let virtual_size = u32_at(&head, s + 8)?.max(raw_size);
        if rva >= va && rva < va + virtual_size {
            // Big resource sections are videos or data, not icons worth reading 100 MB for.
            if raw_size > 64 << 20 {
                return None;
            }
            let mut data = vec![0u8; raw_size as usize];
            f.seek(SeekFrom::Start(raw_at as u64)).ok()?;
            f.read_exact(&mut data).ok()?;
            return Some((data, va));
        }
    }
    None
}

/// A resource directory's entries: (id, offset, whether it's a directory). Named entries get
/// id u32::MAX.
fn entries(rsrc: &[u8], at: usize) -> Vec<(u32, usize, bool)> {
    let (Some(named), Some(ids)) = (u16_at(rsrc, at + 12), u16_at(rsrc, at + 14)) else { return Vec::new() };
    (0..(named + ids) as usize)
        .filter_map(|i| {
            let e = at + 16 + i * 8;
            let (name, off) = (u32_at(rsrc, e)?, u32_at(rsrc, e + 4)?);
            let id = if name & 0x8000_0000 != 0 { u32::MAX } else { name };
            Some((id, (off & 0x7fff_ffff) as usize, off & 0x8000_0000 != 0))
        })
        .collect()
}

/// The data of the first resource of a type (with a given id, if any), whatever its language.
fn resource(rsrc: &[u8], va: u32, kind: u32, id: Option<u32>) -> Option<&[u8]> {
    let (_, names, _) = entries(rsrc, 0).into_iter().find(|(t, _, dir)| *t == kind && *dir)?;
    let (_, langs, _) = entries(rsrc, names).into_iter().find(|(i, _, dir)| *dir && id.is_none_or(|id| *i == id))?;
    let (_, data, _) = entries(rsrc, langs).into_iter().find(|(_, _, dir)| !dir)?;
    let (at, size) = (u32_at(rsrc, data)?.checked_sub(va)? as usize, u32_at(rsrc, data + 4)? as usize);
    rsrc.get(at..at + size)
}

/// The program's biggest icon, as an image file's bytes (an icon file, or a PNG).
pub fn icon(path: &Path) -> Option<Vec<u8>> {
    let (rsrc, va) = resources(path)?;
    let group = resource(&rsrc, va, RT_GROUP_ICON, None)?;
    let count = u16_at(group, 4)? as usize;
    // The biggest (0 means 256), then the most colours.
    let (w, h, colors, planes, bits, id) = (0..count)
        .filter_map(|i| {
            let e = 6 + i * 14;
            let size = |b: u8| if b == 0 { 256u32 } else { b as u32 };
            Some((size(*group.get(e)?), size(*group.get(e + 1)?), *group.get(e + 2)?, u16_at(group, e + 4)?, u16_at(group, e + 6)?, u16_at(group, e + 12)?))
        })
        .max_by_key(|(w, _, _, _, bits, _)| (*w, *bits))?;
    let image = resource(&rsrc, va, RT_ICON, Some(id as u32))?;
    if image.starts_with(b"\x89PNG") {
        return Some(image.to_vec());
    }
    // A bitmap: put an icon file's header in front of it.
    let mut ico = Vec::with_capacity(22 + image.len());
    ico.extend([0, 0, 1, 0, 1, 0]);
    ico.extend([(w % 256) as u8, (h % 256) as u8, colors, 0]);
    ico.extend(planes.to_le_bytes());
    ico.extend(bits.to_le_bytes());
    ico.extend((image.len() as u32).to_le_bytes());
    ico.extend(22u32.to_le_bytes());
    ico.extend(image);
    Some(ico)
}

/// The program's icon saved as a PNG in lobby's cache (made once), for the window to show.
pub fn icon_file(program: &Path) -> Option<std::path::PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    let file = cache.join("lobby/icons").join(format!("{}.png", crate::sha256_hex(program.as_os_str().as_encoded_bytes())));
    if file.is_file() {
        return Some(file);
    }
    let bytes = icon(program)?;
    let img = image::load_from_memory(&bytes).ok()?;
    std::fs::create_dir_all(file.parent()?).ok()?;
    img.save_with_format(&file, image::ImageFormat::Png).ok()?;
    Some(file)
}
