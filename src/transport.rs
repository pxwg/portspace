//! Propagate transport EOF/errors to workspace request cancellation.
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::sync::CancellationToken;

pub struct DisconnectReader<R> {
    reader: R,
    disconnected: CancellationToken,
}
impl<R> DisconnectReader<R> {
    pub fn new(reader: R, disconnected: CancellationToken) -> Self {
        Self {
            reader,
            disconnected,
        }
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for DisconnectReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let remaining = buf.remaining();
        let result = Pin::new(&mut self.reader).poll_read(cx, buf);
        if matches!(&result, Poll::Ready(Err(_)))
            || (matches!(&result, Poll::Ready(Ok(())))
                && remaining > 0
                && buf.filled().len() == before)
        {
            self.disconnected.cancel();
        }
        result
    }
}
impl<R> Drop for DisconnectReader<R> {
    fn drop(&mut self) {
        self.disconnected.cancel();
    }
}
