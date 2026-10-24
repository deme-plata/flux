pub mod obfs4;
pub mod direct;

use crate::{Result, IronTunnelError, Transport};
use tokio::net::TcpStream;
use tokio::io::{AsyncRead, AsyncWrite};
use std::pin::Pin;
use std::task::{Context, Poll};
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    pub transport_type: Transport,
    pub obfs4_cert: Option<String>,
    pub obfs4_iat_mode: u32,
    pub buffer_size: usize,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            transport_type: Transport::Direct,
            obfs4_cert: None,
            obfs4_iat_mode: 0,
            buffer_size: 4096,
        }
    }
}

pub trait TransportStream: AsyncRead + AsyncWrite + Send + Sync + Unpin {
    fn transport_type(&self) -> Transport;
    fn is_obfuscated(&self) -> bool;
}

pub struct TransportManager {
    config: TransportConfig,
}

impl TransportManager {
    pub fn new(config: TransportConfig) -> Self {
        Self { config }
    }
    
    pub async fn wrap_stream(&self, stream: TcpStream) -> Result<Box<dyn TransportStream>> {
        match self.config.transport_type {
            Transport::Direct => {
                Ok(Box::new(DirectTransport::new(stream)))
            }
            Transport::Obfs4 => {
                let obfs4_stream = Obfs4Transport::new(
                    stream,
                    self.config.obfs4_cert.as_deref(),
                    self.config.obfs4_iat_mode,
                ).await?;
                Ok(Box::new(obfs4_stream))
            }
        }
    }
}

pub struct DirectTransport {
    inner: TcpStream,
}

impl DirectTransport {
    pub fn new(stream: TcpStream) -> Self {
        Self { inner: stream }
    }
}

impl TransportStream for DirectTransport {
    fn transport_type(&self) -> Transport {
        Transport::Direct
    }
    
    fn is_obfuscated(&self) -> bool {
        false
    }
}

impl AsyncRead for DirectTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for DirectTransport {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub struct Obfs4Transport {
    inner: TcpStream,
    obfuscator: Obfs4Obfuscator,
    read_buffer: BytesMut,
    write_buffer: BytesMut,
}

impl Obfs4Transport {
    pub async fn new(
        stream: TcpStream,
        cert: Option<&str>,
        iat_mode: u32,
    ) -> Result<Self> {
        let obfuscator = Obfs4Obfuscator::new(cert, iat_mode)?;
        
        Ok(Self {
            inner: stream,
            obfuscator,
            read_buffer: BytesMut::with_capacity(4096),
            write_buffer: BytesMut::with_capacity(4096),
        })
    }
}

impl TransportStream for Obfs4Transport {
    fn transport_type(&self) -> Transport {
        Transport::Obfs4
    }
    
    fn is_obfuscated(&self) -> bool {
        true
    }
}

impl AsyncRead for Obfs4Transport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.read_buffer.is_empty() {
            let mut temp_buf = [0u8; 4096];
            let mut read_buf = tokio::io::ReadBuf::new(&mut temp_buf);
            
            match Pin::new(&mut self.inner).poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => {
                    let filled = read_buf.filled();
                    if !filled.is_empty() {
                        match self.obfuscator.deobfuscate(filled) {
                            Ok(deobfuscated) => {
                                self.read_buffer.extend_from_slice(&deobfuscated);
                            }
                            Err(_) => {
                                return Poll::Ready(Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "Deobfuscation failed",
                                )));
                            }
                        }
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        
        let to_copy = std::cmp::min(buf.remaining(), self.read_buffer.len());
        if to_copy > 0 {
            buf.put_slice(&self.read_buffer.split_to(to_copy));
        }
        
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Obfs4Transport {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let obfuscated = match self.obfuscator.obfuscate(buf) {
            Ok(data) => data,
            Err(_) => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Obfuscation failed",
                )));
            }
        };
        
        self.write_buffer.extend_from_slice(&obfuscated);
        
        match Pin::new(&mut self.inner).poll_write(cx, &self.write_buffer) {
            Poll::Ready(Ok(written)) => {
                self.write_buffer.advance(written);
                Poll::Ready(Ok(buf.len()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
    
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        if !self.write_buffer.is_empty() {
            match Pin::new(&mut self.inner).poll_write(cx, &self.write_buffer) {
                Poll::Ready(Ok(written)) => {
                    self.write_buffer.advance(written);
                    if !self.write_buffer.is_empty() {
                        return Poll::Pending;
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

use obfs4::Obfs4Obfuscator;