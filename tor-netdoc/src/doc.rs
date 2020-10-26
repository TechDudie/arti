//! Individual document types that we can parse in Tor's meta-format.
//!
//! Right now, we recognize four kinds of documents.
//!
//! A [netstatus::MDConsensus] is a multi-signed document that the
//! directory authorities use to tell clients and relays who is on the
//! network.  It contains information about each relay, and it links to
//! additional microdescriptors ([microdesc::Microdesc]) that have
//! more information about each relay.
//!
//! In order to validate a [netstatus::MDConsensus], you need to have
//! the authority certificate ([authcert::AuthCert]) for the directory
//! authorities that signed it.
//!
//! Finally, in order to use relays not listed in the consensus (such
//! as bridges), clients use those relays' self-signed router
//! descriptors ([routerdesc::RouterDesc]).  These router descriptors
//! are also uploaded to the authorities in order to tell them about
//! routers and their status.
//!
//! All of these formats are described in
//! [dir-spec.txt](https://spec.torproject.org/dir-spec).
//!
//! # Limitations
//!
//! Tor recognizes other kinds of documents that this crate doesn't
//! parse yet.  There are "ExtraInfo documents" that encode
//! information about relays that almost nobody needs.  There are
//! "ns-flavored" consensus documents that list all the router
//! descriptors on the network, instead of listing microdescriptors.
//! Finally, there are the voting documents themselves that authorities
//! use in order to calculate the consensus.

pub mod authcert;
pub mod microdesc;
pub mod netstatus;
pub mod routerdesc;
