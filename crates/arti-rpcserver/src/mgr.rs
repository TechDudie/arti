//! Top-level `RpcMgr` to launch sessions.

use std::sync::{Arc, Mutex, RwLock, Weak};

use rand::Rng;
use rpc::InvalidRpcIdentifier;
use tor_rpcbase as rpc;
use tracing::warn;
use weak_table::WeakValueHashMap;

use crate::{
    connection::{Connection, ConnectionId},
    globalid::{GlobalId, MacKey},
    RpcAuthentication,
};

/// A function we use to construct Session objects in response to authentication.
//
// TODO RPC: Perhaps this should return a Result?
type SessionFactory = Box<dyn Fn(&RpcAuthentication) -> Arc<dyn rpc::Object> + Send + Sync>;

/// Shared state, configuration, and data for all RPC sessions.
///
/// An RpcMgr knows how to listen for incoming RPC connections, and launch sessions based on them.
pub struct RpcMgr {
    /// A key that we use to ensure that identifiers are unforgeable.
    ///
    /// When giving out a global (non-session-bound) identifier, we use this key
    /// to authenticate the identifier when it's given back to us.
    ///
    /// We make copies of this key when constructing a session.
    global_id_mac_key: MacKey,

    /// Our reference to the dispatch table used to look up the functions that
    /// implement each object on each.
    ///
    /// Shared with each [`Connection`].
    ///
    /// **NOTE: observe the [Lock hierarchy](crate::mgr::Inner#lock-hierarchy)**
    dispatch_table: Arc<RwLock<rpc::DispatchTable>>,

    /// A function that we use to construct new Session objects when authentication
    /// is successful.
    session_factory: SessionFactory,

    /// Lock-protected view of the manager's state.
    ///
    /// **NOTE: observe the [Lock hierarchy](crate::mgr::Inner#lock-hierarchy)**
    ///
    /// This mutex is at an _inner_ level
    /// compared to the
    /// per-Connection locks.
    /// You must not take any per-connection lock if you
    /// hold this lock.
    /// Code that holds this lock must be checked
    /// to make sure that it doesn't then acquire any `Connection` lock.
    inner: Mutex<Inner>,
}

/// The [`RpcMgr`]'s state. This is kept inside a lock for interior mutability.
///
/// # Lock hierarchy
///
/// This system has, relevantly to the RPC code, three locks.
/// In order from outermost (acquire earlier) to innermost (acquire later):
///
///  1. [`Connection`]`.inner`
///  2. [`RpcMgr`]`.inner`
///  3. `RwLock<rpc::DispatchTable>`
///     (found in [`RpcMgr`]`.dispatch_table` *and* [`Connection`]`.dispatch_table`)
///
/// To avoid deadlock, when more than one of these locks is acquired,
/// they must be acquired in an order consistent with the order listed above.
///
/// (This ordering is slightly surprising:
/// normally a lock covering more-global state would be
/// "outside" (or "earlier")
/// compared to one covering more-narrowly-relevant state.)
// pub(crate) so we can link to the doc comment and its lock hierarchy
pub(crate) struct Inner {
    /// A map from [`ConnectionId`] to weak [`Connection`] references.
    ///
    /// We use this map to give connections a manager-global identifier that can
    /// be used to identify them from a SOCKS connection (or elsewhere outside
    /// of the RPC system).
    ///
    /// We _could_ use a generational arena here, but there isn't any point:
    /// since these identifiers are global, we need to keep them secure by
    /// MACing anything derived from them, which in turn makes the overhead of a
    /// HashMap negligible.
    connections: WeakValueHashMap<ConnectionId, Weak<Connection>>,
}

/// An error from creating or using an RpcMgr.
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RpcMgrError {
    /// At least one method had an invalid name.
    #[error("Method {1} had an invalid name")]
    InvalidMethodName(#[source] InvalidRpcIdentifier, String),
}

/// An [`rpc::Object`], along with its associated [`rpc::Context`].
///
/// The context can be used to invoke any special methods on the object.
type ObjectWithContext = (Arc<dyn rpc::Context>, Arc<dyn rpc::Object>);

impl RpcMgr {
    /// Create a new RpcMgr.
    pub fn new<F>(make_session: F) -> Result<Arc<Self>, RpcMgrError>
    where
        F: Fn(&RpcAuthentication) -> Arc<dyn rpc::Object> + Send + Sync + 'static,
    {
        let problems = rpc::check_method_names([]);
        // We warn about every problem.
        for (m, err) in &problems {
            warn!("Internal issue: Invalid RPC method name {m:?}: {err}");
        }
        let fatal_problem = problems
            .into_iter()
            // We don't treat UnrecognizedNamespace as fatal; somebody else might be extending our methods.
            .find(|(_, err)| !matches!(err, InvalidRpcIdentifier::UnrecognizedNamespace));
        if let Some((name, err)) = fatal_problem {
            return Err(RpcMgrError::InvalidMethodName(err, name.to_owned()));
        }

        Ok(Arc::new(RpcMgr {
            global_id_mac_key: MacKey::new(&mut rand::rng()),
            dispatch_table: Arc::new(RwLock::new(rpc::DispatchTable::from_inventory())),
            session_factory: Box::new(make_session),
            inner: Mutex::new(Inner {
                connections: WeakValueHashMap::new(),
            }),
        }))
    }

    /// Extend our method dispatch table with the method entries in `entries`.
    ///
    /// Ignores any entries that
    ///
    /// # Panics
    ///
    /// Panics if any entries are conflicting, according to the logic of
    /// [`DispatchTable::insert`](rpc::DispatchTable::insert)
    pub fn register_rpc_methods<I>(&self, entries: I)
    where
        I: IntoIterator<Item = rpc::dispatch::InvokerEnt>,
    {
        // TODO: Conceivably we might want to get a read lock on the RPC dispatch table,
        // check for the presence of these entries, and only take the write lock
        // if the entries are absent.  But for now, this function is called during
        // RpcMgr initialization, so there's no reason to optimize it.
        self.with_dispatch_table(|table| table.extend(entries));
    }

    /// Run `func` with a mutable reference to our dispatch table as an argument.
    ///
    /// Used to register additional methods.
    pub fn with_dispatch_table<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&mut rpc::DispatchTable) -> T,
    {
        let mut table = self.dispatch_table.write().expect("poisoned lock");
        func(&mut table)
    }

    /// Start a new session based on this RpcMgr, with a given TorClient.
    pub fn new_connection(
        self: &Arc<Self>,
        require_auth: tor_rpc_connect::auth::RpcAuth,
    ) -> Arc<Connection> {
        let connection_id = ConnectionId::from(rand::rng().random::<[u8; 16]>());
        let connection = Connection::new(
            connection_id,
            self.dispatch_table.clone(),
            self.global_id_mac_key.clone(),
            Arc::downgrade(self),
            require_auth,
        );

        let mut inner = self.inner.lock().expect("poisoned lock");
        let old = inner.connections.insert(connection_id, connection.clone());
        assert!(
            old.is_none(),
            // Specifically, we shouldn't expect collisions until we have made on the
            // order of 2^64 connections, and that shouldn't be possible on
            // realistic systems.
            "connection ID collision detected; this is phenomenally unlikely!",
        );
        connection
    }

    /// Look up an object in  the context of this `RpcMgr`.
    ///
    /// Some object identifiers exist in a manager-global context, so that they
    /// can be used outside of a single RPC session.  This function looks up an
    /// object by such an identifier string.  It returns an error if the
    /// identifier is invalid or the object does not exist.
    ///
    /// Along with the object, this additionally returns the [`rpc::Context`] associated with the
    /// object.  That context can be used to invoke any special methods on the object.
    pub fn lookup_object(&self, id: &rpc::ObjectId) -> Result<ObjectWithContext, rpc::LookupError> {
        let global_id = GlobalId::try_decode(&self.global_id_mac_key, id)?;
        self.lookup_by_global_id(&global_id)
            .ok_or_else(|| rpc::LookupError::NoObject(id.clone()))
    }

    /// As `lookup_object`, but takes a parsed and validated [`GlobalId`].
    pub(crate) fn lookup_by_global_id(&self, id: &GlobalId) -> Option<ObjectWithContext> {
        let connection = {
            let inner = self.inner.lock().expect("lock poisoned");
            let connection = inner.connections.get(&id.connection)?;
            // Here we release the lock on self.inner, which makes it okay to
            // invoke a method on `connection` that may take its lock.
            drop(inner);
            connection
        };
        let obj = connection.lookup_by_idx(id.local_id)?;
        Some((connection, obj))
    }

    /// Construct a new object to serve as the `session` for a connection.
    pub(crate) fn create_session(&self, auth: &RpcAuthentication) -> Arc<dyn rpc::Object> {
        (self.session_factory)(auth)
    }
}
