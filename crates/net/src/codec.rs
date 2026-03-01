use anyhow::{Context, Result};
use bytes::{BufMut, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use nexusfs_proto::MessageEnvelope;

/// Send a framed MessageEnvelope: len_u32_le + postcard(payload)
pub async fn send_msg<W>(w: &mut W, env: &MessageEnvelope) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let payload = postcard::to_stdvec(env).context("postcard encode envelope")?;
    let mut buf = BytesMut::with_capacity(4 + payload.len());
    buf.put_u32_le(payload.len() as u32);
    buf.extend_from_slice(&payload);
    w.write_all(&buf).await.context("write frame")?;
    Ok(())
}

/// Receive a framed MessageEnvelope.
pub async fn recv_msg<R>(r: &mut R, max_len: usize) -> Result<MessageEnvelope>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await.context("read len")?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > max_len {
        anyhow::bail!("frame too large: {} > {}", len, max_len);
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await.context("read payload")?;
    let env: MessageEnvelope =
        postcard::from_bytes(&payload).context("postcard decode envelope")?;
    Ok(env)
}
