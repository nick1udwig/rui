//! Maintained tungstenite framing with an upgrade-time parser allocation ceiling.
//! A bounded HTTP header prefix is parsed by httparse and replayed unchanged to
//! tungstenite, which validates the WebSocket handshake and all frame semantics.
use crate::{proto, TransportError};
use futures_util::{Sink, SinkExt, StreamExt};
use prost::Message as ProstMessage;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig, Message},
    WebSocketStream,
};
use zeroize::Zeroize;
const HEADER_LIMIT: usize = 8192;
pub(crate) const MIN_MESSAGE: usize = 512;
pub(crate) const LIMIT_HEADER: &str = "x-rui-envelope-limit";
fn config(ceiling: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(4096.min(ceiling))
        .write_buffer_size(0)
        .max_write_buffer_size(ceiling.saturating_mul(2).saturating_add(128))
        .max_message_size(Some(ceiling))
        .max_frame_size(Some(ceiling))
        .accept_unmasked_frames(false)
}
pub(crate) struct Replay<S> {
    inner: S,
    header: Vec<u8>,
    position: usize,
}
impl<S: AsyncRead + Unpin> AsyncRead for Replay<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.position < self.header.len() {
            let count = buf.remaining().min(self.header.len() - self.position);
            buf.put_slice(&self.header[self.position..self.position + count]);
            self.position += count;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for Replay<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
#[allow(clippy::result_large_err)] // Required tungstenite callback error type.
pub(crate) async fn server_upgrade<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    maximum: usize,
    allowed_browser_origins: &[String],
) -> Result<(WebSocketStream<Replay<S>>, usize), TransportError> {
    let mut header = Vec::with_capacity(1024);
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= HEADER_LIMIT {
            return Err(TransportError::Limit);
        }
        header.push(stream.read_u8().await.map_err(|_| TransportError::Io)?);
    }
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut request = httparse::Request::new(&mut headers);
    if !request
        .parse(&header)
        .map_err(|_| TransportError::Protocol)?
        .is_complete()
    {
        return Err(TransportError::Protocol);
    }
    let values: Vec<_> = request
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(LIMIT_HEADER))
        .collect();
    let origins: Vec<_> = request
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("origin"))
        .collect();
    let browser = match origins.as_slice() {
        [] => false,
        [origin]
            if allowed_browser_origins
                .iter()
                .any(|allowed| allowed.as_bytes() == origin.value) =>
        {
            true
        }
        _ => return Err(TransportError::Authentication),
    };
    let requested = match values.as_slice() {
        [] if browser => maximum,
        [value] => std::str::from_utf8(value.value)
            .map_err(|_| TransportError::Protocol)?
            .parse::<usize>()
            .map_err(|_| TransportError::Protocol)?,
        _ => return Err(TransportError::Protocol),
    };
    let ceiling = requested.min(maximum);
    if ceiling < MIN_MESSAGE {
        return Err(TransportError::Limit);
    }
    let replay = Replay {
        inner: stream,
        header,
        position: 0,
    };
    let callback =
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
         mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            if request.uri().path() != "/rui"
                || (!browser && request.headers().contains_key("sec-websocket-extensions"))
                || request
                    .headers()
                    .get("sec-websocket-protocol")
                    .is_none_or(|v| v != "rui.v1")
            {
                let mut error =
                    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(
                        "unsupported websocket upgrade".into(),
                    ));
                *error.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::BAD_REQUEST;
                return Err(error);
            }
            response.headers_mut().insert(
                LIMIT_HEADER,
                ceiling.to_string().parse().expect("numeric header"),
            );
            response.headers_mut().insert(
                "sec-websocket-protocol",
                "rui.v1".parse().expect("static header"),
            );
            Ok(response)
        };
    let socket =
        tokio_tungstenite::accept_hdr_async_with_config(replay, callback, Some(config(ceiling)))
            .await
            .map_err(|_| TransportError::WebSocket)?;
    Ok((socket, ceiling))
}
pub(crate) async fn client_upgrade<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    address: std::net::SocketAddr,
    name: &str,
    ceiling: usize,
) -> Result<(WebSocketStream<ResponseGuard<S>>, usize), TransportError> {
    let mut request = format!("wss://{name}:{}/rui", address.port())
        .into_client_request()
        .map_err(|_| TransportError::Protocol)?;
    request.headers_mut().insert(
        LIMIT_HEADER,
        ceiling
            .to_string()
            .parse()
            .map_err(|_| TransportError::Protocol)?,
    );
    request.headers_mut().insert(
        "sec-websocket-protocol",
        "rui.v1".parse().expect("static header"),
    );
    let (socket, response) = tokio_tungstenite::client_async_with_config(
        request,
        ResponseGuard {
            inner: stream,
            read: 0,
            tail: [0; 4],
            complete: false,
        },
        Some(config(ceiling)),
    )
    .await
    .map_err(|_| TransportError::WebSocket)?;
    if response
        .headers()
        .get("sec-websocket-protocol")
        .is_none_or(|v| v != "rui.v1")
        || response.headers().contains_key("sec-websocket-extensions")
    {
        return Err(TransportError::Protocol);
    }
    let selected = response
        .headers()
        .get(LIMIT_HEADER)
        .ok_or(TransportError::Protocol)?
        .to_str()
        .map_err(|_| TransportError::Protocol)?
        .parse::<usize>()
        .map_err(|_| TransportError::Protocol)?;
    if selected < MIN_MESSAGE || selected > ceiling {
        return Err(TransportError::Limit);
    }
    Ok((socket, selected))
}
pub(crate) async fn receive<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
) -> Result<Vec<u8>, TransportError> {
    loop {
        match socket
            .next()
            .await
            .ok_or(TransportError::Closed)?
            .map_err(|_| TransportError::WebSocket)?
        {
            Message::Binary(bytes) => return Ok(bytes.to_vec()),
            Message::Close(_) => return Err(TransportError::Closed),
            Message::Ping(_) | Message::Pong(_) => {}
            _ => return Err(TransportError::Protocol),
        }
    }
}
pub(crate) async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    mut packet: proto::Packet,
    ceiling: usize,
    deadline: Duration,
) -> Result<(), TransportError> {
    if packet.encoded_len() > ceiling {
        return Err(TransportError::Limit);
    }
    let bytes = packet.encode_to_vec();
    packet.payload.zeroize();
    send_bytes(socket, bytes, deadline).await
}
pub(crate) async fn send_bytes<S>(
    socket: &mut S,
    bytes: Vec<u8>,
    deadline: Duration,
) -> Result<(), TransportError>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    tokio::time::timeout(deadline, socket.send(Message::Binary(bytes.into())))
        .await
        .map_err(|_| TransportError::Deadline)?
        .map_err(|_| TransportError::WebSocket)
}

/// Bound a potentially malicious pinned host's HTTP response before the
/// maintained handshake parser allocates its complete headers.
pub(crate) struct ResponseGuard<S> {
    inner: S,
    read: usize,
    tail: [u8; 4],
    complete: bool,
}
impl<S: AsyncRead + Unpin> AsyncRead for ResponseGuard<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if self.complete {
            return Pin::new(&mut self.inner).poll_read(cx, buf);
        }
        let maximum = buf.remaining().min(HEADER_LIMIT.saturating_sub(self.read));
        if maximum == 0 {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "response headers exceed ceiling",
            )));
        }
        let count = {
            let target = buf.initialize_unfilled_to(maximum);
            let mut limited = ReadBuf::new(target);
            match Pin::new(&mut self.inner).poll_read(cx, &mut limited) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {}
            }
            for byte in limited.filled() {
                self.tail.rotate_left(1);
                self.tail[3] = *byte;
                if self.tail == *b"\r\n\r\n" {
                    self.complete = true;
                }
            }
            limited.filled().len()
        };
        self.read += count;
        buf.advance(count);
        Poll::Ready(Ok(()))
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for ResponseGuard<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn client_response_headers_are_bounded_before_handshake_parse() {
        let (mut writer, reader) = tokio::io::duplex(16384);
        writer
            .write_all(&vec![b'x'; HEADER_LIMIT + 1])
            .await
            .unwrap();
        let mut guarded = ResponseGuard {
            inner: reader,
            read: 0,
            tail: [0; 4],
            complete: false,
        };
        let mut bytes = [0; HEADER_LIMIT];
        guarded.read_exact(&mut bytes).await.unwrap();
        assert_eq!(
            guarded.read_u8().await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
    #[tokio::test]
    async fn oversized_frame_is_rejected_from_header_without_body() {
        let (mut peer, reader) = tokio::io::duplex(128);
        let mut socket = WebSocketStream::from_raw_socket(
            reader,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            Some(config(512)),
        )
        .await;
        // Masked binary frame announces 1 MiB but supplies no body.
        peer.write_all(&[0x82, 0xff, 0, 0, 0, 0, 0, 0x10, 0, 0, 1, 2, 3, 4])
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), receive(&mut socket))
            .await
            .unwrap();
        assert_eq!(result, Err(TransportError::WebSocket));
    }
    #[tokio::test]
    async fn compression_extension_and_text_are_rejected() {
        let (mut client, server) = tokio::io::duplex(4096);
        let handler = tokio::spawn(server_upgrade(server, 512, &[]));
        client.write_all(b"GET /rui HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: rui.v1\r\nSec-WebSocket-Extensions: permessage-deflate\r\nx-rui-envelope-limit: 512\r\n\r\n").await.unwrap();
        assert!(handler.await.unwrap().is_err());
        let (peer, reader) = tokio::io::duplex(4096);
        let mut sender = WebSocketStream::from_raw_socket(
            peer,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            Some(config(512)),
        )
        .await;
        let mut socket = WebSocketStream::from_raw_socket(
            reader,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            Some(config(512)),
        )
        .await;
        sender
            .send(Message::Text("no-json-transport".into()))
            .await
            .unwrap();
        assert_eq!(receive(&mut socket).await, Err(TransportError::Protocol));
    }
}

#[cfg(test)]
mod subprotocol_test {
    use super::*;
    #[tokio::test]
    #[allow(clippy::result_large_err)] // Required tungstenite callback error type.
    async fn client_requires_exact_returned_subprotocol() {
        let (client, server) = tokio::io::duplex(16384);
        let remote = tokio::spawn(async move {
            let socket=tokio_tungstenite::accept_hdr_async(server,|_:&tokio_tungstenite::tungstenite::handshake::server::Request,mut response:tokio_tungstenite::tungstenite::handshake::server::Response| {
            response.headers_mut().insert(LIMIT_HEADER,"512".parse().unwrap());response.headers_mut().insert("sec-websocket-protocol","other.v1".parse().unwrap());Ok(response)
        }).await;
            drop(socket);
        });
        assert!(
            client_upgrade(client, "127.0.0.1:443".parse().unwrap(), "localhost", 512)
                .await
                .is_err()
        );
        remote.await.unwrap();
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;
    async fn upgrade(origin: Option<&str>, allowed: Vec<String>) -> bool {
        let (client, server) = tokio::io::duplex(8192);
        let host = tokio::spawn(async move { server_upgrade(server, 512, &allowed).await });
        let mut request = "wss://localhost/rui".into_client_request().unwrap();
        request
            .headers_mut()
            .insert("sec-websocket-protocol", "rui.v1".parse().unwrap());
        request.headers_mut().insert(
            "sec-websocket-extensions",
            "permessage-deflate; client_max_window_bits"
                .parse()
                .unwrap(),
        );
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("origin", origin.parse().unwrap());
        }
        let result = tokio_tungstenite::client_async(request, client).await;
        match result {
            Ok((_socket, response)) => {
                assert!(!response.headers().contains_key("sec-websocket-extensions"));
                assert_eq!(
                    response.headers().get("sec-websocket-protocol").unwrap(),
                    "rui.v1"
                );
                assert_eq!(host.await.unwrap().unwrap().1, 512);
                true
            }
            Err(_) => {
                assert!(host.await.unwrap().is_err());
                false
            }
        }
    }
    #[tokio::test]
    async fn browser_origin_is_exact_opt_in_and_compression_is_never_selected() {
        let allow = vec!["http://localhost:5173".into()];
        assert!(upgrade(Some("http://localhost:5173"), allow.clone()).await);
        assert!(!upgrade(Some("http://localhost:5173"), vec![]).await);
        assert!(!upgrade(Some("http://localhost.evil:5173"), allow.clone()).await);
        assert!(!upgrade(Some("http://localhost:5174"), allow.clone()).await);
        assert!(!upgrade(None, allow).await);
    }
}
