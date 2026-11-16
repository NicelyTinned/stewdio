//! LZ4 block compression/decompression (used by Roblox binary chunks).

pub fn decompress(src: &[u8], out_len: usize) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(out_len.min(1 << 28));
    let mut i = 0usize;
    while i < src.len() {
        let token = src[i];
        i += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            loop {
                let b = *src.get(i).ok_or("truncated literal length")?;
                i += 1;
                lit += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        if i + lit > src.len() {
            return Err("literal run overruns input".to_string());
        }
        if out.len() + lit > out_len {
            return Err("output larger than declared size".to_string());
        }
        out.extend_from_slice(&src[i..i + lit]);
        i += lit;
        if i >= src.len() {
            break;
        }
        if i + 2 > src.len() {
            return Err("truncated match offset".to_string());
        }
        let off = u16::from_le_bytes([src[i], src[i + 1]]) as usize;
        i += 2;
        if off == 0 || off > out.len() {
            return Err("invalid match offset".to_string());
        }
        let mut ml = (token & 15) as usize;
        if ml == 15 {
            loop {
                let b = *src.get(i).ok_or("truncated match length")?;
                i += 1;
                ml += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        ml += 4;
        if out.len() + ml > out_len {
            return Err("output larger than declared size".to_string());
        }
        let start = out.len() - off;
        for k in 0..ml {
            let b = out[start + k];
            out.push(b);
        }
    }
    if out.len() != out_len {
        return Err(format!("size mismatch: expected {out_len} bytes, got {}", out.len()));
    }
    Ok(out)
}

fn put_len(out: &mut Vec<u8>, mut n: usize) {
    while n >= 255 {
        out.push(255);
        n -= 255;
    }
    out.push(n as u8);
}

pub fn compress(src: &[u8]) -> Vec<u8> {
    let n = src.len();
    let mut out: Vec<u8> = Vec::with_capacity(n / 2 + 16);
    let mut table = vec![0u32; 1 << 16];
    let mut anchor = 0usize;
    let mut i = 0usize;
    if n >= 13 {
        while i < n - 12 {
            let seq = u32::from_le_bytes([src[i], src[i + 1], src[i + 2], src[i + 3]]);
            let h = (seq.wrapping_mul(2654435761) >> 16) as usize;
            let cand = table[h] as usize;
            table[h] = (i + 1) as u32;
            if cand != 0 {
                let c = cand - 1;
                if i - c <= 65535 && src[c..c + 4] == src[i..i + 4] {
                    let max_len = n - 5 - i;
                    let mut len = 4;
                    while len < max_len && src[c + len] == src[i + len] {
                        len += 1;
                    }
                    let lit = i - anchor;
                    out.push(((lit.min(15) as u8) << 4) | ((len - 4).min(15) as u8));
                    if lit >= 15 {
                        put_len(&mut out, lit - 15);
                    }
                    out.extend_from_slice(&src[anchor..i]);
                    out.extend_from_slice(&((i - c) as u16).to_le_bytes());
                    if len - 4 >= 15 {
                        put_len(&mut out, len - 4 - 15);
                    }
                    i += len;
                    anchor = i;
                    continue;
                }
            }
            i += 1;
        }
    }
    let lit = n - anchor;
    out.push((lit.min(15) as u8) << 4);
    if lit >= 15 {
        put_len(&mut out, lit - 15);
    }
    out.extend_from_slice(&src[anchor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut data = Vec::new();
        for i in 0..5000u32 {
            data.extend_from_slice(&(i % 37).to_le_bytes());
            data.extend_from_slice(b"stewdio stewdio stewdio");
        }
        let c = compress(&data);
        assert!(c.len() < data.len());
        assert_eq!(decompress(&c, data.len()).unwrap(), data);
        assert_eq!(decompress(&compress(b""), 0).unwrap(), b"");
        assert_eq!(decompress(&compress(b"abc"), 3).unwrap(), b"abc");
    }
}
