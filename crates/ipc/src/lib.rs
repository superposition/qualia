//! Control-plane transport shared by the stack supervisor and the runners it
//! starts.
//!
//! The supervisor owns a named endpoint ([`ControlListener`]); every runner
//! connects to it ([`ControlStream`]). Both sides exchange one small fixed
//! frame: a discriminant byte naming the [`ControlMsg`], a length byte saying
//! whether a one-byte payload follows, and then that payload. Nothing else
//! crosses this channel, which is what keeps an operator's `qualia shutdown`
//! usable from a terminal while the engine is mid-tick.
//!
//! Unix binds the endpoint to the socket file named by the stack manifest.
//! Windows has no equivalent namespace for this use, so the endpoint path is
//! folded onto a deterministic loopback port; both ends of an existing
//! configuration land on the same address without gaining a new setting.

pub use qualia_types::*;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::net::{TcpListener as EndpointListener, TcpStream as EndpointStream};
#[cfg(not(windows))]
use std::os::unix::net::{UnixListener as EndpointListener, UnixStream as EndpointStream};

/// Bytes in a control frame header: discriminant plus payload length.
const HEADER_LEN: usize = 2;

/// Bytes of optional payload a frame may carry.
const MAX_PAYLOAD_LEN: u8 = 1;

/// An instruction an operator or supervisor sends to a runner.
///
/// The numeric values are wire discriminants: they are part of the frame
/// contract and must not be renumbered.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlMsg {
    /// Stop the whole engine gracefully.
    Shutdown = 0,
    /// Halt all layers at once; this is the emergency path.
    Estop = 1,
    /// Suspend one layer, selected by the payload byte.
    Pause = 2,
    /// Resume one layer, selected by the payload byte.
    Resume = 3,
    /// Ask for status; a payload of `0xFF` means every layer.
    Status = 4,
}

impl ControlMsg {
    /// Decode a wire discriminant, rejecting values this build does not know.
    fn decode(byte: u8) -> io::Result<Self> {
        match byte {
            0 => Ok(ControlMsg::Shutdown),
            1 => Ok(ControlMsg::Estop),
            2 => Ok(ControlMsg::Pause),
            3 => Ok(ControlMsg::Resume),
            4 => Ok(ControlMsg::Status),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown control message type: {}", other),
            )),
        }
    }
}

/// The supervisor's side of the control channel.
pub struct ControlListener {
    listener: EndpointListener,
    path: PathBuf,
}

impl ControlListener {
    /// Bind the control endpoint at `path`.
    ///
    /// On Unix a socket file left behind by a supervisor that crashed is
    /// unlinked first, so a restart never has to be preceded by a manual
    /// cleanup.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        #[cfg(not(windows))]
        let listener = {
            let _ = std::fs::remove_file(&path);
            EndpointListener::bind(&path)?
        };
        #[cfg(windows)]
        let listener = EndpointListener::bind(loopback_addr(&path))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, path })
    }

    /// Take a pending connection, or `None` when none has arrived yet.
    ///
    /// The accepted stream is switched to blocking mode: only the listener
    /// polls, so [`ControlStream::recv`] can wait out a frame that arrives in
    /// pieces. Windows would otherwise hand back a stream that inherits the
    /// listener's non-blocking flag and fail a split frame with `WouldBlock`.
    pub fn try_accept(&self) -> Option<ControlStream> {
        self.listener
            .accept()
            .ok()
            .and_then(|(stream, _)| ControlStream::accepted(stream).ok())
    }

    /// Wait for the next connection.
    ///
    /// The listener is left polling non-blockingly afterwards, which is the
    /// mode [`try_accept`](Self::try_accept) expects.
    pub fn accept_blocking(&self) -> io::Result<ControlStream> {
        self.listener.set_nonblocking(false)?;
        let accepted = self
            .listener
            .accept()
            .and_then(|(stream, _)| ControlStream::accepted(stream));
        self.listener.set_nonblocking(true)?;
        accepted
    }

    /// The path this listener is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ControlListener {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One runner's side of the control channel.
pub struct ControlStream {
    stream: EndpointStream,
}

impl ControlStream {
    /// Wrap a freshly accepted socket, leaving it in blocking mode.
    fn accepted(stream: EndpointStream) -> io::Result<Self> {
        stream.set_nonblocking(false)?;
        Ok(Self { stream })
    }

    /// Connect to a supervisor's control endpoint.
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        #[cfg(not(windows))]
        let stream = EndpointStream::connect(path)?;
        #[cfg(windows)]
        let stream = EndpointStream::connect(loopback_addr(path.as_ref()))?;
        Ok(Self { stream })
    }

    /// Send one control frame with an optional single-byte payload.
    pub fn send(&mut self, msg: ControlMsg, payload: Option<u8>) -> io::Result<()> {
        let payload_len = u8::from(payload.is_some());
        self.stream.write_all(&[msg as u8, payload_len])?;
        if let Some(byte) = payload {
            self.stream.write_all(&[byte])?;
        }
        self.stream.flush()
    }

    /// Receive one control frame, blocking until it is complete.
    ///
    /// A peer that disappears part-way through a frame surfaces as
    /// [`io::ErrorKind::UnexpectedEof`] from the short read rather than as a
    /// panic; an unknown discriminant or a payload length above one byte
    /// surfaces as [`io::ErrorKind::InvalidData`].
    pub fn recv(&mut self) -> io::Result<(ControlMsg, Option<u8>)> {
        let mut header = [0u8; HEADER_LEN];
        self.stream.read_exact(&mut header)?;
        let msg = ControlMsg::decode(header[0])?;
        match header[1] {
            0 => Ok((msg, None)),
            MAX_PAYLOAD_LEN => {
                let mut payload = [0u8; MAX_PAYLOAD_LEN as usize];
                self.stream.read_exact(&mut payload)?;
                Ok((msg, Some(payload[0])))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid control frame length: {}", other),
            )),
        }
    }

    /// Switch blocking mode for this stream.
    ///
    /// The supervisor accepts in non-blocking mode, so a runner that has to
    /// wait for history rather than for a control message sets this itself.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.stream.set_nonblocking(nonblocking)
    }
}

/// Map a control endpoint path onto a loopback address.
///
/// Windows cannot bind a socket to a filesystem path, so the path is hashed
/// (FNV-1a, 32-bit) into a port in `20000..40000`. The mapping is pure and
/// stable across processes, which is what lets a separately started runner
/// compute the same address as its supervisor.
#[cfg(windows)]
fn loopback_addr(path: &Path) -> String {
    const FNV_OFFSET_BASIS: u32 = 2166136261;
    const FNV_PRIME: u32 = 16777619;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let port = 20000 + (hash % 20000);
    format!("127.0.0.1:{}", port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    /// How long a test waits for a connection before declaring failure.
    const ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);

    fn endpoint(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "qualia-ipc-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn accept_within(listener: &ControlListener) -> ControlStream {
        let deadline = Instant::now() + ACCEPT_TIMEOUT;
        loop {
            if let Some(stream) = listener.try_accept() {
                return stream;
            }
            assert!(
                Instant::now() < deadline,
                "no connection arrived before the deadline"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// A client that connects, writes exact bytes, and closes. Used to place
    /// malformed or deliberately split frames on the wire.
    fn write_raw(path: PathBuf, bytes: Vec<u8>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut stream = ControlStream::connect(&path).expect("raw client connect");
            stream.stream.write_all(&bytes).expect("raw client write");
            stream.stream.flush().expect("raw client flush");
        })
    }

    #[test]
    fn control_message_round_trips_over_the_endpoint() {
        let path = endpoint("roundtrip");
        let listener = ControlListener::bind(&path).expect("bind");
        assert_eq!(listener.path(), path.as_path());

        let client = {
            let path = path.clone();
            thread::spawn(move || {
                let mut stream = ControlStream::connect(&path).expect("connect");
                stream.send(ControlMsg::Pause, Some(3)).expect("send");
            })
        };

        let mut server = accept_within(&listener);
        assert_eq!(server.recv().expect("recv"), (ControlMsg::Pause, Some(3)));
        client.join().expect("client thread");
    }

    #[test]
    fn payloadless_frame_round_trips() {
        let path = endpoint("payloadless");
        let listener = ControlListener::bind(&path).expect("bind");

        let client = {
            let path = path.clone();
            thread::spawn(move || {
                let mut stream = ControlStream::connect(&path).expect("connect");
                stream.send(ControlMsg::Shutdown, None).expect("send");
            })
        };

        let mut server = accept_within(&listener);
        assert_eq!(
            server.recv().expect("recv"),
            (ControlMsg::Shutdown, None),
            "a zero-length frame must decode with no payload"
        );
        client.join().expect("client thread");
    }

    #[test]
    fn frame_split_across_reads_reassembles() {
        let path = endpoint("split");
        let listener = ControlListener::bind(&path).expect("bind");

        let client = {
            let path = path.clone();
            thread::spawn(move || {
                let mut stream = ControlStream::connect(&path).expect("connect");
                // Half a header, then a pause, then the rest: the receiver
                // must block across the gap rather than treat it as a frame.
                stream
                    .stream
                    .write_all(&[ControlMsg::Resume as u8])
                    .expect("first byte");
                stream.stream.flush().expect("flush");
                thread::sleep(Duration::from_millis(100));
                stream
                    .stream
                    .write_all(&[1, 9])
                    .expect("remaining bytes");
                stream.stream.flush().expect("flush");
            })
        };

        let mut server = accept_within(&listener);
        assert_eq!(server.recv().expect("recv"), (ControlMsg::Resume, Some(9)));
        client.join().expect("client thread");
    }

    #[test]
    fn peer_vanishing_mid_frame_is_an_error_not_a_panic() {
        let path = endpoint("eof");
        let listener = ControlListener::bind(&path).expect("bind");

        let client = {
            let path = path.clone();
            thread::spawn(move || {
                let mut stream = ControlStream::connect(&path).expect("connect");
                stream
                    .stream
                    .write_all(&[ControlMsg::Status as u8])
                    .expect("half a header");
                stream.stream.flush().expect("flush");
                // Dropping the stream here closes the connection with the
                // frame half written.
            })
        };

        let mut server = accept_within(&listener);
        client.join().expect("client thread");

        let err = server
            .recv()
            .expect_err("a truncated frame must not decode as a message");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn unknown_discriminant_is_invalid_data() {
        let path = endpoint("unknown-msg");
        let listener = ControlListener::bind(&path).expect("bind");
        let client = write_raw(path.clone(), vec![42, 0]);

        let mut server = accept_within(&listener);
        let err = server.recv().expect_err("unknown discriminant must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        client.join().expect("client thread");
    }

    #[test]
    fn oversized_payload_length_is_invalid_data() {
        let path = endpoint("bad-length");
        let listener = ControlListener::bind(&path).expect("bind");
        let client = write_raw(path.clone(), vec![ControlMsg::Shutdown as u8, 2]);

        let mut server = accept_within(&listener);
        let err = server.recv().expect_err("length 2 must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        client.join().expect("client thread");
    }

    #[test]
    fn nonblocking_recv_reports_would_block_on_an_empty_stream() {
        let path = endpoint("nonblocking");
        let listener = ControlListener::bind(&path).expect("bind");

        let (release, held) = mpsc::channel::<()>();
        let client = {
            let path = path.clone();
            thread::spawn(move || {
                let stream = ControlStream::connect(&path).expect("connect");
                let _ = held.recv_timeout(ACCEPT_TIMEOUT);
                drop(stream);
            })
        };

        let mut server = accept_within(&listener);
        server.set_nonblocking(true).expect("set nonblocking");
        let err = server
            .recv()
            .expect_err("an empty non-blocking read must not produce a frame");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);

        release.send(()).ok();
        client.join().expect("client thread");
    }

    #[cfg(not(windows))]
    #[test]
    fn socket_file_lives_while_bound_and_is_unlinked_on_drop() {
        let path = endpoint("unlink");
        {
            let listener = ControlListener::bind(&path).expect("bind");
            assert!(path.exists(), "the socket file should exist while bound");
            assert_eq!(listener.path(), path.as_path());
        }
        assert!(!path.exists(), "the socket file should be removed on drop");
    }

    #[cfg(not(windows))]
    #[test]
    fn binding_replaces_a_stale_socket_file() {
        let path = endpoint("stale");
        let first = ControlListener::bind(&path).expect("first bind");
        std::mem::forget(first); // a supervisor that never got to clean up
        assert!(path.exists());

        let listener = ControlListener::bind(&path).expect("rebind over the stale file");
        assert_eq!(listener.path(), path.as_path());
    }

    #[cfg(windows)]
    #[test]
    fn loopback_mapping_is_stable_and_distinct_per_path() {
        let a = endpoint("loopback-a");
        let b = endpoint("loopback-b");
        assert_eq!(loopback_addr(&a), loopback_addr(&a));
        assert_ne!(loopback_addr(&a), loopback_addr(&b));
        assert!(
            !a.exists(),
            "windows routes through loopback, not through a socket file"
        );
    }

    #[cfg(windows)]
    #[test]
    fn distinct_paths_yield_independent_channels() {
        let path_a = endpoint("channel-a");
        let path_b = endpoint("channel-b");
        let listener_a = ControlListener::bind(&path_a).expect("bind a");
        let listener_b = ControlListener::bind(&path_b).expect("bind b");

        let client_a = {
            let path = path_a.clone();
            thread::spawn(move || {
                let mut stream = ControlStream::connect(&path).expect("connect a");
                stream.send(ControlMsg::Resume, Some(7)).expect("send a");
            })
        };
        let client_b = {
            let path = path_b.clone();
            thread::spawn(move || {
                let mut stream = ControlStream::connect(&path).expect("connect b");
                stream.send(ControlMsg::Pause, None).expect("send b");
            })
        };

        let mut server_a = listener_a.accept_blocking().expect("accept a");
        let mut server_b = listener_b.accept_blocking().expect("accept b");
        assert_eq!(server_a.recv().expect("recv a"), (ControlMsg::Resume, Some(7)));
        assert_eq!(server_b.recv().expect("recv b"), (ControlMsg::Pause, None));

        client_a.join().expect("client a");
        client_b.join().expect("client b");
    }
}
