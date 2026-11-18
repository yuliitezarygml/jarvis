use bytes::{BufMut, Bytes};
pub const HZ: usize = 24000;
pub fn pcm(frame: &[f32]) -> anyhow::Result<Bytes> {
    let mut data = Vec::with_capacity(frame.len() * 2);
    for &v in frame {
        anyhow::ensure!(v.is_finite(), "non-finite model output");
        data.put_i16_le((v.clamp(-1., 1.) * 32767.) as i16);
    }
    Ok(data.into())
}
pub fn wav(pcm: &[u8]) -> Vec<u8> {
    let n = pcm.len() as u32;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend(b"RIFF");
    out.put_u32_le(n + 36);
    out.extend(b"WAVEfmt ");
    out.put_u32_le(16);
    out.put_u16_le(1);
    out.put_u16_le(1);
    out.put_u32_le(HZ as u32);
    out.put_u32_le(2 * HZ as u32);
    out.put_u16_le(2);
    out.put_u16_le(16);
    out.extend(b"data");
    out.put_u32_le(n);
    out.extend(pcm);
    out
}
