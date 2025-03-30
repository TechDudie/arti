//! The [`Keystore`] trait and its implementations.

pub(crate) mod arti;
#[cfg(feature = "ctor-keystore")]
pub(crate) mod ctor;
pub(crate) mod fs_utils;

#[cfg(feature = "ephemeral-keystore")]
pub(crate) mod ephemeral;

use tor_key_forge::{EncodableItem, ErasedKey, KeystoreItemType};

use crate::{KeyPath, KeySpecifier, KeystoreId, Result};

/// A generic key store.
pub trait Keystore: Send + Sync + 'static {
    /// An identifier for this key store instance.
    ///
    /// This identifier is used by some [`KeyMgr`](crate::KeyMgr) APIs to identify a specific key
    /// store.
    fn id(&self) -> &KeystoreId;

    /// Check if the key identified by `key_spec` exists in this key store.
    fn contains(&self, key_spec: &dyn KeySpecifier, item_type: &KeystoreItemType) -> Result<bool>;

    /// Retrieve the key identified by `key_spec`.
    ///
    /// Returns `Ok(Some(key))` if the key was successfully retrieved. Returns `Ok(None)` if the
    /// key does not exist in this key store.
    fn get(
        &self,
        key_spec: &dyn KeySpecifier,
        item_type: &KeystoreItemType,
    ) -> Result<Option<ErasedKey>>;

    /// Write `key` to the key store.
    //
    // TODO (BREAKING CHANGE):
    // Param `_item_type` is now redundant. Drop `_item_type` in the next breaking change.
    fn insert(
        &self,
        key: &dyn EncodableItem,
        key_spec: &dyn KeySpecifier,
        _item_type: &KeystoreItemType,
    ) -> Result<()>;

    /// Remove the specified key.
    ///
    /// A return value of `Ok(None)` indicates the key doesn't exist in this key store, whereas
    /// `Ok(Some(())` means the key was successfully removed.
    ///
    /// Returns `Err` if an error occurred while trying to remove the key.
    fn remove(
        &self,
        key_spec: &dyn KeySpecifier,
        item_type: &KeystoreItemType,
    ) -> Result<Option<()>>;

    /// List all the keys in this keystore.
    fn list(&self) -> Result<Vec<(KeyPath, KeystoreItemType)>>;
}
