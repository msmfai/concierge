//! Reconstruct a `.dds` header for a `DX10` BA2 texture. BA2 stores only the
//! raw mip chunks + dimensions/format; a loadable `.dds` needs the 128-byte
//! `DDS_HEADER` plus the 20-byte `DDS_HEADER_DXT10` extension (used so the
//! DXGI format byte carries through unchanged), per the documented DDS layout.

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSD_LINEARSIZE: u32 = 0x8_0000;

const DDPF_FOURCC: u32 = 0x4;

const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_MIPMAP: u32 = 0x40_0000;

const DDSCAPS2_CUBEMAP_ALLFACES: u32 = 0xFE00;
const DDS_DIMENSION_TEXTURE2D: u32 = 3;
const DDS_RESOURCE_MISC_TEXTURECUBE: u32 = 0x4;

/// Build the `.dds` header (magic + `DDS_HEADER` + `DDS_HEADER_DXT10`). The
/// caller appends the concatenated mip data.
#[must_use]
pub fn header(width: u16, height: u16, num_mips: u8, dxgi_format: u8, is_cubemap: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(148);
    out.extend_from_slice(b"DDS ");

    let flags = DDSD_CAPS
        | DDSD_HEIGHT
        | DDSD_WIDTH
        | DDSD_PIXELFORMAT
        | DDSD_MIPMAPCOUNT
        | DDSD_LINEARSIZE;
    let mut caps = DDSCAPS_TEXTURE;
    if num_mips > 1 {
        caps |= DDSCAPS_MIPMAP | DDSCAPS_COMPLEX;
    }
    let caps2 = if is_cubemap {
        caps |= DDSCAPS_COMPLEX;
        DDSCAPS2_CUBEMAP_ALLFACES
    } else {
        0
    };

    // DDS_HEADER (124 bytes)
    push_u32(&mut out, 124); // dwSize
    push_u32(&mut out, flags);
    push_u32(&mut out, u32::from(height));
    push_u32(&mut out, u32::from(width));
    push_u32(&mut out, 0); // pitchOrLinearSize (loaders recompute for BC)
    push_u32(&mut out, 0); // depth
    push_u32(&mut out, u32::from(num_mips.max(1)));
    for _ in 0..11 {
        push_u32(&mut out, 0); // reserved1[11]
    }
    // DDS_PIXELFORMAT (32 bytes) — FourCC "DX10"
    push_u32(&mut out, 32); // dwSize
    push_u32(&mut out, DDPF_FOURCC);
    out.extend_from_slice(b"DX10"); // dwFourCC
    for _ in 0..5 {
        push_u32(&mut out, 0); // bit count + 4 masks
    }
    push_u32(&mut out, caps);
    push_u32(&mut out, caps2);
    push_u32(&mut out, 0); // caps3
    push_u32(&mut out, 0); // caps4
    push_u32(&mut out, 0); // reserved2

    // DDS_HEADER_DXT10 (20 bytes)
    push_u32(&mut out, u32::from(dxgi_format));
    push_u32(&mut out, DDS_DIMENSION_TEXTURE2D);
    push_u32(
        &mut out,
        if is_cubemap {
            DDS_RESOURCE_MISC_TEXTURECUBE
        } else {
            0
        },
    );
    push_u32(&mut out, 1); // arraySize
    push_u32(&mut out, 0); // miscFlags2

    out
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
