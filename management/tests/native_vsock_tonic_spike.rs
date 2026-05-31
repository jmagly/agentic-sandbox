#![cfg(target_os = "linux")]

use std::env;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::{Stream, StreamExt, TryStreamExt};
use hyper::rt::{Read as HyperRead, ReadBufCursor, Write as HyperWrite};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_vsock::{VsockAddr, VsockListener, VsockStream, VMADDR_CID_LOCAL};
use tonic::transport::server::Connected;
use tonic::transport::{Endpoint, Server};
use tonic::{Request, Response, Status, Streaming};
use tower::service_fn;

pub mod spike {
    tonic::include_proto!("agentic.sandbox.spike.v1");
}

use spike::native_vsock_spike_client::NativeVsockSpikeClient;
use spike::native_vsock_spike_server::{NativeVsockSpike, NativeVsockSpikeServer};
use spike::SpikeFrame;

const RUN_ENV: &str = "AGENTIC_RUN_NATIVE_VSOCK_SPIKE";

#[derive(Clone, Copy, Debug)]
struct NativeVsockPeer {
    addr: Option<VsockAddr>,
}

#[derive(Debug)]
struct TonicVsockIo {
    inner: TokioIo<VsockStream>,
    peer: NativeVsockPeer,
}

impl TonicVsockIo {
    fn new(stream: VsockStream) -> Self {
        let peer = NativeVsockPeer {
            addr: stream.peer_addr().ok(),
        };
        Self {
            inner: TokioIo::new(stream),
            peer,
        }
    }
}

impl Connected for TonicVsockIo {
    type ConnectInfo = NativeVsockPeer;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.peer
    }
}

impl HyperRead for TonicVsockIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl HyperWrite for TonicVsockIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

impl AsyncRead for TonicVsockIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_read(cx, buf)
    }
}

impl AsyncWrite for TonicVsockIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.inner().is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(self.get_mut().inner.inner_mut()).poll_write_vectored(cx, bufs)
    }
}

#[derive(Debug, Default)]
struct EchoSpike;

#[tonic::async_trait]
impl NativeVsockSpike for EchoSpike {
    type BidiStream = Pin<Box<dyn Stream<Item = Result<SpikeFrame, Status>> + Send>>;

    async fn bidi(
        &self,
        request: Request<Streaming<SpikeFrame>>,
    ) -> Result<Response<Self::BidiStream>, Status> {
        let peer = request
            .extensions()
            .get::<NativeVsockPeer>()
            .and_then(|peer| peer.addr);
        let seen_peer_cid = peer.map(|addr| addr.cid()).unwrap_or(u32::MAX);
        let seen_peer_port = peer.map(|addr| addr.port()).unwrap_or(u32::MAX);

        let stream = request.into_inner().map(move |frame| {
            frame.map(|mut frame| {
                frame.seen_peer_cid = seen_peer_cid;
                frame.seen_peer_port = seen_peer_port;
                frame
            })
        });

        Ok(Response::new(Box::pin(stream)))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_vsock_tonic_bidi_stream_exposes_peer_cid() -> Result<(), Box<dyn std::error::Error>>
{
    if env::var_os(RUN_ENV).is_none() {
        eprintln!("skipping native AF_VSOCK spike; set {RUN_ENV}=1 to run");
        return Ok(());
    }

    let port = 45_000 + (std::process::id() % 1_000);
    let addr = VsockAddr::new(VMADDR_CID_LOCAL, port);
    let listener = VsockListener::bind(addr)?;
    let incoming = listener.incoming().map_ok(TonicVsockIo::new);

    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(NativeVsockSpikeServer::new(EchoSpike))
            .serve_with_incoming(incoming)
            .await
    });

    let endpoint = Endpoint::try_from("http://native-vsock-spike")?;
    let channel = endpoint
        .connect_with_connector(service_fn(move |_| async move {
            VsockStream::connect(addr).await.map(TonicVsockIo::new)
        }))
        .await?;
    let mut client = NativeVsockSpikeClient::new(channel);

    let outbound = tokio_stream::iter([SpikeFrame {
        payload: "native-vsock-tonic-ok".to_string(),
        seen_peer_cid: 0,
        seen_peer_port: 0,
    }]);

    let mut inbound = client.bidi(Request::new(outbound)).await?.into_inner();
    let frame = inbound
        .message()
        .await?
        .ok_or("server closed stream before echo")?;

    assert_eq!(frame.payload, "native-vsock-tonic-ok");
    assert_eq!(frame.seen_peer_cid, VMADDR_CID_LOCAL);
    assert_ne!(frame.seen_peer_port, u32::MAX);

    server.abort();
    Ok(())
}
