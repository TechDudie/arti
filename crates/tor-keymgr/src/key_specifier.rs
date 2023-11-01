//! The [`KeySpecifier`] trait and its implementations.

use std::ops::Range;
use std::result::Result as StdResult;

use arrayvec::ArrayVec;
use derive_more::{Deref, DerefMut, Display, From, Into};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tor_error::{ErrorKind, HasKind};
use tor_hscrypto::time::TimePeriod;

use crate::KeystoreError;

/// The path of a key in the Arti key store.
///
/// An `ArtiPath` is a nonempty sequence of [`ArtiPathComponent`]s, separated by `/`.  Path
/// components may contain UTF-8 alphanumerics, and (except as the first or last character) `-`,
/// `_`, or  `.`.
/// Consequently, leading or trailing or duplicated / are forbidden.
///
/// The last component of the path may optionally contain the encoded (string) representation
/// of a [`KeyDenotator`] (obtained from [`KeyDenotator::encode`]).
/// The denotator is separated from the rest of the component by a single [`DENOTATOR_SEP`]
/// character. For example, the last component of the path `"foo/bar/bax+denotator_example"`
/// is `"bax+denotator_example"`, and the denotator is `"denotator_example"`.
/// Denotator strings are validited in the same way as [`ArtiPathComponent`]s.
///
/// NOTE: There is a 1:1 mapping between a value that implements `KeySpecifier` and its
/// corresponding `ArtiPath`. A `KeySpecifier` can be converted to an `ArtiPath`, but the reverse
/// conversion is not supported.
///
// TODO HSS: we should allow keys to have more than one `KeyDenotator`.
// See https://gitlab.torproject.org/tpo/core/arti/-/merge_requests/1722#note_2960442
//
// But this should be done _after_ we rewrite define_key_specifier using d-a
#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, Deref, DerefMut, Into, Display)]
pub struct ArtiPath(String);

/// The identifier of a key.
#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, From, Display)]
#[non_exhaustive]
pub enum KeyPath {
    /// An Arti key path.
    Arti(ArtiPath),
    /// A C-Tor key path.
    CTor(CTorPath),
}

/// A range specifying a substring of a [`KeyPath`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, From)]
pub struct KeyPathRange(Range<usize>);

impl KeyPath {
    /// Check whether this `KeyPath` matches the specified [`KeyPathPattern`].
    ///
    /// If the `KeyPath` matches the pattern, this returns the ranges that match its dynamic parts.
    pub fn matches(&self, pat: &KeyPathPatternSet) -> Option<Vec<KeyPathRange>> {
        let (pattern, path): (&str, &str) = match self {
            KeyPath::Arti(p) => (pat.arti.as_ref(), p.as_ref()),
            KeyPath::CTor(p) => (pat.ctor.as_ref(), p.as_ref()),
        };

        glob_match::glob_match_with_captures(pattern, path)
            .map(|res| res.into_iter().map(|r| r.into()).collect())
    }

    // TODO: rewrite these getters using derive_adhoc if KeyPath grows more variants.

    /// Return the underlying [`ArtiPath`], if this is a `KeyPath::Arti`.
    pub fn arti(&self) -> Option<&ArtiPath> {
        match self {
            KeyPath::Arti(ref arti) => Some(arti),
            KeyPath::CTor(_) => None,
        }
    }

    /// Return the underlying [`CTorPath`], if this is a `KeyPath::CTor`.
    pub fn ctor(&self) -> Option<&CTorPath> {
        match self {
            KeyPath::Arti(_) => None,
            KeyPath::CTor(ref ctor) => Some(ctor),
        }
    }
}

/// [`KeyPathPattern`]s that can be used to match [`ArtiPath`]s or [`CTorPath`]s.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Into)]
#[non_exhaustive]
pub struct KeyPathPatternSet {
    /// A pattern for matching [`ArtiPath`]s.
    pub arti: KeyPathPattern,
    /// A pattern for matching [`CTorPath`]s.
    pub ctor: KeyPathPattern,
}

impl KeyPathPatternSet {
    /// Create a new [`KeyPathPatternSet`].
    pub fn new(arti: KeyPathPattern, ctor: KeyPathPattern) -> Self {
        Self { arti, ctor }
    }
}

/// A pattern for matching [`ArtiPath`]s.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deref, DerefMut, Display)]
pub struct KeyPathPattern(String);

impl KeyPathPattern {
    /// Create a new `KeyPathPattern`.
    ///
    /// ## Syntax
    ///
    /// NOTE: this table is copied vebatim from the [`glob-match`] docs.
    ///
    /// | Syntax  | Meaning                                                                                                                                                                                             |
    /// | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
    /// | `?`     | Matches any single character.                                                                                                                                                                       |
    /// | `*`     | Matches zero or more characters, except for path separators (e.g. `/`).                                                                                                                             |
    /// | `**`    | Matches zero or more characters, including path separators. Must match a complete path segment (i.e. followed by a `/` or the end of the pattern).                                                  |
    /// | `[ab]`  | Matches one of the characters contained in the brackets. Character ranges, e.g. `[a-z]` are also supported. Use `[!ab]` or `[^ab]` to match any character _except_ those contained in the brackets. |
    /// | `{a,b}` | Matches one of the patterns contained in the braces. Any of the wildcard characters can be used in the sub-patterns. Braces may be nested up to 10 levels deep.                                     |
    /// | `!`     | When at the start of the glob, this negates the result. Multiple `!` characters negate the glob multiple times.                                                                                     |
    /// | `\`     | A backslash character may be used to escape any of the above special characters.                                                                                                                    |
    ///
    /// [`glob-match`]: https://crates.io/crates/glob-match
    pub fn new(inner: impl AsRef<str>) -> Self {
        Self(inner.as_ref().into())
    }

    /// Create a `KeyPathPattern` that only matches the empty string.
    ///
    /// This pattern will fail to match any valid [`ArtiPath`]s or [`CTorPath`]s.
    pub fn empty() -> Self {
        Self("".into())
    }
}

/// A separator for `ArtiPath`s.
const PATH_SEP: char = '/';

/// A separator for that marks the beginning of the [`KeyDenotator`]s
/// within an [`ArtiPath`].
///
/// This separator can only appear within the last component of an [`ArtiPath`],
/// and the substring that follows it is assumed to be the string representation
/// of the denotators of the path.
pub const DENOTATOR_SEP: char = '+';

impl ArtiPath {
    /// Create a new [`ArtiPath`].
    ///
    /// This function returns an error if `inner` is not a valid `ArtiPath`.
    pub fn new(inner: String) -> StdResult<Self, ArtiPathError> {
        // Validate the denotator, if there is one.
        let path = if let Some((inner, denotator)) = inner.rsplit_once(DENOTATOR_SEP) {
            let () = ArtiPathComponent::validate_str(denotator)?;

            inner
        } else {
            inner.as_ref()
        };

        if let Some(e) = path
            .split(PATH_SEP)
            .find_map(|s| ArtiPathComponent::validate_str(s).err())
        {
            return Err(e);
        }

        Ok(Self(inner))
    }

    /// Return the substring corresponding to the specified `range`.
    ///
    /// Returns `None` if `range` is not within the bounds of this `ArtiPath`.
    pub fn substring(&self, range: &KeyPathRange) -> Option<&str> {
        self.0.get(range.0.clone())
    }
}

/// A component of an [`ArtiPath`].
///
/// Path components may contain UTF-8 alphanumerics, and (except as the first or last character)
/// `-`,  `_`, or `.`.
#[derive(
    Clone,
    Debug,
    derive_more::Deref,
    derive_more::DerefMut,
    derive_more::Into,
    derive_more::Display,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct ArtiPathComponent(String);

impl ArtiPathComponent {
    /// Create a new [`ArtiPathComponent`].
    ///
    /// This function returns an error if `inner` is not a valid `ArtiPathComponent`.
    pub fn new(inner: String) -> StdResult<Self, ArtiPathError> {
        Self::validate_str(&inner)?;

        Ok(Self(inner))
    }

    /// Check whether `c` can be used within an `ArtiPathComponent`.
    fn is_allowed_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
    }

    /// Validate the underlying representation of an `ArtiPath` or `ArtiPathComponent`.
    fn validate_str(inner: &str) -> StdResult<(), ArtiPathError> {
        /// These cannot be the first or last chars of an `ArtiPath` or `ArtiPathComponent`.
        const MIDDLE_ONLY: &[char] = &['-', '_', '.'];

        if inner.is_empty() {
            return Err(ArtiPathError::EmptyPathComponent);
        }

        if let Some(c) = inner.chars().find(|c| !Self::is_allowed_char(*c)) {
            return Err(ArtiPathError::DisallowedChar(c));
        }

        if inner.contains("..") {
            return Err(ArtiPathError::PathTraversal);
        }

        for c in MIDDLE_ONLY {
            if inner.starts_with(*c) || inner.ends_with(*c) {
                return Err(ArtiPathError::BadOuterChar(*c));
            }
        }

        Ok(())
    }
}

impl TryFrom<String> for ArtiPathComponent {
    type Error = ArtiPathError;

    fn try_from(s: String) -> StdResult<ArtiPathComponent, ArtiPathError> {
        Self::new(s)
    }
}

impl AsRef<str> for ArtiPathComponent {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The path of a key in the C Tor key store.
#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, Deref, DerefMut, Into, Display)]
pub struct CTorPath(String);

/// The "specifier" of a key, which identifies an instance of a key.
///
/// [`KeySpecifier::arti_path()`] should uniquely identify an instance of a key.
pub trait KeySpecifier {
    /// The location of the key in the Arti key store.
    ///
    /// This also acts as a unique identifier for a specific key instance.
    fn arti_path(&self) -> StdResult<ArtiPath, ArtiPathUnavailableError>;

    /// The location of the key in the C Tor key store (if supported).
    ///
    /// This function should return `None` for keys that are recognized by Arti's key stores, but
    /// not by C Tor's key store (such as `HsClientIntroAuthKeypair`).
    fn ctor_path(&self) -> Option<CTorPath>;
}

/// An error returned by a [`KeySpecifier`].
///
/// The putative `KeySpecifier` might be simply invalid,
/// or it might be being used in an inappropriate context.
#[derive(Error, Debug, Clone)]
#[non_exhaustive]
pub enum ArtiPathUnavailableError {
    /// An internal error.
    #[error("Internal error")]
    Bug(#[from] tor_error::Bug),

    /// An error returned by a [`KeySpecifier`] that does not have an [`ArtiPath`].
    ///
    /// This is returned, for example, by [`CTorPath`]'s [`KeySpecifier::arti_path`]
    /// implementation.
    #[error("ArtiPath unvailable")]
    ArtiPathUnavailable,
}

/// An error caused by an invalid [`ArtiPath`].
#[derive(Error, Debug, Copy, Clone)]
#[error("Invalid ArtiPath")]
#[non_exhaustive]
pub enum ArtiPathError {
    /// Found an empty path component.
    #[error("Empty path component")]
    EmptyPathComponent,

    /// The path contains a disallowed char.
    #[error("Found disallowed char {0}")]
    DisallowedChar(char),

    /// The path contains the `..` pattern.
    #[error("Found `..` pattern")]
    PathTraversal,

    /// The path starts with a disallowed char.
    #[error("Path starts or ends with disallowed char {0}")]
    BadOuterChar(char),

    /// The path contains an invalid key denotator.
    ///
    /// See the [`ArtiPath`] docs for more information.
    InvalidDenotator,
}

/// An error caused by keystore corruption.
///
// TODO HSS: refactor the keymgr error types to be variants of a top-level KeyMgrError enum
// (it should only be necessary to impl KeystoreError for custom/opaque keystore errors).
#[derive(Error, Debug, Copy, Clone)]
#[error("Keystore corruption")]
#[non_exhaustive]
pub enum KeystoreCorruptionError {
    /// A keysotre contains a key that has an invalid [`ArtiPath`].
    #[error("{0}")]
    ArtiPath(#[from] ArtiPathError),
}

impl KeystoreError for KeystoreCorruptionError {}

impl HasKind for KeystoreCorruptionError {
    fn kind(&self) -> ErrorKind {
        ErrorKind::KeystoreCorrupted
    }
}

impl KeySpecifier for ArtiPath {
    fn arti_path(&self) -> StdResult<ArtiPath, ArtiPathUnavailableError> {
        Ok(self.clone())
    }

    fn ctor_path(&self) -> Option<CTorPath> {
        None
    }
}

impl KeySpecifier for CTorPath {
    fn arti_path(&self) -> StdResult<ArtiPath, ArtiPathUnavailableError> {
        Err(ArtiPathUnavailableError::ArtiPathUnavailable)
    }

    fn ctor_path(&self) -> Option<CTorPath> {
        Some(self.clone())
    }
}

impl KeySpecifier for KeyPath {
    fn arti_path(&self) -> StdResult<ArtiPath, ArtiPathUnavailableError> {
        match self {
            KeyPath::Arti(p) => p.arti_path(),
            KeyPath::CTor(p) => p.arti_path(),
        }
    }

    fn ctor_path(&self) -> Option<CTorPath> {
        match self {
            KeyPath::Arti(p) => p.ctor_path(),
            KeyPath::CTor(p) => p.ctor_path(),
        }
    }
}

/// A trait for displaying key denotators, for use within an [`ArtiPath`]
/// or [`CTorPath`].
///
/// A key's denotators *denote* an instance of a key.
pub trait KeyDenotator {
    /// Encode the denotators in a format that can be used within an
    /// [`ArtiPath`] or [`CTorPath`].
    fn encode(&self) -> String;

    /// Try to convert the specified string `s` to a value of this type.
    fn decode(s: &str) -> crate::Result<Self>
    where
        Self: Sized;
}

impl KeyDenotator for TimePeriod {
    fn encode(&self) -> String {
        format!(
            "{}_{}_{}",
            self.interval_num(),
            self.length(),
            self.epoch_offset_in_sec()
        )
    }

    fn decode(s: &str) -> crate::Result<Self>
    where
        Self: Sized,
    {
        let parts = s.split('_').collect::<ArrayVec<&str, 3>>();
        let [interval, len, offset]: [&str; 3] = parts
            .into_inner()
            .map_err(|_| KeystoreCorruptionError::from(ArtiPathError::InvalidDenotator))?;

        let (interval_num, length, offset_in_sec) = (|| {
            let length = len.parse().ok()?;
            let interval_num = interval.parse().ok()?;
            let offset_in_sec = offset.parse().ok()?;

            Some((interval_num, length, offset_in_sec))
        })()
        .ok_or_else(|| KeystoreCorruptionError::from(ArtiPathError::InvalidDenotator))?;

        Ok(TimePeriod::from_parts(length, interval_num, offset_in_sec))
    }
}

/// A helper for implementing [`KeySpecifier`]s.
///
/// Defines a `key_spec` struct, that has some static components (`prefix`, `role`),
/// and a number of variable components represented by its fields.
///
/// The `prefix` is the first component of the [`ArtiPath`] of the [`KeySpecifier`].
///
/// The `role` is the _prefix of the last component_ of the [`ArtiPath`] of the specifier.
/// The `role` is followed by the denotators of the key, if they are any.
///
/// The field that contains the denotators of the key, if there is one,
/// should be anotated with `#[denotator]`.
/// The denotator **must** implement [`KeyDenotator`],
/// and it **must** come before all the other fields.
/// The `#[denotator]` anotation can be used at most once.
///
/// The declaration order of the non-denotator fields is important.
/// The inner components of the [`ArtiPath`] of the specifier are built
/// from the string representation of its fields, taken in declaration order.
/// As such, all fields, except for the denotator, **must** implement
/// [`Display`](std::fmt::Display).
///
/// For example, a key specifier with `prefix` `"foo"` and `role` `"bar"`
/// will have an [`ArtiPath`] of the form
/// `"foo/<field1_str>/<field2_str>/../bar[_<denotator>]"`.
//
// TODO HSS: extend this to work for c-tor paths too (it will likely be a breaking
// change).
//
// TODO HSS: rewrite this using derive-ahoc
// See https://gitlab.torproject.org/tpo/core/arti/-/merge_requests/1710
#[macro_export]
macro_rules! define_key_specifier {
    {
        #[prefix = $prefix:expr]
        #[role = $role:expr]
        $( #[ $($attrs:meta)* ] )*
        $vis:vis struct $key_spec:ident $( [ $($gen:tt)+ ] )?
        $( where [ $($where_clauses:tt)* ] )?
        {
            #[denotator]
            $( #[ $($denotator_attrs:meta)* ] )*
            $denotator:ident : $denotator_ty:ty,

            $(
                $( #[ $($field_attrs:meta)* ] )*
                $field:ident : $field_ty:ty,
            )*
        }
    } => {
        $(#[ $($attrs)* ])*
        $vis struct $key_spec $( < $($gen)+ > )?
        $( where $($where_clauses)* )?
        {
            $(
                $( #[ $($field_attrs)* ] )*
                $field : $field_ty,
            )*

            $( #[ $($denotator_attrs)* ] )*
            $denotator: $denotator_ty,
        }

        impl $( < $($gen)* > )? $key_spec $( < $($gen)+ > )?
        $( where $($where_clauses)* )?
        {
            /// Create a new key specifier of this type.
            $vis fn new($($field: $field_ty,)* $denotator: $denotator_ty) -> Self {

                Self { $($field,)* $denotator }
            }

            /// Get an [`KeyPathPattern`] that can match the [`ArtiPath`]s
            /// of all the keys of this type.
            ///
            /// This builds a pattern by joining the `prefix` of this specifier
            /// with the specified field values, its `role`, and the glob
            /// pattern returned by the [`KeyDenotator::glob`] implementation
            /// of its denotator.
            $vis fn arti_pattern($($field: &$field_ty,)*) -> $crate::KeyPathPattern {
                let pat = Self::arti_path_prefix($(&$field,)*);
                KeyPathPattern::new(format!("{pat}{}*", $crate::DENOTATOR_SEP))
            }
        }

        define_key_specifier! {
            impl_common

            #[prefix = $prefix]
            #[role = $role]
            $(#[ $($attrs)* ])*
            $vis struct $key_spec $( [ $($gen)+ ] )?
            $( where $($where_clauses)* )?
            {
                $(
                    $( #[ $($field_attrs)* ] )*
                    $field: $field_ty,
                )*
            }
        }

        impl $( < $($gen)* > )? $crate::KeySpecifier for $key_spec $( < $($gen)* > )?
        $( where $($where_clauses)* )?
        {
            fn arti_path(&self) -> Result<$crate::ArtiPath, $crate::ArtiPathUnavailableError> {
                let prefix = self.prefix();
                let denotator = $crate::KeyDenotator::encode(&self.$denotator);
                let path = format!("{prefix}{}{denotator}", $crate::DENOTATOR_SEP);

                Ok($crate::ArtiPath::new(path).map_err(|e| tor_error::internal!("{e}"))?)
            }

            fn ctor_path(&self) -> Option<$crate::CTorPath> {
                // TODO HSS: the HsSvcKeySpecifier will need to be configured with all the directories used
                // by C tor. The resulting CTorPath will be prefixed with the appropriate C tor directory,
                // based on the HsSvcKeyRole.
                //
                // This function will return `None` for keys that aren't stored on disk by C tor.
                todo!()
            }
        }
    };

    {
        #[prefix = $prefix:expr]
        #[role = $role:expr]
        $( #[ $($attrs:meta)* ] )*
        $vis:vis struct $key_spec:ident $( [ $($gen:tt)+ ] )?
        $( where [ $($where_clauses:tt)* ] )?
        {
            $(
                $( #[ $($field_attrs:meta)* ] )*
                $field:ident : $field_ty:ty,
            )*
        }
    } => {
        $(#[ $($attrs)* ])*
        $vis struct $key_spec $( < $($gen)+ > )? {
            $(
                $( #[ $($field_attrs)* ] )*
                $field: $field_ty,
            )*
        }

        impl $( < $($gen)* > )? $key_spec $( < $($gen)* > )?
        $( where $($where_clauses)* )?
        {
            /// Create a new key specifier of this type.
            $vis fn new($($field: $field_ty,)*) -> Self {
                Self { $($field,)* }
            }

            /// Get an [`KeyPathPattern`] that can match the [`ArtiPath`]s corresponding to the key
            /// corresponding to the specified service `nickname` and `role`.
            $vis fn arti_pattern($($field: $field_ty,)*) -> KeyPathPattern {
                let pat = Self::arti_path_prefix($(&$field,)*);
                KeyPathPattern::new(pat)
            }
        }

        impl $( < $($gen)* > )? $crate::KeySpecifier for $key_spec $( < $($gen)* > )?
        $( where $($where_clauses)* )?
        {
            fn arti_path(&self) -> Result<$crate::ArtiPath, $crate::ArtiPathUnavailableError> {
                let prefix = self.prefix();
                Ok($crate::ArtiPath::new(prefix).map_err(|e| tor_error::internal!("{e}"))?)
            }

            fn ctor_path(&self) -> Option<$crate::CTorPath> {
                // TODO HSS: the HsSvcKeySpecifier will need to be configured with all the directories used
                // by C tor. The resulting CTorPath will be prefixed with the appropriate C tor directory,
                // based on the HsSvcKeyRole.
                //
                // This function will return `None` for keys that aren't stored on disk by C tor.
                todo!()
            }
        }

        define_key_specifier! {
            impl_common

            #[prefix = $prefix]
            #[role = $role]
            $(#[ $($attrs)* ])*
            $vis struct $key_spec $( [ $($gen)+ ] )?
            $( where $($where_clauses)* )?
            {
                $(
                    $( #[ $($field_attrs)* ] )*
                    $field: $field_ty,
                )*
            }
        }
    };

    // A helper branch for implementing the functions shared by all key specifiers.
    {
        impl_common

        #[prefix = $prefix:expr]
        #[role = $role:expr]
        $( #[ $($attrs:meta)* ] )*
        $vis:vis struct $key_spec:ident $( [ $($gen:tt)+ ] )?
        $( where [ $($where_clauses:tt)* ] )?
        {
            $(
                $( #[ $($field_attrs:meta)* ] )*
                $field:ident : $field_ty:ty,
            )*
        }
    } => {
        impl $( < $($gen)* > )? $key_spec $( < $($gen)+ > )?
        $( where $($where_clauses)* )?
        {
            /// Return the role of this specifier.
            $vis fn role(&self) -> &'static str {
                $role
            }

            /// A helper for generating the prefix shared by all `ArtiPath`s
            /// of the keys associated with this specifier.
            ///
            /// Returns the `ArtiPath`, minus the denotators.
            $vis fn arti_path_prefix($($field: &$field_ty,)*) -> String {
                vec![
                    $prefix.to_string(),
                    $(
                        $field.to_string(),
                    )*
                    $role.to_string()
                ].join("/")
            }

            /// A convenience wrapper around `Self::arti_path_prefix`.
            fn prefix(&self) -> String {
                Self::arti_path_prefix($(&self.$field,)*)
            }
        }
    }
}

#[cfg(test)]
mod test {
    // @@ begin test lint list maintained by maint/add_warning @@
    #![allow(clippy::bool_assert_comparison)]
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::dbg_macro)]
    #![allow(clippy::print_stderr)]
    #![allow(clippy::print_stdout)]
    #![allow(clippy::single_char_pattern)]
    #![allow(clippy::unwrap_used)]
    #![allow(clippy::unchecked_duration_subtraction)]
    #![allow(clippy::useless_vec)]
    #![allow(clippy::needless_pass_by_value)]
    //! <!-- @@ end test lint list maintained by maint/add_warning @@ -->
    use super::*;

    macro_rules! assert_err {
        ($ty:ident, $inner:expr, $error_kind:pat) => {{
            let path = $ty::new($inner.to_string());
            assert!(path.is_err(), "{} should be invalid", $inner);
            assert!(
                matches!(path.as_ref().unwrap_err(), $error_kind),
                "wrong error type for {}: {path:?}",
                $inner
            );
        }};
    }

    macro_rules! assert_ok {
        ($ty:ident, $inner:expr) => {{
            let path = $ty::new($inner.to_string());
            assert!(path.is_ok(), "{} should be valid", $inner);
            assert_eq!(path.unwrap().to_string(), *$inner);
        }};
    }

    impl KeyDenotator for usize {
        fn encode(&self) -> String {
            self.to_string()
        }

        fn decode(s: &str) -> crate::Result<Self>
        where
            Self: Sized,
        {
            use std::str::FromStr;

            Ok(usize::from_str(s).unwrap())
        }
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn arti_path_validation() {
        const VALID_ARTI_PATHS: &[&str] = &[
            "my-hs-client-2",
            "hs_client",
            "client٣¾",
            "clientß",
            "client.key",
        ];

        const BAD_OUTER_CHAR_ARTI_PATHS: &[&str] = &[
            "-hs_client",
            "_hs_client",
            "hs_client-",
            "hs_client_",
            ".client",
            "client.",
            "-",
            "_",
        ];

        const DISALLOWED_CHAR_ARTI_PATHS: &[&str] = &["client?", "no spaces please"];

        const EMPTY_PATH_COMPONENT: &[&str] =
            &["/////", "/alice/bob", "alice//bob", "alice/bob/", "/"];

        for path in VALID_ARTI_PATHS {
            assert_ok!(ArtiPath, path);
            assert_ok!(ArtiPathComponent, path);
        }

        for path in DISALLOWED_CHAR_ARTI_PATHS {
            assert_err!(ArtiPath, path, ArtiPathError::DisallowedChar(_));
            assert_err!(ArtiPathComponent, path, ArtiPathError::DisallowedChar(_));
        }

        for path in BAD_OUTER_CHAR_ARTI_PATHS {
            assert_err!(ArtiPath, path, ArtiPathError::BadOuterChar(_));
            assert_err!(ArtiPathComponent, path, ArtiPathError::BadOuterChar(_));
        }

        for path in EMPTY_PATH_COMPONENT {
            assert_err!(ArtiPath, path, ArtiPathError::EmptyPathComponent);
            assert_err!(ArtiPathComponent, path, ArtiPathError::DisallowedChar('/'));
        }

        const SEP: char = PATH_SEP;
        // This is a valid ArtiPath, but not a valid ArtiPathComponent
        let path = format!("a{SEP}client{SEP}key.private");
        assert_ok!(ArtiPath, &path);
        assert_err!(ArtiPathComponent, &path, ArtiPathError::DisallowedChar('/'));

        const PATH_WITH_TRAVERSAL: &str = "alice/../bob";
        assert_err!(ArtiPath, PATH_WITH_TRAVERSAL, ArtiPathError::PathTraversal);
        assert_err!(
            ArtiPathComponent,
            PATH_WITH_TRAVERSAL,
            ArtiPathError::DisallowedChar('/')
        );

        const REL_PATH: &str = "./bob";
        assert_err!(ArtiPath, REL_PATH, ArtiPathError::BadOuterChar('.'));
        assert_err!(
            ArtiPathComponent,
            REL_PATH,
            ArtiPathError::DisallowedChar('/')
        );

        const EMPTY_DENOTATOR: &str = "c++";
        assert_err!(ArtiPath, EMPTY_DENOTATOR, ArtiPathError::EmptyPathComponent);
        assert_err!(
            ArtiPathComponent,
            EMPTY_DENOTATOR,
            ArtiPathError::DisallowedChar('+')
        );
    }

    #[test]
    fn arti_path_with_denotator() {
        const VALID_ARTI_DENOTATORS: &[&str] = &["foo", "one_two_three-f0ur"];

        const BAD_OUTER_CHAR_DENOTATORS: &[&str] =
            &["1-2-3-", "1-2-3_", "1-2-3.", "-1-2-3", "_1-2-3", ".1-2-3"];

        for denotator in VALID_ARTI_DENOTATORS {
            let path = format!("foo/bar/qux+{denotator}");
            assert_ok!(ArtiPath, path);
            assert_ok!(ArtiPathComponent, denotator);
        }

        for denotator in BAD_OUTER_CHAR_DENOTATORS {
            let path = format!("hs_client+{denotator}");

            assert_err!(ArtiPath, path, ArtiPathError::BadOuterChar(_));
            assert_err!(ArtiPathComponent, denotator, ArtiPathError::BadOuterChar(_));
            assert_err!(ArtiPathComponent, path, ArtiPathError::DisallowedChar('+'));
        }
    }

    #[test]
    fn serde() {
        // TODO HSS clone-and-hack with tor_hsservice::::nickname::test::serde
        // perhaps there should be some utility in tor-basic-utils for testing
        // validated string newtypes, or something
        #[derive(Serialize, Deserialize, Debug)]
        struct T {
            n: ArtiPathComponent,
        }
        let j = serde_json::from_str(r#"{ "n": "x" }"#).unwrap();
        let t: T = serde_json::from_value(j).unwrap();
        assert_eq!(&t.n.to_string(), "x");

        assert_eq!(&serde_json::to_string(&t).unwrap(), r#"{"n":"x"}"#);

        let j = serde_json::from_str(r#"{ "n": "!" }"#).unwrap();
        let e = serde_json::from_value::<T>(j).unwrap_err();
        assert!(
            e.to_string().contains("Found disallowed char"),
            "wrong msg {e:?}"
        );
    }

    #[test]
    fn substring() {
        const KEY_PATH: &str = "hello";
        let path = ArtiPath::new(KEY_PATH.to_string()).unwrap();

        assert_eq!(path.substring(&(0..1).into()).unwrap(), "h");
        assert_eq!(path.substring(&(2..KEY_PATH.len()).into()).unwrap(), "llo");
        assert_eq!(
            path.substring(&(0..KEY_PATH.len()).into()).unwrap(),
            "hello"
        );
        assert_eq!(path.substring(&(0..KEY_PATH.len() + 1).into()), None);
        assert_eq!(path.substring(&(0..0).into()).unwrap(), "");
    }

    #[allow(dead_code)] // some of the auto-generated functions are unused
    #[test]
    fn define_key_specifier_with_fields_and_denotator() {
        define_key_specifier!(
            #[prefix = "encabulator"]
            #[role = "marzlevane"]
            struct TestSpecifier {
                #[denotator]
                /// The denotator.
                count: usize,

                // The remaining fields
                kind: String,
                base: String,
                casing: String,
            }
        );

        let key_spec = TestSpecifier {
            kind: "hydrocoptic".into(),
            base: "waneshaft".into(),
            casing: "logarithmic".into(),
            count: 6,
        };

        assert_eq!(
            key_spec.arti_path().unwrap().as_str(),
            "encabulator/hydrocoptic/waneshaft/logarithmic/marzlevane+6"
        );
        assert_eq!(key_spec.role(), "marzlevane");
    }

    #[allow(dead_code)] // some of the auto-generated functions are unused
    #[test]
    fn define_key_specifier_no_fields() {
        define_key_specifier!(
            #[prefix = "encabulator"]
            #[role = "marzlevane"]
            struct TestSpecifier {}
        );

        let key_spec = TestSpecifier {};

        assert_eq!(
            key_spec.arti_path().unwrap().as_str(),
            "encabulator/marzlevane"
        );
        assert_eq!(key_spec.role(), "marzlevane");
    }

    #[allow(dead_code)] // some of the auto-generated functions are unused
    #[test]
    fn define_key_specifier_with_denotator() {
        define_key_specifier!(
            #[prefix = "encabulator"]
            #[role = "marzlevane"]
            struct TestSpecifier {
                #[denotator]
                count: usize,
            }
        );

        let key_spec = TestSpecifier { count: 6 };

        assert_eq!(
            key_spec.arti_path().unwrap().as_str(),
            "encabulator/marzlevane+6"
        );
        assert_eq!(key_spec.role(), "marzlevane");
    }

    #[allow(dead_code)] // some of the auto-generated functions are unused
    #[test]
    fn define_key_specifier_with_fields() {
        define_key_specifier!(
            #[prefix = "encabulator"]
            #[role = "fan"]
            struct TestSpecifier {
                casing: String,
                /// A doc comment.
                bearings: String,
            }
        );

        let key_spec = TestSpecifier {
            casing: "logarithmic".into(),
            bearings: "spurving".into(),
        };

        assert_eq!(
            key_spec.arti_path().unwrap().as_str(),
            "encabulator/logarithmic/spurving/fan"
        );
        assert_eq!(key_spec.role(), "fan");
    }

    #[test]
    fn encode_time_period() {
        let period = TimePeriod::from_parts(1, 2, 3);
        let encoded_period = period.encode();

        assert_eq!(encoded_period, "2_1_3");
        assert_eq!(period, TimePeriod::decode(&encoded_period).unwrap());

        assert!(TimePeriod::decode("invalid_tp").is_err());
    }
}
