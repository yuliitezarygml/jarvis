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

/// FFmpeg only processes completed PCM. Native inference never invokes an interpreter.
pub async fn encode(data: Vec<u8>, mp3: bool, speed: f32) -> anyhow::Result<Vec<u8>> {
    use std::{process::Stdio, time::Duration};
    use tokio::{io::AsyncWriteExt, process::Command, sync::Semaphore};
    static SLOTS: Semaphore = Semaphore::const_new(2);
    let _permit = SLOTS.try_acquire()?;
    let mut cmd =
        Command::new(std::env::var_os("JARVIS_FFMPEG").unwrap_or_else(|| "ffmpeg".into()));
    cmd.args([
        "-nostdin",
        "-v",
        "error",
        "-threads",
        "1",
        "-f",
        "s16le",
        "-ar",
        "24000",
        "-ac",
        "1",
        "-i",
        "pipe:0",
        "-af",
        &format!("atempo={speed}"),
    ]);
    if mp3 {
        cmd.args(["-f", "mp3", "-c:a", "libmp3lame", "-b:a", "96k"]);
    } else {
        cmd.args(["-f", "s16le", "-c:a", "pcm_s16le"]);
    }
    let mut child = cmd
        .arg("pipe:1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15), async move {
        let writer = async move {
            stdin.write_all(&data).await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        tokio::try_join!(writer, child.wait_with_output())
    })
    .await??
    .1;
    anyhow::ensure!(result.status.success(), "audio encoding failed");
    Ok(if mp3 {
        result.stdout
    } else {
        wav(&result.stdout)
    })
}
