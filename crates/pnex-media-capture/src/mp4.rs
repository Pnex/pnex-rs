//! MP4 "fast start" in memory: moves the `moov` box before `mdat` and
//! shifts the chunk offsets, so ffmpeg can demux a file read from a pipe
//! (no seek). Phone recordings write `moov` at the end.

/// One top-level box: `(start, len, type)`.
fn boxes(bytes: &[u8]) -> Option<Vec<(usize, usize, [u8; 4])>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let head = bytes.get(pos..pos + 8)?;
        let size32 = u32::from_be_bytes(head[0..4].try_into().ok()?) as usize;
        let kind: [u8; 4] = head[4..8].try_into().ok()?;
        let len = match size32 {
            0 => bytes.len() - pos,
            1 => u64::from_be_bytes(bytes.get(pos + 8..pos + 16)?.try_into().ok()?) as usize,
            n => n,
        };
        if len < 8 || pos.checked_add(len)? > bytes.len() {
            return None;
        }
        out.push((pos, len, kind));
        pos += len;
    }
    Some(out)
}

/// Adds `delta` to every `stco` / `co64` entry under `moov` whose offset is
/// in `[from, to)`. `None` = malformed, compressed `moov`, or a 32-bit
/// offset that would overflow.
fn shift_offsets(moov: &mut [u8], from: u64, to: u64, delta: u64) -> Option<()> {
    const CONTAINERS: [&[u8; 4]; 5] = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];
    let mut pos = 8usize;
    let end = moov.len();
    // Iterative walk: (cursor, end) of the containers being visited.
    let mut stack = vec![(pos, end)];
    while let Some((p, e)) = stack.pop() {
        pos = p;
        while pos + 8 <= e {
            let size = u32::from_be_bytes(moov[pos..pos + 4].try_into().ok()?) as usize;
            let kind: [u8; 4] = moov[pos + 4..pos + 8].try_into().ok()?;
            if size < 8 || pos + size > e {
                return None;
            }
            if &kind == b"cmov" {
                return None;
            }
            if CONTAINERS.contains(&&kind) {
                stack.push((pos + 8, pos + size));
            } else if &kind == b"stco" || &kind == b"co64" {
                let wide = &kind == b"co64";
                let count = u32::from_be_bytes(moov.get(pos + 12..pos + 16)?.try_into().ok()?);
                let step = if wide { 8 } else { 4 };
                let first = pos + 16;
                if first + count as usize * step > pos + size {
                    return None;
                }
                for i in 0..count as usize {
                    let at = first + i * step;
                    let cell = &mut moov[at..at + step];
                    let off = if wide {
                        u64::from_be_bytes(cell.try_into().ok()?)
                    } else {
                        u64::from(u32::from_be_bytes(cell.try_into().ok()?))
                    };
                    if off < from || off >= to {
                        continue;
                    }
                    let new = off + delta;
                    if wide {
                        cell.copy_from_slice(&new.to_be_bytes());
                    } else {
                        // shortcut: no stco → co64 upgrade; files over 4 GiB are refused.
                        cell.copy_from_slice(&u32::try_from(new).ok()?.to_be_bytes());
                    }
                }
            }
            pos += size;
        }
    }
    Some(())
}

/// The file with `moov` before the first `mdat`; unchanged when it already
/// is. `None` = not a readable MP4 (no `moov`, malformed boxes).
pub fn faststart(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let list = boxes(&bytes)?;
    let moov_i = list.iter().position(|b| &b.2 == b"moov")?;
    let Some(mdat_i) = list.iter().position(|b| &b.2 == b"mdat") else {
        return Some(bytes);
    };
    if moov_i < mdat_i {
        return Some(bytes);
    }
    let (moov_start, moov_len, _) = list[moov_i];
    let insert_at = list[mdat_i].0;
    if moov_len != u32::from_be_bytes(bytes[moov_start..moov_start + 4].try_into().ok()?) as usize {
        // A 64-bit sized moov: rare, not handled.
        return None;
    }
    let mut moov = bytes[moov_start..moov_start + moov_len].to_vec();
    // Everything between the insertion point and the old moov moves by
    // the size of moov.
    shift_offsets(
        &mut moov,
        insert_at as u64,
        moov_start as u64,
        moov_len as u64,
    )?;
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..insert_at]);
    out.extend_from_slice(&moov);
    out.extend_from_slice(&bytes[insert_at..moov_start]);
    out.extend_from_slice(&bytes[moov_start + moov_len..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    /// ftyp, mdat, then moov with one stco entry pointing at `PAYLOAD`.
    fn sample(wide: bool) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"isom\0\0\0\0");
        let payload_at = ftyp.len() + 8;
        let mdat = bx(b"mdat", b"PAYLOAD");
        let table = if wide {
            let mut t = vec![0, 0, 0, 0, 0, 0, 0, 1];
            t.extend_from_slice(&(payload_at as u64).to_be_bytes());
            bx(b"co64", &t)
        } else {
            let mut t = vec![0, 0, 0, 0, 0, 0, 0, 1];
            t.extend_from_slice(&(payload_at as u32).to_be_bytes());
            bx(b"stco", &t)
        };
        let stbl = bx(b"stbl", &table);
        let minf = bx(b"minf", &stbl);
        let mdia = bx(b"mdia", &minf);
        let trak = bx(b"trak", &mdia);
        let moov = bx(b"moov", &trak);
        [ftyp, mdat, moov].concat()
    }

    fn payload_offset(file: &[u8], wide: bool) -> usize {
        let tag: &[u8] = if wide { b"co64" } else { b"stco" };
        let at = file.windows(4).position(|w| w == tag).unwrap() + 4 + 8;
        if wide {
            u64::from_be_bytes(file[at..at + 8].try_into().unwrap()) as usize
        } else {
            u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize
        }
    }

    #[test]
    fn moov_moves_first_and_offsets_follow_the_data() {
        for wide in [false, true] {
            let out = faststart(sample(wide)).expect("faststart");
            let order: Vec<[u8; 4]> = boxes(&out).unwrap().iter().map(|b| b.2).collect();
            assert_eq!(order, [*b"ftyp", *b"moov", *b"mdat"]);
            let at = payload_offset(&out, wide);
            assert_eq!(&out[at..at + 7], b"PAYLOAD");
        }
    }

    #[test]
    fn already_fast_or_not_mp4() {
        let fast = faststart(sample(false)).unwrap();
        assert_eq!(faststart(fast.clone()).unwrap(), fast);
        assert!(faststart(b"not an mp4 at all".to_vec()).is_none());
        assert!(faststart(Vec::new()).is_none());
    }
}
