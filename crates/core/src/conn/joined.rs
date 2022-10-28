//! JoinListener and it's implements.
use std::io::{self, Result as IoResult};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::async_trait;
use crate::conn::Holding;
use crate::conn::HttpBuilders;
use crate::http::{HttpConnection, Version};
use crate::service::HyperHandler;

use super::{Accepted, Acceptor, Listener};

/// A I/O stream for JoinedListener.
pub enum JoinedStream<A, B> {
    #[allow(missing_docs)]
    A(A),
    #[allow(missing_docs)]
    B(B),
}

impl<A, B> AsyncRead for JoinedStream<A, B>
where
    A: AsyncRead + Send + Unpin + 'static,
    B: AsyncRead + Send + Unpin + 'static,
{
    #[inline]
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut() {
            JoinedStream::A(a) => Pin::new(a).poll_read(cx, buf),
            JoinedStream::B(b) => Pin::new(b).poll_read(cx, buf),
        }
    }
}

impl<A, B> AsyncWrite for JoinedStream<A, B>
where
    A: AsyncWrite + Send + Unpin + 'static,
    B: AsyncWrite + Send + Unpin + 'static,
{
    #[inline]
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match &mut self.get_mut() {
            JoinedStream::A(a) => Pin::new(a).poll_write(cx, buf),
            JoinedStream::B(b) => Pin::new(b).poll_write(cx, buf),
        }
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut() {
            JoinedStream::A(a) => Pin::new(a).poll_flush(cx),
            JoinedStream::B(b) => Pin::new(b).poll_flush(cx),
        }
    }

    #[inline]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut() {
            JoinedStream::A(a) => Pin::new(a).poll_shutdown(cx),
            JoinedStream::B(b) => Pin::new(b).poll_shutdown(cx),
        }
    }
}

/// JoinedListener
#[pin_project]
pub struct JoinedListener<A, B> {
    #[pin]
    a: A,
    #[pin]
    b: B,
}

impl<A, B> JoinedListener<A, B> {
    /// Create a new `JoinedListener`.
    #[inline]
    pub fn new(a: A, b: B) -> Self {
        JoinedListener { a, b }
    }
}
#[async_trait]
impl<A, B> Listener for JoinedListener<A, B>
where
    A: Listener + Send + Unpin + 'static,
    B: Listener + Send + Unpin + 'static,
    A::Acceptor: Acceptor + Send + Unpin + 'static,
    B::Acceptor: Acceptor + Send + Unpin + 'static,
{
    type Acceptor = JoinedAcceptor<A::Acceptor, B::Acceptor>;

    async fn bind(self) -> Self::Acceptor {
        self.try_bind().await.unwrap()
    }

    async fn try_bind(self) -> IoResult<Self::Acceptor> {
        let a = self.a.try_bind().await?;
        let b = self.b.try_bind().await?;
        let holdings = a.holdings().iter().chain(b.holdings().iter()).cloned().collect();
        Ok(JoinedAcceptor { a, b, holdings })
    }
}

pub struct JoinedAcceptor<A, B> {
    a: A,
    b: B,
    holdings: Vec<Holding>,
}

#[async_trait]
impl<A, B> HttpConnection for JoinedStream<A, B>
where
    A: HttpConnection + Send,
    B: HttpConnection + Send,
{
    async fn version(&mut self) -> Option<Version> {
        match self {
            JoinedStream::A(a) => a.version().await,
            JoinedStream::B(b) => b.version().await,
        }
    }
    async fn serve(self, handler: HyperHandler, builders: Arc<HttpBuilders>) -> IoResult<()> {
        match self {
            JoinedStream::A(a) => a.serve(handler, builders).await,
            JoinedStream::B(b) => b.serve(handler, builders).await,
        }
    }
}

#[async_trait]
impl<A, B> Acceptor for JoinedAcceptor<A, B>
where
    A: Acceptor + Send + Unpin + 'static,
    B: Acceptor + Send + Unpin + 'static,
    A::Conn: HttpConnection + AsyncRead + AsyncWrite + Send + Unpin + 'static,
    B::Conn: HttpConnection + AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    type Conn = JoinedStream<A::Conn, B::Conn>;

    #[inline]
    fn holdings(&self) -> &[Holding] {
        &self.holdings
    }

    #[inline]
    async fn accept(&mut self) -> IoResult<Accepted<Self::Conn>> {
        tokio::select! {
            accepted = self.a.accept() => {
                Ok(accepted?.map_conn(JoinedStream::A))
            }
            accepted = self.b.accept() => {
                Ok(accepted?.map_conn(JoinedStream::B))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{ AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;
    use crate::conn::TcpListener;

    #[tokio::test]
    async fn test_joined_listener() {
        let addr1 = std::net::SocketAddr::from(([127, 0, 0, 1], 6978));
        let addr2 = std::net::SocketAddr::from(([127, 0, 0, 1], 6979));

        let mut acceptor = TcpListener::new(addr1).join(TcpListener::new(addr2)).bind().await;
        tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr1).await.unwrap();
            stream.write_i32(50).await.unwrap();

            let mut stream = TcpStream::connect(addr2).await.unwrap();
            stream.write_i32(100).await.unwrap();
        });
        let Accepted { mut conn, .. } = acceptor.accept().await.unwrap();
        let first = conn.read_i32().await.unwrap();
        let Accepted { mut conn, .. } = acceptor.accept().await.unwrap();
        let second = conn.read_i32().await.unwrap();
        assert_eq!(first + second, 150);
    }
}