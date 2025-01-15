//! RPC commands and related functionality for authentication.
//!
//! In Arti's RPC system, authentication is a kind of method that can be invoked
//! on the special "connection" object, which gives you an RPC _session_ as a
//! result.  The RPC session is the root for all other capabilities.

use std::sync::Arc;

use super::Connection;
use derive_deftly::Deftly;
use tor_rpcbase as rpc;
use tor_rpcbase::templates::*;

mod cookie;
mod inherent;

/// Information about how an RPC session has been authenticated.
///
/// Currently, this isn't actually used for anything, since there's only one way
/// to authenticate a connection.  It exists so that later we can pass
/// information to the session-creator function.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RpcAuthentication {}

/// The authentication scheme as enumerated in the spec.
///
/// Conceptually, an authentication scheme answers the question "How can the
/// Arti process know you have permissions to use or administer it?"
#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
enum AuthenticationScheme {
    /// Inherent authority based on the ability to open the connection to this address.
    #[serde(rename = "auth:inherent")]
    Inherent,

    /// Negotiation based on mutual proof of ability to read a file from disk.
    #[serde(rename = "auth:cookie")]
    Cookie,
}

/// Ask which authentication methods are supported.
///
/// This method can be invoked on a `Connection` pre-authentication;
/// it's used to tell which methods are supported,
/// and what parameters they require.
#[derive(Debug, serde::Deserialize, Deftly)]
#[derive_deftly(DynMethod)]
#[deftly(rpc(method_name = "auth:query"))]
struct AuthQuery {}

/// A list of supported authentication schemes and their parameters.
#[derive(Debug, serde::Serialize)]
struct SupportedAuth {
    /// A list of the supported authentication schemes.
    ///
    /// TODO RPC: Actually, this should be able to contain strings _or_ maps,
    /// where the maps are additional information about the parameters needed
    /// for a particular scheme.  But I think that's a change we can make later
    /// once we have a scheme that takes parameters.
    ///
    /// TODO RPC: Should we indicate which schemes get you additional privileges?
    schemes: Vec<AuthenticationScheme>,
}

impl rpc::RpcMethod for AuthQuery {
    type Output = SupportedAuth;
    type Update = rpc::NoUpdates;
}
/// Implement `auth:AuthQuery` on a connection.
async fn conn_authquery(
    conn: Arc<Connection>,
    _query: Box<AuthQuery>,
    _ctx: Arc<dyn rpc::Context>,
) -> Result<SupportedAuth, rpc::RpcError> {
    use tor_rpc_connect::auth::RpcAuth;
    let schemes = match &conn.require_auth {
        RpcAuth::Inherent => vec![AuthenticationScheme::Inherent],
        RpcAuth::Cookie { .. } => {
            vec![AuthenticationScheme::Cookie]
        }
        _ => vec![],
    };
    Ok(SupportedAuth { schemes })
}
rpc::static_rpc_invoke_fn! {
    conn_authquery;
}

/// An error during authentication.
#[derive(Debug, Clone, thiserror::Error, serde::Serialize)]
enum AuthenticationFailure {
    /// The authentication method wasn't one we support.
    #[error("Tried to use unexpected authentication method")]
    IncorrectMethod,
    /// Tried to reuse a cookie authentication object
    #[error("Tried to re-authenticate with a cookie authentication object")]
    CookieNonceReused,
    /// Tried to provide a secret, MAC, or other object that wasn't correct.
    #[error("Incorrect authentication value")]
    IncorrectAuthentication,
    /// RPC system is shutting down; can't authenticate
    #[error("Shutting down; can't authenticate")]
    ShuttingDown,
}

/// A successful response from an authenticate method.
#[derive(Debug, serde::Serialize)]
struct AuthenticateReply {
    /// An handle for a `Session` object.
    session: rpc::ObjectId,
}
