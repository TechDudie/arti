//! Declaration for an n-keyed set type, allowing access to each of its members by each of N different keys.

// Re-export dependencies that we use to make this macro work.
#[doc(hidden)]
pub mod deps {
    pub use paste::paste;
    pub use slab::Slab;
}

/// Declare a structure that can hold elements with multiple unique keys.
///
/// Each element can be looked up or removed by any of its keys. The keys
/// themselves can be any type that supports `Hash`, `Eq`, and `Clone`. Elements
/// can have multiple keys of the same type: for example, a person can have a
/// firstname `String` and a lastname `String`.
///
/// All keys in the set must be unique: if a new element is inserted that has
/// the same value for any key as a previous element, the old element is
/// removed.
///
/// Keys may be accessed from elements either by field access or by an accessor
/// function.
///
/// Keys may be optional.  If all keys are optional, then we require
/// additionally that every element must have at least one key.
///
/// # Examples
///
/// ```
/// use tor_basic_utils::n_key_set;
///
/// // We declare a person struct with several different fields.
/// pub struct Person {
///     username: String,
///     given_name: String,
///     student_id: Option<u64>,
///     favorite_joke: Option<String>,
/// }
///
/// n_key_set! {
///     pub struct PersonSet for Person {
///         // See note on "Key syntax" below.  The ".foo" syntax
///         // here means that the value for the key is returned
///         // by accessing a given field.
///         username: String { .username },
///         given_name: String { .given_name },
///         student_id: Option<u64> { .student_id }
///     }
/// }
///
/// let mut people = PersonSet::new();
/// people.insert(Person {
///     username: "mina".into(),
///     given_name: "Mina Harker".into(),
///     student_id: None,
///     favorite_joke: None
/// });
/// assert!(people.by_username("mina").is_some());
/// assert!(people.by_given_name("Mina Harker").is_some());
/// ```
///
/// # Key syntax
///
/// You can access the keys of an element in any of several ways.
///
/// * `name : type { func() }` - A key whose name is `name` and type is `type`,
///   that can be accessed from a given element by calling `element.func()`.
/// * name : type { .field }` - A key whose name is `name` and type is `type`,
///   that can be accessed from a given element by calling `&element.field`.
/// * `name : type` - Short for as `name : type { name() }`.
///
/// If the type of a key is given as `Option<type2>`, then the inner `type2` is
/// treated as the real key type, and the key is treated as optional.
///
/// # Additional features
///
/// You can put generic parameters and `where` constraints on your structure.
#[macro_export]
macro_rules! n_key_set {
{
    $(#[$meta:meta])*
    $vis:vis struct $mapname:ident $(<$($P:ident),*>)? for $V:ty
    $( where $($constr:tt)+ )?
    {
        $( $key:ident : $KEY:ty $({ $($source:tt)+ })? ),+
        $(,)?
    }
} => {
$crate::n_key_set::deps::paste!{
   $( #[$meta] )*
    #[doc = concat!("
        A set of elements of type ", stringify!($V), " whose members can be 
        accessed by multiple keys.

        The keys are:
        ",
        $( " * `", stringify!($key), "` (`",stringify!($ty),"`)\n" , )+
        "

        The set contains at most one member for any value of a given key.

        # Requirements

        Key types must have consistent `Hash` and `Eq` implementations, as
        they will be used as keys in a `HashSet`.

        If all keys are of type `Option<T>`, then every element in this set
        must have at least one non-None key.

        An element must not change its keys over time through interior
        mutability.
        
        # Limitations

        This could be more efficient in space and time.
        "
    )]
    $vis struct $mapname $(<$($P),*>)?
        where $( $KEY : std::hash::Hash + Eq + Clone , )+  $($($constr)+)?
    {
        // The $key fields here are a set of maps from each of the key values to
        // the position of that value within the Slab..
        //
        // Invariants:
        //    * There is an entry K=>idx in the map `$key` if and only if
        //      values[idx].$accessor() == K.
        //
        // TODO: Dare we have these HashMaps key based on a reference to V
        // instead? That would create a self-referential structure and require
        // unsafety.  Probably best to avoid that for now.
        $($key: std::collections::HashMap<$KEY, usize> , )+

        // A map from the indices to the values.
        values: $crate::n_key_set::deps::Slab<$V>,
    }

    #[allow(dead_code)] // May be needed if this is not public.
    impl $(<$($P),*>)? $mapname $(<$($P),*>)?
        where $( $KEY : std::hash::Hash + Eq + Clone , )+  $($($constr)+)?
    {
        #[doc = concat!("Construct a new ", stringify!($mapname))]
        $vis fn new() -> Self {
            Self::with_capacity(0)
        }
        #[doc = concat!("Construct a new ", stringify!($mapname), " with a given capacity.")]

        $vis fn with_capacity(n: usize) -> Self {
            Self {
                $($key: std::collections::HashMap::with_capacity(n),)*
                values: $crate::n_key_set::deps::Slab::with_capacity(n),
            }
        }
        $(
        #[doc = concat!("Return a reference to the element whose `", stringify!($key), "` is `key`.
        
        Return None if there is no such element.")]
        $vis fn [<by_ $key>] <T>(&self, key: &T) -> Option<&$V>
            where $KEY : std::borrow::Borrow<T>,
                  T: std::hash::Hash + Eq + ?Sized
        {
            self.$key.get(key).and_then(|idx| self.values.get(*idx))
        }

        /*  Removed: This seems too risky for real life.

        #[doc = concat!("Return a mutable reference to the element whose `", stringify!($key), "` is `key`.

        Return None if there is no such element.

        # Correctness

        This reference must not be used to change the value of any of the resulting
        element's keys: doing so can invalidate this set.
        ")]
        $vis fn [<by_ $key _mut>] <T>(&mut self, $key: &T) -> Option<&mut $V>
            where $KEY : std::borrow::Borrow<T>,
                  T: std::hash::Hash + Eq + ?Sized
        {
            self.$key.get($key).and_then(|idx| self.values.get_mut(*idx))
        }

        */

        #[doc = concat!("Return true if this set contains an element whose `", stringify!($key), "` is `key`.")]
        $vis fn [<contains_ $key>] <T>(&mut self, $key: &T) -> bool
        where $KEY : std::borrow::Borrow<T>,
              T: std::hash::Hash + Eq + ?Sized
        {
            self.$key.get($key).is_some()
        }

        #[doc = concat!("Remove the element whose `", stringify!($key), "` is `key`.
        
        Return that element on success, and None if there is no such element.")]
        #[doc=stringify!($key)]
        $vis fn [<remove_by_ $key>] <T>(&mut self, $key: &T) -> Option<$V>
            where $KEY : std::borrow::Borrow<T>,
                  T: std::hash::Hash + Eq + ?Sized
        {
            self.$key.get($key).copied().and_then(|old_idx| self.remove_at(old_idx))
        }
        )+

        /// Return an iterator over the elements in this container.
        $vis fn values(&self) -> impl Iterator<Item=&$V> + '_ {
            self.values.iter().map(|(_, v)| v)
        }

        /// Consumer this container and return an iterator of its values.
        $vis fn into_values(self) -> impl Iterator<Item=$V> {
            self.values.into_iter().map(|(_, v)| v)
        }

        /// Insert the value `value`.
        ///
        /// Remove any previous values that shared any keys with `value`, and
        /// return them in a vector.
        $vis fn insert(&mut self, value: $V) -> Vec<$V> {
            if self.capacity() > 32 && self.len() < self.capacity() / 4 {
                // We're have the opportunity to free up a fair amount of space; let's take it.
                self.compact()
            }

            // First, remove all the elements that have at least one key in common with `value`.
            let mut replaced = Vec::new();
            $(
                $crate::n_key_set!( @access(value, $key : $KEY $({$($source)+})?) )
                    .and_then(|key| self.$key.get(key))
                    .and_then(|idx| self.values.try_remove(*idx))
                    .map(|val| replaced.push(val));
            )*

            // Now insert the new value, and add it to all of the maps.
            let new_idx = self.values.insert(value);
            let value_ref = self.values.get(new_idx).expect("we just inserted this");
            let mut some_key_found = false;
            $(
                $crate::n_key_set!( @access(value_ref, $key : $KEY $({$($source)+})?) )
                    .map(|key| {
                        self.$key.insert(key.to_owned(), new_idx);
                        some_key_found = true;
                    });
            )*
            // If we didn't find any key on the newly added value, that's
            // an invariant violation.
            debug_assert!(some_key_found);

            replaced
        }

        /// Return the number of elements in this container.
        $vis fn len(&self) -> usize {
            self.values.len()
        }

        /// Return true if there are no elements in this container.
        $vis fn is_empty(&self) -> bool {
            self.values.len() == 0
        }

        /// Return the number of elements for which this container has allocated
        /// storage.
        $vis fn capacity(&self) -> usize {
            self.values.capacity()
        }

        /// Remove every element that does not satisfy the predicate `pred`.
        $vis fn retain<F>(&mut self, mut pred: F)
            where F: FnMut(&$V) -> bool,
        {
            for idx in 0..self.values.capacity() {
                if self.values.get(idx).map(&mut pred) == Some(false) {
                    self.remove_at(idx);
                }
            }
        }

        /// Helper: remove the item stored at index `idx`, and remove it from
        /// every key map.
        ///
        /// If there was no element at `idx`, do nothing.
        ///
        /// Return the element removed (if any).
        fn remove_at(&mut self, idx: usize) -> Option<$V> {
            if let Some(removed) = self.values.try_remove(idx) {
                $(
                    if let Some($key) = $crate::n_key_set!( @access(removed, $key : $KEY $({$($source)+})?) ) {
                        let old_idx = self.$key.remove($key);
                        debug_assert_eq!(old_idx, Some(idx));
                    }
                )*
                Some(removed)
            } else {
                None
            }
        }

        /// Re-index all the values in this map, so that the map can use a more
        /// compact representation.
        ///
        /// This should be done infrequently; it's expensive.
        fn compact(&mut self) {
            let old_value = std::mem::replace(self, Self::with_capacity(self.len()));
            for item in old_value.into_values() {
                self.insert(item);
            }
        }
    }

    impl $(<$($P),*>)? Default for $mapname $(<$($P),*>)?
        where $( $KEY : std::hash::Hash + Eq + Clone , )*  $($($constr)+)?
    {
        fn default() -> Self {
            $mapname::new()
        }
    }

    impl $(<$($P),*>)? FromIterator<$V> for $mapname $(<$($P),*>)?
        where $( $KEY : std::hash::Hash + Eq + Clone , )*  $($($constr)+)?
    {
        fn from_iter<T>(iter: T) -> Self
        where
            T: IntoIterator<Item = $V>
        {
            let iter = iter.into_iter();
            let mut set = Self::with_capacity(iter.size_hint().0);
            for value in iter {
                set.insert(value);
            }
            set
        }
    }
}
};

// Helper: Generate an expression to access a specific key and return
// an Option<&TYPE> for that key.  This is the part of the macro
// that parses key descriptions.

{ @access($ex:expr, $key:ident : Option<$t:ty> ) } => {
    $ex.key()
};
{ @access($ex:expr, $key:ident : $t:ty) } => {
    Some($ex.key())
};
{ @access($ex:expr, $key:ident : Option<$t:ty> { . $field:tt } ) } => {
    $ex.$field.as_ref()
};
{ @access($ex:expr, $key:ident : $t:ty { . $field:tt } ) } => {
   Some(&$ex.$field)
};
{ @access($ex:expr, $key:ident : Option<$t:ty> { $func:ident () } ) } => {
    $ex.$func()
};
{ @access($ex:expr, $key:ident : $t:ty { $func:ident () } ) } => {
    Some($ex.$func())
};
}

#[cfg(test)]
mod test {
    // @@ begin test lint list maintained by maint/add_warning @@
    #![allow(clippy::bool_assert_comparison)]
    #![allow(clippy::clone_on_copy)]
    #![allow(clippy::dbg_macro)]
    #![allow(clippy::print_stderr)]
    #![allow(clippy::print_stdout)]
    #![allow(clippy::unwrap_used)]
    //! <!-- @@ end test lint list maintained by maint/add_warning @@ -->

    n_key_set! {
        #[derive(Clone, Debug)]
        struct Tuple2Set<A,B> for (A,B) {
            first: A { .0 },
            second: B { .1 },
        }
    }

    #[test]
    fn basic() {
        let mut set = Tuple2Set::new();
        assert!(set.is_empty());

        set.insert((0_u32, 99_u16));
        assert_eq!(set.contains_first(&0), true);
        assert_eq!(set.contains_second(&99), true);
        assert_eq!(set.contains_first(&99), false);
        assert_eq!(set.contains_second(&0), false);
        assert_eq!(set.by_first(&0), Some(&(0, 99)));
        assert_eq!(set.by_second(&99), Some(&(0, 99)));
        assert_eq!(set.by_first(&99), None);
        assert_eq!(set.by_second(&0), None);

        assert_eq!(set.insert((12, 34)), vec![]);
        assert_eq!(set.len(), 2);
        assert!(set.capacity() >= 2);
        assert_eq!(set.by_first(&0), Some(&(0, 99)));
        assert_eq!(set.by_first(&12), Some(&(12, 34)));
        assert_eq!(set.remove_by_second(&99), Some((0, 99)));
        assert_eq!(set.len(), 1);

        // no overlap in these next few inserts.
        set.insert((34, 56));
        set.insert((56, 78));
        set.insert((78, 90));
        assert_eq!(set.len(), 4);
        // This insert replaces (12, 34)
        assert_eq!(set.insert((12, 123)), vec![(12, 34)]);
        // This one replaces (12,123) and (34,56).
        let mut replaced = set.insert((12, 56));
        replaced.sort();
        assert_eq!(replaced, vec![(12, 123), (34, 56)]);
        assert_eq!(set.len(), 3);
        assert_eq!(set.is_empty(), false);

        // Test our iterators
        let mut all_members: Vec<_> = set.values().collect();
        all_members.sort();
        assert_eq!(all_members, vec![&(12, 56), &(56, 78), &(78, 90)]);

        let mut drained_members: Vec<_> = set.into_values().collect();
        drained_members.sort();
        assert_eq!(drained_members, vec![(12, 56), (56, 78), (78, 90)]);
    }

    #[test]
    fn retain_and_compact() {
        let mut set: Tuple2Set<String, String> = (1..=1000)
            .map(|idx| (format!("A={}", idx), format!("B={}", idx)))
            .collect();

        assert_eq!(set.len(), 1000);
        let cap_orig = set.capacity();
        assert!(cap_orig >= set.len());

        // Retain only the values whose first key is 3 characters long.
        // That's 9 values out of 1000.
        set.retain(|(a, _)| a.len() <= 3);
        assert_eq!(set.len(), 9);
        // We don't shrink till we next insert.
        assert_eq!(set.capacity(), cap_orig);

        assert!(set
            .insert(("A=0".to_string(), "B=0".to_string()))
            .is_empty());
        assert!(set.capacity() < cap_orig);
        assert_eq!(set.len(), 10);
        for idx in 0..=9 {
            assert!(set.contains_first(&format!("A={}", idx)));
        }
    }

    #[allow(dead_code)]
    struct Weekday {
        dow: u8,
        name: &'static str,
        lucky_number: Option<u16>,
    }
    #[allow(dead_code)]
    impl Weekday {
        fn dow(&self) -> &u8 {
            &self.dow
        }
        fn name(&self) -> &str {
            self.name
        }
        fn lucky_number(&self) -> Option<&u16> {
            self.lucky_number.as_ref()
        }
    }
    n_key_set! {
        struct WeekdaySet for Weekday {
            idx: u8 { dow() },
           // BUG: Apparently Option isn't working right.
           //      lucky: Option<u16> { lucky_number() }

            name: String { name() }
        }
    }
}
