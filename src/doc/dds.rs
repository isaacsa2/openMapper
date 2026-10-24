//! The 8-bit alpha DDS OMSI keeps its ground painting masks in.

/// An uncompressed DDS with one alpha byte per texel (DDPF_ALPHA, mask 0xff), no mipmaps.
pub fn alpha8(size: usize, texels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + texels.len());
    let w = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(b"DDS ");
    w(&mut out, 124); // header size
    // (byte for byte the header of the masks OMSI's editor writes: caps, height, width and
    // pixel format flags, no pitch)
    w(&mut out, 0x1 | 0x2 | 0x4 | 0x1000);
    w(&mut out, size as u32); // height
    w(&mut out, size as u32); // width
    w(&mut out, 0); // pitch
    w(&mut out, 0); // depth
    w(&mut out, 0); // mip count
    for _ in 0..11 {
        w(&mut out, 0);
    }
    // pixel format
    w(&mut out, 32);
    w(&mut out, 0x2); // DDPF_ALPHA
    w(&mut out, 0); // fourcc
    w(&mut out, 8); // bits
    w(&mut out, 0);
    w(&mut out, 0);
    w(&mut out, 0);
    w(&mut out, 0xff); // alpha mask
    w(&mut out, 0x1000 | 0x2); // caps: texture, alpha
    for _ in 0..4 {
        w(&mut out, 0);
    }
    out.extend_from_slice(&texels[..size * size]);
    out
}

/// An uncompressed 32-bit DDS (A8R8G8B8, as Direct3D lays it out: blue first), no mipmaps:
/// a texture exported for tools that want DDS.
pub fn rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + rgba.len());
    let w = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(b"DDS ");
    w(&mut out, 124);
    w(&mut out, 0x1 | 0x2 | 0x4 | 0x1000 | 0x8);
    w(&mut out, height);
    w(&mut out, width);
    w(&mut out, width * 4);
    w(&mut out, 0);
    w(&mut out, 0);
    for _ in 0..11 {
        w(&mut out, 0);
    }
    w(&mut out, 32);
    w(&mut out, 0x40 | 0x1); // DDPF_RGB | DDPF_ALPHAPIXELS
    w(&mut out, 0);
    w(&mut out, 32);
    w(&mut out, 0x00ff_0000);
    w(&mut out, 0x0000_ff00);
    w(&mut out, 0x0000_00ff);
    w(&mut out, 0xff00_0000);
    w(&mut out, 0x1000);
    for _ in 0..4 {
        w(&mut out, 0);
    }
    for px in rgba.chunks(4).take((width * height) as usize) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_rgba_dds_reads_back_through_openomsi() {
        let px: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 7 % 256) as u8).collect();
        let bytes = super::rgba(4, 4, &px);
        let img = omsi_texture::dds::decode(&bytes).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(&img.rgba[..8], &px[..8]);
    }

    #[test]
    fn reads_back_through_openomsi() {
        let size = 16;
        let texels: Vec<u8> = (0..size * size).map(|i| (i % 251) as u8).collect();
        let bytes = super::alpha8(size, &texels);
        let img = omsi_texture::decode_bytes(&bytes, std::path::Path::new("m.dds")).unwrap();
        assert_eq!((img.width, img.height), (16, 16));
        assert_eq!(img.rgba[4 * 5 + 3], texels[5]);
    }
}
