use std::{cmp, io};

use bytes::{BufMut, Bytes, BytesMut};
use http::{
    header::{CONNECTION, CONTENT_LENGTH, DATE, TRANSFER_ENCODING},
    response::Parts,
    StatusCode, Version,
};
use log::{debug, warn};

use crate::body::{ResponseBody, ResponseBodySize};
use crate::util::date::DATE_VALUE_LENGTH;

use super::buf::{EncodedBuf, WriteBuf};
use super::codec::Kind;
use super::context::{ConnectionType, Context};
use super::error::{Parse, ProtoError};

impl Context<'_> {
    pub(super) fn encode_continue<const WRITE_BUF_LIMIT: usize>(&mut self, buf: &mut WriteBuf<WRITE_BUF_LIMIT>) {
        debug_assert!(self.is_expect_header());
        match *buf {
            WriteBuf::Flat(ref mut bytes) => bytes.put_slice(b"HTTP/1.1 100 Continue\r\n\r\n"),
            WriteBuf::List(ref mut list) => {
                list.buffer(EncodedBuf::Static(b"HTTP/1.1 100 Continue\r\n\r\n"));
            }
        }
    }

    pub(super) fn encode_head<const WRITE_BUF_LIMIT: usize>(
        &mut self,
        parts: Parts,
        size: ResponseBodySize,
        buf: &mut WriteBuf<WRITE_BUF_LIMIT>,
    ) -> Result<(), ProtoError> {
        match *buf {
            WriteBuf::List(ref mut list) => {
                let buf = list.buf_mut();

                self.encode_head_inner(parts, size, buf)?;

                let bytes = buf.split().freeze();
                list.list_mut().push(EncodedBuf::Buf(bytes));

                Ok(())
            }
            WriteBuf::Flat(ref mut buf) => self.encode_head_inner(parts, size, buf),
        }
    }

    fn encode_head_inner(
        &mut self,
        mut parts: Parts,
        size: ResponseBodySize,
        buf: &mut BytesMut,
    ) -> Result<(), ProtoError> {
        let version = parts.version;
        let status = parts.status;

        // decide if content-length or transfer-encoding header would be skipped.
        let mut skip_len = match (status, version) {
            (StatusCode::SWITCHING_PROTOCOLS, _) => false,
            // Sending content-length or transfer-encoding header on 2xx response
            // to CONNECT is forbidden in RFC 7231.
            (s, _) if self.is_connect_method() && s.is_success() => true,
            (s, _) if s.is_informational() => {
                warn!("response with 1xx status code not supported");
                return Err(ProtoError::Parse(Parse::StatusCode));
            }
            _ => false,
        };

        // In some error cases, we don't know about the invalid message until already
        // pushing some bytes onto the `buf`. In those cases, we don't want to send
        // the half-pushed message, so rewind to before.
        // let orig_len = buf.len();

        // encode version, status code and reason
        encode_version_status_reason(buf, version, status);

        let mut skip_date = false;

        for (name, value) in parts.headers.drain() {
            let name = name.expect("Handling optional header name is not implemented");

            // TODO: more spec check needed. the current check barely does anything.
            match name {
                CONTENT_LENGTH => {
                    debug_assert!(!skip_len, "CONTENT_LENGTH header can not be set");
                    skip_len = true;
                }
                TRANSFER_ENCODING => {
                    debug_assert!(!skip_len, "TRANSFER_ENCODING header can not be set");
                    skip_len = true;
                }
                CONNECTION if self.is_force_close() => continue,
                CONNECTION => {
                    for val in value.to_str().map_err(|_| Parse::HeaderValue)?.split(',') {
                        let val = val.trim();

                        if val.eq_ignore_ascii_case("close") {
                            self.set_ctype(ConnectionType::Close);
                        } else if val.eq_ignore_ascii_case("keep-alive") {
                            self.set_ctype(ConnectionType::KeepAlive);
                        } else if val.eq_ignore_ascii_case("upgrade") {
                            self.set_ctype(ConnectionType::Upgrade);
                        }
                    }
                }
                DATE => skip_date = true,
                _ => {}
            }

            buf.put_slice(name.as_str().as_bytes());
            buf.put_slice(b": ");
            buf.put_slice(value.as_bytes());
            buf.put_slice(b"\r\n");
        }

        if self.is_force_close() {
            buf.put_slice(b"connection: close\r\n");
        }

        // encode transfer-encoding or content-length
        if !skip_len {
            match size {
                ResponseBodySize::None => {}
                ResponseBodySize::Stream => buf.put_slice(b"transfer-encoding: chunked\r\n"),
                ResponseBodySize::Sized(size) => {
                    let mut buffer = itoa::Buffer::new();
                    buf.put_slice(b"content-length: ");
                    buf.put_slice(buffer.format(size).as_bytes());
                    buf.put_slice(b"\r\n");
                }
            }
        }

        // set date header if there is not any.
        if !skip_date {
            buf.reserve(DATE_VALUE_LENGTH + 8);
            buf.put_slice(b"date: ");
            buf.put_slice(self.date.get().date());
            buf.put_slice(b"\r\n\r\n");
        } else {
            buf.put_slice(b"\r\n");
        }

        // put header map back to cache.
        self.header_cache = Some(parts.headers);

        Ok(())
    }
}

fn encode_version_status_reason<B: BufMut>(buf: &mut B, version: Version, status: StatusCode) {
    // encode version, status code and reason
    match (version, status) {
        // happy path shortcut.
        (Version::HTTP_11, StatusCode::OK) => {
            buf.put_slice(b"HTTP/1.1 200 OK\r\n");
            return;
        }
        (Version::HTTP_10, _) => {
            buf.put_slice(b"HTTP/1.0 ");
        }
        (Version::HTTP_11, _) => {
            buf.put_slice(b"HTTP/1.1 ");
        }
        _ => {
            debug!("response with unexpected response version");
            buf.put_slice(b"HTTP/1.1 ");
        }
    }

    buf.put_slice(status.as_str().as_bytes());
    buf.put_slice(b" ");
    // a reason MUST be written, as many parsers will expect it.
    buf.put_slice(status.canonical_reason().unwrap_or("<none>").as_bytes());
    buf.put_slice(b"\r\n");
}

impl<B> ResponseBody<B> {
    /// `TransferEncoding` must match the behavior of `Stream` impl of `ResponseBody`.
    /// Which means when `Stream::poll_next` returns Some(`Stream::Item`) the encoding
    /// must be able to encode data. And when it returns `None` it must valid to encode
    /// eof which would finish the encoding.
    pub(super) fn encoder(&self, ctype: ConnectionType) -> TransferEncoding {
        match *self {
            // None body would return None on first poll of ResponseBody as Stream.
            // an eof encoding would return Ok(()) afterward.
            Self::None => TransferEncoding::eof(),
            // Empty bytes would return None on first poll of ResponseBody as Stream.
            // A length encoding would see the remainning length is 0 and return Ok(()).
            Self::Bytes { ref bytes, .. } => TransferEncoding::length(bytes.len() as u64),
            Self::Stream { .. } => {
                if ctype == ConnectionType::Upgrade {
                    TransferEncoding::plain_chunked()
                } else {
                    TransferEncoding::chunked()
                }
            }
        }
    }
}

/// Encoders to handle different Transfer-Encodings.
#[derive(Debug)]
pub(super) struct TransferEncoding {
    kind: Kind,
}

impl TransferEncoding {
    #[inline(always)]
    pub(super) fn eof() -> TransferEncoding {
        TransferEncoding { kind: Kind::Eof }
    }

    #[inline(always)]
    pub(super) fn chunked() -> TransferEncoding {
        TransferEncoding {
            kind: Kind::EncodeChunked(false),
        }
    }

    #[inline(always)]
    pub(super) fn plain_chunked() -> TransferEncoding {
        TransferEncoding {
            kind: Kind::PlainChunked,
        }
    }

    #[inline(always)]
    pub(super) fn length(len: u64) -> TransferEncoding {
        TransferEncoding {
            kind: Kind::Length(len),
        }
    }

    /// Encode message. Return `EOF` state of encoder
    #[inline(always)]
    pub(super) fn encode<const WRITE_BUF_LIMIT: usize>(
        &mut self,
        mut msg: Bytes,
        buf: &mut WriteBuf<WRITE_BUF_LIMIT>,
    ) -> io::Result<bool> {
        match self.kind {
            Kind::Eof | Kind::PlainChunked => {
                let eof = msg.is_empty();
                match *buf {
                    WriteBuf::Flat(ref mut bytes) => bytes.put_slice(&msg),
                    WriteBuf::List(ref mut list) => list.buffer(EncodedBuf::Buf(msg)),
                }
                Ok(eof)
            }
            Kind::EncodeChunked(ref mut eof) => {
                if *eof {
                    return Ok(true);
                }

                match *buf {
                    WriteBuf::List(ref mut list) => {
                        if msg.is_empty() {
                            *eof = true;
                            list.buffer(EncodedBuf::Static(b"0\r\n\r\n"));
                        } else {
                            list.buffer(EncodedBuf::Buf(Bytes::from(format!("{:X}\r\n", msg.len()))));
                            list.buffer(EncodedBuf::Buf(msg));
                            list.buffer(EncodedBuf::Static(b"\r\n"));
                        }
                    }
                    WriteBuf::Flat(ref mut bytes) => {
                        if msg.is_empty() {
                            *eof = true;
                            bytes.put_slice(b"0\r\n\r\n");
                        } else {
                            use io::Write;

                            struct Writer<'a, B>(pub &'a mut B);

                            impl<'a, B> Write for Writer<'a, B>
                            where
                                B: BufMut,
                            {
                                fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                                    self.0.put_slice(buf);
                                    Ok(buf.len())
                                }

                                fn flush(&mut self) -> io::Result<()> {
                                    Ok(())
                                }
                            }

                            writeln!(Writer(bytes), "{:X}\r", msg.len()).unwrap();

                            bytes.reserve(msg.len() + 2);
                            bytes.put_slice(&msg);
                            bytes.put_slice(b"\r\n");
                        }
                    }
                }
                Ok(*eof)
            }
            Kind::Length(ref mut remaining) => {
                if *remaining > 0 {
                    if msg.is_empty() {
                        return Ok(*remaining == 0);
                    }
                    let len = cmp::min(*remaining, msg.len() as u64);

                    match buf {
                        WriteBuf::Flat(ref mut bytes) => {
                            bytes.put_slice(&msg.split_to(len as usize));
                        }
                        WriteBuf::List(ref mut list) => {
                            list.buffer(EncodedBuf::Buf(msg.split_to(len as usize)));
                        }
                    }

                    *remaining -= len as u64;
                    Ok(*remaining == 0)
                } else {
                    Ok(true)
                }
            }
            _ => unreachable!(),
        }
    }

    /// Encode eof. Return `EOF` state of encoder
    #[inline(always)]
    pub(super) fn encode_eof<const WRITE_BUF_LIMIT: usize>(
        &mut self,
        buf: &mut WriteBuf<WRITE_BUF_LIMIT>,
    ) -> io::Result<()> {
        match self.kind {
            Kind::Eof | Kind::PlainChunked => Ok(()),
            Kind::Length(rem) => {
                if rem != 0 {
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, ""))
                } else {
                    Ok(())
                }
            }
            Kind::EncodeChunked(ref mut eof) => {
                if !*eof {
                    *eof = true;
                    match *buf {
                        WriteBuf::Flat(ref mut bytes) => bytes.put_slice(b"0\r\n\r\n"),
                        WriteBuf::List(ref mut list) => list.buffer(EncodedBuf::Static(b"0\r\n\r\n")),
                    }
                }
                Ok(())
            }
            _ => unreachable!(),
        }
    }
}
