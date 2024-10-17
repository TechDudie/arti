//! Support for generalized addresses.
//!
//! We use the [`SocketAddr`] type in this module,
//! and its associated [`Stream`] and [`Listener`] types,
//! when we want write code
//! that can treat AF_UNIX addresses and internet addresses as a single type.
//!
//! As an alternative, you could also write your code to be generic
//! over address, listener, provider, and stream types.
//! That would give you the performance benefits of monomorphization
//! over some corresponding costs in complexity and code size.
//! Generally, it's better to use these types unless you know
//! that the minor performance overhead here will matter in practice.

use async_trait::async_trait;
use futures::{stream, AsyncRead, AsyncWrite, StreamExt as _};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::{unix, NetStreamListener, NetStreamProvider};
use std::{
    io::{Error as IoError, Result as IoResult},
    net,
};

#[cfg(target_os = "android")]
use std::os::android::net::SocketAddrExt as _;
#[cfg(target_os = "linux")]
use std::os::linux::net::SocketAddrExt as _;

/// Any address that Arti can listen on or connect to.
///
/// We use this type when we want to make streams
/// without being concerned whether they are AF_UNIX streams, TCP streams, or so forth.
///
/// To avoid confusion, you might want to avoid importing this type directly.
/// Instead, import [`rtcompat::general`](crate::general)
/// and refer to this type as `general::SocketAddr`.
///
/// ## String representation
///
/// Any `general::SocketAddr` has up to two string representations:
///
/// 1. A _qualified_ representation, consisting of a schema
///    (either "unix" or "tcp"),
///    followed by a single colon,
///    followed by the address itself represented as a string.
///
///    Examples: `unix:/path/to/socket`, `tcp:127.0.0.1:9999`,
///    `tcp:[::1]:9999`.
///
///    The "unnamed" unix address is represented as `unix:`.
///
/// 2. A _unqualified_ representation,
///    consisting of a TCP address represented as a string.
///
///    Examples: `127.0.0.1:9999`,  `[::1]:9999`.
///
/// Note that not every `general::SocketAddr` has a string representation!
/// Currently, the ones that might not be representable are:
///
///  - "Abstract" AF_UNIX addresses (a Linux feature)
///  - AF_UNIX addresses whose path name is not UTF-8.
///
/// Note also that string representations may contain whitespace
/// or other unusual characters.
/// `/var/run/arti socket` is a valid filename,
/// so `unix:/var/run/arti socket` is a valid representation.
///
/// We may add new schemas in the future.
/// If we do, any new schema will begin with an ascii alphabetical character,
/// and will consist only of ascii alphanumeric characters,
/// the character `-`, and the character `_`.
///
/// ### TCP address representation
///
/// When representing a TCP address as a string,
/// we use the formats implemented by [`std::net::SocketAddr`]'s
/// `FromStr` implementation.  In contrast with the textual representations of
/// [`Ipv4Addr`](std::net::Ipv4Addr) and [`Ipv6Addr`](std::net::Ipv6Addr),
/// these formats are not currently very well specified by Rust.
/// Therefore we describe them here:
///   * A IPv4 TCP address is encoded as:
///     - an [IPv4 address],
///     - a colon (`:`),
///     - a 16-bit decimal integer.
///   * An IPv6 TCP address is encoded as:
///     - a left square bracket (`[`),
///     - an [IPv6 address],
///     - optionally, a percent sign (`%`) and a 32-bit decimal integer
///     - a right square bracket (`]`),
///     - a colon (`:`),
///     - a 16-bit decimal integer.
///
/// Note that the above TCP implementation does not provide any way
/// to encode the [`flowinfo`](std::net::SocketAddrV6::flowinfo) member
/// of a `SocketAddrV6`.
/// Any `flowinfo` information set in an address
/// will therefore be lost when the address is encoded.
///
/// [IPv4 address]: https://doc.rust-lang.org/std/net/struct.Ipv4Addr.html#textual-representation
/// [IPv6 address]: https://doc.rust-lang.org/std/net/struct.Ipv6Addr.html#textual-representation
///
/// TODO: We should try to get Rust's stdlib specify these formats, so we don't have to.
/// There is an open PR at <https://github.com/rust-lang/rust/pull/131790>.
#[derive(Clone, Debug, derive_more::From, derive_more::TryInto)]
#[non_exhaustive]
pub enum SocketAddr {
    /// An IPv4 or IPv6 address on the internet.
    Inet(net::SocketAddr),
    /// A local AF_UNIX address.
    ///
    /// (Note that [`unix::SocketAddr`] is unconstructable on platforms where it is not supported.)
    Unix(unix::SocketAddr),
}

impl SocketAddr {
    /// Return a wrapper object that can be used to display this address.
    ///
    /// The resulting display might be lossy, depending on whether this address can be represented
    /// as a string.
    ///
    /// The displayed format here is intentionally undocumented;
    /// it may change in the future.
    pub fn display_lossy(&self) -> DisplayLossy<'_> {
        DisplayLossy(self)
    }

    /// If possible, return a qualified string representation for this address.
    ///
    /// Otherwise return None.
    pub fn try_to_string(&self) -> Option<String> {
        use SocketAddr::*;
        match self {
            Inet(sa) => Some(format!("tcp:{}", sa)),
            Unix(sa) => {
                if sa.is_unnamed() {
                    Some("unix:".to_string())
                } else {
                    sa.as_pathname()
                        .and_then(Path::to_str)
                        .map(|p| format!("unix:{}", p))
                }
            }
        }
    }
}

/// Lossy display for a [`SocketAddr`].
pub struct DisplayLossy<'a>(&'a SocketAddr);

impl<'a> std::fmt::Display for DisplayLossy<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use SocketAddr::*;
        match self.0 {
            Inet(sa) => write!(f, "tcp:{}", sa),
            Unix(sa) => {
                if let Some(path) = sa.as_pathname() {
                    if let Some(path_str) = path.to_str() {
                        write!(f, "unix:{}", path_str)
                    } else {
                        write!(f, "unix:{} [lossy]", path.to_string_lossy())
                    }
                } else if sa.is_unnamed() {
                    write!(f, "unix:")
                } else {
                    write!(f, "unix:{:?} [lossy]", sa)
                }
            }
        }
    }
}

impl std::str::FromStr for SocketAddr {
    type Err = AddrParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with(|c: char| (c.is_ascii_digit() || c == '[')) {
            // This looks like a tcp address, and cannot be a qualified address.
            Ok(s.parse::<net::SocketAddr>()?.into())
        } else if let Some((schema, remainder)) = s.split_once(':') {
            match schema {
                "unix" => Ok(unix::SocketAddr::from_pathname(remainder)?.into()),
                "tcp" => Ok(remainder.parse::<net::SocketAddr>()?.into()),
                _ => Err(AddrParseError::UnrecognizedSchema(schema.to_string())),
            }
        } else {
            Err(AddrParseError::NoSchema)
        }
    }
}

/// An error encountered while attempting to parse a [`SocketAddr`]
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AddrParseError {
    /// Tried to parse an address with an unrecognized schema.
    #[error("Address schema {0:?} unrecognized")]
    UnrecognizedSchema(String),
    /// Tried to parse a non TCP-address with no schema.
    #[error("Address did not look like TCP, but had no address schema.")]
    NoSchema,
    /// Tried to parse an address as an AF_UNIX address, but failed.
    #[error("Invalid AF_UNIX address")]
    InvalidUnixAddress(#[source] Arc<IoError>),
    /// Tried to parse an address as a TCP address, but failed.
    #[error("Invalid TCP address")]
    InvalidTcpAddress(#[from] std::net::AddrParseError),
}

impl From<IoError> for AddrParseError {
    fn from(e: IoError) -> Self {
        Self::InvalidUnixAddress(Arc::new(e))
    }
}

impl PartialEq for SocketAddr {
    /// Return true if two `SocketAddr`s are equal.
    ///
    /// For `Inet` addresses, delegates to `std::net::SocketAddr::eq`.
    ///
    /// For `Unix` addresses, treats two addresses as equal if any of the following is true:
    ///   - Both addresses have the same path.
    ///   - Both addresses are unnamed.
    ///   - (Linux only) Both addresses have the same abstract name.
    ///
    /// Addresses of different types are always unequal.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Inet(l0), Self::Inet(r0)) => l0 == r0,
            #[cfg(unix)]
            (Self::Unix(l0), Self::Unix(r0)) => {
                // Sadly, std::os::unix::net::SocketAddr doesn't implement PartialEq.
                //
                // This requires us to make our own, and prevents us from providing Eq.
                if l0.is_unnamed() && r0.is_unnamed() {
                    return true;
                }
                if let (Some(a), Some(b)) = (l0.as_pathname(), r0.as_pathname()) {
                    return a == b;
                }
                #[cfg(any(target_os = "android", target_os = "linux"))]
                if let (Some(a), Some(b)) = (l0.as_abstract_name(), r0.as_abstract_name()) {
                    return a == b;
                }
                false
            }
            _ => false,
        }
    }
}

#[cfg(feature = "arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for SocketAddr {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        /// Simple enumeration to select an address type.
        #[allow(clippy::missing_docs_in_private_items)]
        #[derive(arbitrary::Arbitrary)]
        enum Kind {
            V4,
            V6,
            #[cfg(unix)]
            Unix,
            #[cfg(any(target_os = "android", target_os = "linux"))]
            UnixAbstract,
        }
        match u.arbitrary()? {
            Kind::V4 => Ok(SocketAddr::Inet(
                net::SocketAddrV4::new(u.arbitrary()?, u.arbitrary()?).into(),
            )),
            Kind::V6 => Ok(SocketAddr::Inet(
                net::SocketAddrV6::new(
                    u.arbitrary()?,
                    u.arbitrary()?,
                    u.arbitrary()?,
                    u.arbitrary()?,
                )
                .into(),
            )),
            #[cfg(unix)]
            Kind::Unix => {
                let pathname: std::ffi::OsString = u.arbitrary()?;
                Ok(SocketAddr::Unix(
                    unix::SocketAddr::from_pathname(pathname)
                        .map_err(|_| arbitrary::Error::IncorrectFormat)?,
                ))
            }
            #[cfg(any(target_os = "android", target_os = "linux"))]
            Kind::UnixAbstract => {
                use std::os::linux::net::SocketAddrExt as _;
                let name: &[u8] = u.arbitrary()?;
                Ok(SocketAddr::Unix(
                    unix::SocketAddr::from_abstract_name(name)
                        .map_err(|_| arbitrary::Error::IncorrectFormat)?,
                ))
            }
        }
    }
}

/// Helper trait to allow us to create a type-erased stream.
///
/// (Rust doesn't allow "dyn AsyncRead + AsyncWrite")
trait ReadAndWrite: AsyncRead + AsyncWrite + Send + Sync {}
impl<T> ReadAndWrite for T where T: AsyncRead + AsyncWrite + Send + Sync {}

/// A stream returned by a `NetStreamProvider<GeneralizedAddr>`
pub struct Stream(Pin<Box<dyn ReadAndWrite>>);
impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<IoResult<usize>> {
        self.0.as_mut().poll_read(cx, buf)
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        self.0.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        self.0.as_mut().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        self.0.as_mut().poll_close(cx)
    }
}

/// The type of the result from an [`IncomingStreams`].
type StreamItem = IoResult<(Stream, SocketAddr)>;

/// A stream of incoming connections on a [`general::Listener`](Listener).
pub struct IncomingStreams(Pin<Box<dyn stream::Stream<Item = StreamItem> + Send + Sync>>);

impl stream::Stream for IncomingStreams {
    type Item = IoResult<(Stream, SocketAddr)>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.as_mut().poll_next(cx)
    }
}

/// A listener returned by a `NetStreamProvider<general::SocketAddr>`.
pub struct Listener {
    /// The `futures::Stream` of incoming network streams.
    streams: IncomingStreams,
    /// The local address on which we're listening.
    local_addr: SocketAddr,
}

impl NetStreamListener<SocketAddr> for Listener {
    type Stream = Stream;
    type Incoming = IncomingStreams;

    fn incoming(self) -> IncomingStreams {
        self.streams
    }

    fn local_addr(&self) -> IoResult<SocketAddr> {
        Ok(self.local_addr.clone())
    }
}

/// Use `provider` to launch a `NetStreamListener` at `address`, and wrap that listener
/// as a `Listener`.
async fn abstract_listener_on<ADDR, P>(provider: &P, address: &ADDR) -> IoResult<Listener>
where
    P: NetStreamProvider<ADDR>,
    SocketAddr: From<ADDR>,
{
    let lis = provider.listen(address).await?;
    let local_addr = SocketAddr::from(lis.local_addr()?);
    let streams = lis.incoming().map(|result| {
        result.map(|(socket, addr)| (Stream(Box::pin(socket)), SocketAddr::from(addr)))
    });
    let streams = IncomingStreams(Box::pin(streams));
    Ok(Listener {
        streams,
        local_addr,
    })
}

#[async_trait]
impl<T> NetStreamProvider<SocketAddr> for T
where
    T: NetStreamProvider<net::SocketAddr> + NetStreamProvider<unix::SocketAddr>,
{
    type Stream = Stream;
    type Listener = Listener;

    async fn connect(&self, addr: &SocketAddr) -> IoResult<Stream> {
        use SocketAddr as G;
        match addr {
            G::Inet(a) => Ok(Stream(Box::pin(self.connect(a).await?))),
            G::Unix(a) => Ok(Stream(Box::pin(self.connect(a).await?))),
        }
    }
    async fn listen(&self, addr: &SocketAddr) -> IoResult<Listener> {
        use SocketAddr as G;
        match addr {
            G::Inet(a) => abstract_listener_on(self, a).await,
            G::Unix(a) => abstract_listener_on(self, a).await,
        }
    }
}

#[cfg(test)]
mod test {
    // @@ begin test lint list maintained by maint/add_warning @@
    #![allow(clippy::bool_assert_comparison)]
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::dbg_macro)]
    #![allow(clippy::mixed_attributes_style)]
    #![allow(clippy::print_stderr)]
    #![allow(clippy::print_stdout)]
    #![allow(clippy::single_char_pattern)]
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::unchecked_duration_subtraction)]
    #![allow(clippy::useless_vec)]
    #![allow(clippy::needless_pass_by_value)]
    //! <!-- @@ end test lint list maintained by maint/add_warning @@ -->

    use super::AddrParseError;
    use crate::general;
    use assert_matches::assert_matches;
    #[cfg(unix)]
    use std::os::unix::net as unix;
    use std::{net, str::FromStr as _};

    /// Parse `s` as a `net::SocketAddr`, and build a `general::SocketAddr` from it.
    ///
    /// Testing only. Panics on error.
    fn from_inet(s: &str) -> general::SocketAddr {
        let a: net::SocketAddr = s.parse().unwrap();
        a.into()
    }

    #[test]
    fn ok_tcp() {
        assert_eq!(
            from_inet("127.0.0.1:9999"),
            general::SocketAddr::from_str("127.0.0.1:9999").unwrap()
        );
        assert_eq!(
            from_inet("127.0.0.1:9999"),
            general::SocketAddr::from_str("tcp:127.0.0.1:9999").unwrap()
        );

        assert_eq!(
            from_inet("[::1]:9999"),
            general::SocketAddr::from_str("[::1]:9999").unwrap()
        );
        assert_eq!(
            from_inet("[::1]:9999"),
            general::SocketAddr::from_str("tcp:[::1]:9999").unwrap()
        );

        assert_ne!(
            general::SocketAddr::from_str("127.0.0.1:9999").unwrap(),
            general::SocketAddr::from_str("[::1]:9999").unwrap()
        );

        let ga1 = from_inet("127.0.0.1:9999");
        assert_eq!(ga1.display_lossy().to_string(), "tcp:127.0.0.1:9999");
        assert_eq!(ga1.try_to_string().unwrap(), "tcp:127.0.0.1:9999");

        let ga2 = from_inet("[::1]:9999");
        assert_eq!(ga2.display_lossy().to_string(), "tcp:[::1]:9999");
        assert_eq!(ga2.try_to_string().unwrap(), "tcp:[::1]:9999");
    }

    /// Treat `s` as a unix path, and build a `general::SocketAddr` from it.
    ///
    /// Testing only. Panics on error.
    #[cfg(unix)]
    fn from_pathname(s: impl AsRef<std::path::Path>) -> general::SocketAddr {
        let a = unix::SocketAddr::from_pathname(s).unwrap();
        a.into()
    }
    #[test]
    #[cfg(unix)]
    fn ok_unix() {
        assert_eq!(
            from_pathname("/some/path"),
            general::SocketAddr::from_str("unix:/some/path").unwrap()
        );
        assert_eq!(
            from_pathname("/another/path"),
            general::SocketAddr::from_str("unix:/another/path").unwrap()
        );
        assert_eq!(
            from_pathname("/path/with spaces"),
            general::SocketAddr::from_str("unix:/path/with spaces").unwrap()
        );
        assert_ne!(
            general::SocketAddr::from_str("unix:/some/path").unwrap(),
            general::SocketAddr::from_str("unix:/another/path").unwrap()
        );
        assert_eq!(
            from_pathname(""),
            general::SocketAddr::from_str("unix:").unwrap()
        );

        let ga1 = general::SocketAddr::from_str("unix:/some/path").unwrap();
        assert_eq!(ga1.display_lossy().to_string(), "unix:/some/path");
        assert_eq!(ga1.try_to_string().unwrap(), "unix:/some/path");

        let ga2 = general::SocketAddr::from_str("unix:/another/path").unwrap();
        assert_eq!(ga2.display_lossy().to_string(), "unix:/another/path");
        assert_eq!(ga2.try_to_string().unwrap(), "unix:/another/path");
    }

    #[test]
    fn parse_err_tcp() {
        assert_matches!(
            "1234567890:999".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidTcpAddress(_))
        );
        assert_matches!(
            "1z".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidTcpAddress(_))
        );
        assert_matches!(
            "[[77".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidTcpAddress(_))
        );

        assert_matches!(
            "tcp:fred:9999".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidTcpAddress(_))
        );

        assert_matches!(
            "tcp:127.0.0.1".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidTcpAddress(_))
        );

        assert_matches!(
            "tcp:[::1]".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidTcpAddress(_))
        );
    }

    #[test]
    fn parse_err_schemata() {
        assert_matches!(
            "fred".parse::<general::SocketAddr>(),
            Err(AddrParseError::NoSchema)
        );
        assert_matches!(
            "fred:".parse::<general::SocketAddr>(),
            Err(AddrParseError::UnrecognizedSchema(f)) if f == "fred"
        );
        assert_matches!(
            "fred:hello".parse::<general::SocketAddr>(),
            Err(AddrParseError::UnrecognizedSchema(f)) if f == "fred"
        );
    }

    #[test]
    #[cfg(unix)]
    fn display_unix_weird() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let a1 = from_pathname(OsStr::from_bytes(&[255, 255, 255, 255]));
        assert!(a1.try_to_string().is_none());
        assert_eq!(a1.display_lossy().to_string(), "unix:���� [lossy]");

        let a2 = from_pathname("");
        assert_eq!(a2.try_to_string().unwrap(), "unix:");
        assert_eq!(a2.display_lossy().to_string(), "unix:");
    }

    #[test]
    #[cfg(not(unix))]
    fn parse_err_no_unix() {
        assert_matches!(
            "unix:".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidUnixAddress(_))
        );
        assert_matches!(
            "unix:/any/path".parse::<general::SocketAddr>(),
            Err(AddrParseError::InvalidUnixAddress(_))
        );
    }
}
