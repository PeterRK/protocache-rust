//! Mutable surface matching `access-ex.h`.

use std::array;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;

use crate::access::checked_slice_end;
use crate::serialize::serialize_map_pairs_at_mut;
use crate::{
    ArrayView, Buffer, FieldView, MapKey, MapView, MessageView, Scalar, StringView, Unit,
    build_perfect_hash_index_with_positions, fold_field, serialize_array_at_mut, serialize_bool,
    serialize_bytes, serialize_scalar, serialize_str,
};

#[derive(Debug)]
/// Error returned while decoding or serializing a generated mutable message.
pub enum MutableError {
    InvalidRoot {
        descriptor: String,
    },
    MissingField {
        descriptor: String,
        field: String,
    },
    TypeMismatch {
        descriptor: String,
        field: String,
        expected: &'static str,
    },
    InvalidMapKey {
        descriptor: String,
        field: String,
        key: String,
    },
    SerializeFailed {
        descriptor: String,
        field: String,
    },
}

impl std::fmt::Display for MutableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot { descriptor } => write!(f, "invalid root for {descriptor}"),
            Self::MissingField { descriptor, field } => {
                write!(f, "missing field {field} in {descriptor}")
            }
            Self::TypeMismatch {
                descriptor,
                field,
                expected,
            } => write!(
                f,
                "type mismatch for {descriptor}.{field}, expected {expected}"
            ),
            Self::InvalidMapKey {
                descriptor,
                field,
                key,
            } => write!(f, "invalid map key {key} for {descriptor}.{field}"),
            Self::SerializeFailed { descriptor, field } => {
                write!(f, "failed to serialize {descriptor}.{field}")
            }
        }
    }
}

impl std::error::Error for MutableError {}

#[derive(Clone, Copy)]
/// The wire-level key representation used by a mutable map.
pub enum MutableMapKeyKind {
    String,
    I32,
    U32,
    I64,
    U64,
}

/// Encoding and dirty-tracking contract implemented by mutable field values.
///
/// This trait is primarily implemented by generated bindings. Application code
/// normally interacts with the generated field accessors instead.
pub trait MutableField<'a>: Clone + Default {
    fn decode(field: FieldView<'a>) -> Option<Self>;
    fn detect(field: FieldView<'a>) -> Option<&'a [u32]>;
    fn folds_when_present() -> bool
    where
        Self: Sized,
    {
        true
    }
    fn is_empty_field(&self) -> bool {
        false
    }
    fn omits_default_after_encode() -> bool
    where
        Self: Sized,
    {
        true
    }
    fn is_dirty(&self) -> bool {
        false
    }
    fn has_nested_dirty(&self) -> bool {
        self.is_dirty()
    }
    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError>;
}

/// Array-specific extension of [`MutableField`] used by generated bindings.
pub trait MutableArrayElement<'a>: MutableField<'a> {
    fn decode_array(words: &'a [u32]) -> Option<Vec<Self>>;
    fn detect_array_words(words: &'a [u32]) -> Option<&'a [u32]> {
        detect_array_words::<Self>(words)
    }
    fn detect_array_field(field: FieldView<'a>) -> Option<&'a [u32]> {
        Self::detect_array_words(field.object_words()?)
    }
    fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError>;
}

/// Map-key encoding contract used by [`MutableMap`].
pub trait MutableMapKey<'a>: Clone + Eq + Hash {
    fn decode_key(field: FieldView<'a>) -> Option<Self>;
    fn detect_key(field: FieldView<'a>) -> Option<&'a [u32]>;
    fn encode_key(&self, buffer: &mut Buffer) -> Result<Unit, MutableError>;
    fn key_kind() -> MutableMapKeyKind;
    fn borrowed_key_bytes(&self) -> Option<&[u8]> {
        None
    }
    fn write_key_bytes<'b>(&'b self, scratch: &'b mut [u8; 8]) -> &'b [u8];
}

#[derive(Clone, Copy)]
enum KeyBytes<'a> {
    Borrowed(&'a [u8]),
    Inline { data: [u8; 8], len: usize },
}

impl AsRef<[u8]> for KeyBytes<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Inline { data, len } => &data[..*len],
        }
    }
}

#[derive(Clone, Debug)]
/// An owned mutable array with dirty tracking for generated messages.
pub struct MutableArray<'a, T> {
    values: Vec<T>,
    dirty: bool,
    _marker: PhantomData<&'a ()>,
}

impl<'a, T> Default for MutableArray<'a, T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            dirty: false,
            _marker: PhantomData,
        }
    }
}

impl<'a, T> MutableArray<'a, T> {
    /// Creates an empty, clean mutable array.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Removes all values and marks the array dirty.
    pub fn clear(&mut self) {
        self.dirty = true;
        self.values.clear();
    }

    pub fn reserve(&mut self, additional: usize) {
        self.values.reserve(additional);
    }

    /// Appends a value and marks the array dirty.
    pub fn push(&mut self, value: T) {
        self.dirty = true;
        self.values.push(value);
    }

    pub fn as_slice(&self) -> &[T] {
        &self.values
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self.dirty = true;
        &mut self.values
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.values.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.dirty = true;
        self.values.iter_mut()
    }
}

impl<'a, T> From<Vec<T>> for MutableArray<'a, T> {
    fn from(values: Vec<T>) -> Self {
        Self {
            values,
            dirty: false,
            _marker: PhantomData,
        }
    }
}

impl<'a, T> Extend<T> for MutableArray<'a, T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.dirty = true;
        self.values.extend(iter);
    }
}

impl<'a, T> std::iter::FromIterator<T> for MutableArray<'a, T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self {
            values: iter.into_iter().collect(),
            dirty: false,
            _marker: PhantomData,
        }
    }
}

impl<'b, 'a, T> IntoIterator for &'b MutableArray<'a, T> {
    type Item = &'b T;
    type IntoIter = std::slice::Iter<'b, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<'b, 'a, T> IntoIterator for &'b mut MutableArray<'a, T> {
    type Item = &'b mut T;
    type IntoIter = std::slice::IterMut<'b, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.dirty = true;
        self.values.iter_mut()
    }
}

impl<'a, T> std::ops::Index<usize> for MutableArray<'a, T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.values[index]
    }
}

impl<'a, T> std::ops::IndexMut<usize> for MutableArray<'a, T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.dirty = true;
        &mut self.values[index]
    }
}

impl<'a, T: MutableArrayElement<'a>> MutableArray<'a, T> {
    /// Decodes all elements from an encoded array.
    pub fn from_words(words: &'a [u32]) -> Option<Self> {
        Some(Self {
            values: T::decode_array(words)?,
            dirty: false,
            _marker: PhantomData,
        })
    }

    /// Encodes the array into `buffer` and returns its field representation.
    pub fn encode_to_unit(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        T::encode_array(&self.values, buffer)
    }
}

#[derive(Clone, Debug)]
/// An owned mutable map with dirty tracking and ProtoCache key encoding.
pub struct MutableMap<'a, K, V> {
    entries: HashMap<K, V>,
    dirty: bool,
    _marker: PhantomData<&'a ()>,
}

impl<'a, K, V> Default for MutableMap<'a, K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            dirty: false,
            _marker: PhantomData,
        }
    }
}

impl<'a, K: Eq + Hash, V> MutableMap<'a, K, V> {
    /// Creates an empty, clean mutable map.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes all entries and marks the map dirty.
    pub fn clear(&mut self) {
        self.dirty = true;
        self.entries.clear();
    }

    pub fn reserve(&mut self, additional: usize) {
        self.entries.reserve(additional);
    }

    /// Inserts an entry and marks the map dirty.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.dirty = true;
        self.entries.insert(key, value)
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.contains_key(key)
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.get(key)
    }

    /// Returns mutable access to a value and marks the map dirty when found.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.dirty = true;
        self.entries.get_mut(key)
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.dirty = true;
        self.entries.remove(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter()
    }
}

impl<'a, K, V> From<HashMap<K, V>> for MutableMap<'a, K, V> {
    fn from(entries: HashMap<K, V>) -> Self {
        Self {
            entries,
            dirty: false,
            _marker: PhantomData,
        }
    }
}

impl<'a, K: Eq + Hash, V> Extend<(K, V)> for MutableMap<'a, K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        self.dirty = true;
        self.entries.extend(iter);
    }
}

impl<'a, K: Eq + Hash, V> std::iter::FromIterator<(K, V)> for MutableMap<'a, K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self {
            entries: iter.into_iter().collect(),
            dirty: false,
            _marker: PhantomData,
        }
    }
}

impl<'b, 'a, K, V> IntoIterator for &'b MutableMap<'a, K, V> {
    type Item = (&'b K, &'b V);
    type IntoIter = std::collections::hash_map::Iter<'b, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl<'b, 'a, K, V> IntoIterator for &'b mut MutableMap<'a, K, V> {
    type Item = (&'b K, &'b mut V);
    type IntoIter = std::collections::hash_map::IterMut<'b, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.dirty = true;
        self.entries.iter_mut()
    }
}

impl<'a, K: MutableMapKey<'a>, V: MutableField<'a>> MutableMap<'a, K, V> {
    /// Decodes all entries from an encoded map.
    pub fn from_words(words: &'a [u32]) -> Option<Self> {
        let map = MapView::new(words)?;
        let mut entries = HashMap::with_capacity(map.len());
        for pair in map.iter() {
            entries.insert(K::decode_key(pair.key())?, V::decode(pair.value())?);
        }
        Some(Self {
            entries,
            dirty: false,
            _marker: PhantomData,
        })
    }

    /// Encodes the map and its perfect-hash index into `buffer`.
    pub fn encode_to_unit(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        if self.entries.is_empty() {
            return Ok(Unit::inline(&[5u32 << 28]));
        }

        let last = buffer.len();
        let mut memo = Vec::with_capacity(self.entries.len());
        let mut key_bytes = Vec::with_capacity(self.entries.len());
        match K::key_kind() {
            MutableMapKeyKind::String => {
                for pair in self.entries.iter() {
                    key_bytes.push(KeyBytes::Borrowed(
                        pair.0
                            .borrowed_key_bytes()
                            .expect("string keys should expose borrowed bytes"),
                    ));
                    memo.push(pair);
                }
            }
            _ => {
                for pair in self.entries.iter() {
                    let mut scratch = [0u8; 8];
                    let len = {
                        let bytes = pair.0.write_key_bytes(&mut scratch);
                        bytes.len()
                    };
                    key_bytes.push(KeyBytes::Inline { data: scratch, len });
                    memo.push(pair);
                }
            }
        }
        let (index, positions) = build_perfect_hash_index_with_positions(&key_bytes).ok_or(
            MutableError::SerializeFailed {
                descriptor: "<map>".to_owned(),
                field: "<index>".to_owned(),
            },
        )?;

        let mut book = vec![memo[0]; memo.len()];
        for (idx, position) in positions.into_iter().enumerate() {
            book[position] = memo[idx];
        }

        let mut pairs = vec![(Unit::empty(), Unit::empty()); book.len()];
        for i in (0..book.len()).rev() {
            let (key, value) = book[i];
            pairs[i].1 = value.encode(buffer)?;
            pairs[i].0 = key.encode_key(buffer)?;
        }

        serialize_map_pairs_at_mut(&index, &mut pairs, buffer, last).ok_or_else(|| {
            MutableError::SerializeFailed {
                descriptor: "<map>".to_owned(),
                field: "<encode>".to_owned(),
            }
        })
    }
}

#[inline(always)]
fn detect_array_words<'a, T: MutableField<'a>>(words: &'a [u32]) -> Option<&'a [u32]> {
    let array = ArrayView::new(words)?;
    let end = array.total_words();
    for index in (0..array.len()).rev() {
        let detected = T::detect(array.expect_field(index))?;
        let detected_end = checked_slice_end(words, detected)?;
        if detected_end > end {
            return words.get(..detected_end);
        }
    }
    words.get(..end)
}

#[inline(always)]
fn detect_map_words<'a, K: MutableMapKey<'a>, V: MutableField<'a>>(
    words: &'a [u32],
) -> Option<&'a [u32]> {
    let map = MapView::new(words)?;
    let end = map.total_words();
    for index in (0..map.len()).rev() {
        let pair = map.expect_pair(index);
        let detected = V::detect(pair.value())?;
        let detected_end = checked_slice_end(words, detected)?;
        if detected_end > end {
            return words.get(..detected_end);
        }

        let detected = K::detect_key(pair.key())?;
        let detected_end = checked_slice_end(words, detected)?;
        if detected_end > end {
            return words.get(..detected_end);
        }
    }
    words.get(..end)
}

#[derive(Clone, Debug)]
/// Lazy mutable state for a generated message.
///
/// `N` is the number of schema fields and `WORDS` is the generated bitset
/// storage. Untouched fields continue to borrow their original encoded words.
pub struct MutableMessage<'a, const N: usize, const WORDS: usize> {
    words: Option<&'a [u32]>,
    accessed: [u64; WORDS],
}

impl<'a, const N: usize, const WORDS: usize> Default for MutableMessage<'a, N, WORDS> {
    fn default() -> Self {
        Self {
            words: None,
            accessed: [0; WORDS],
        }
    }
}

impl<'a, const N: usize, const WORDS: usize> MutableMessage<'a, N, WORDS> {
    /// Creates empty mutable state with no borrowed source message.
    pub fn new() -> Self {
        debug_assert_eq!(WORDS, N.div_ceil(64));
        Self::default()
    }

    /// Creates lazy mutable state backed by a validated encoded message.
    pub fn from_words(words: &'a [u32]) -> Option<Self> {
        MessageView::layout(words)?;
        debug_assert_eq!(WORDS, N.div_ceil(64));
        Some(Self {
            words: Some(words),
            accessed: [0; WORDS],
        })
    }

    #[inline(always)]
    fn field(&self, id: usize) -> Option<FieldView<'a>> {
        let words = self.words?;
        let layout = MessageView::layout(words)?;
        MessageView::field_in(words, &layout, id)
    }

    #[inline(always)]
    /// Returns whether a field exists in the source message or has been set.
    pub fn has_field(&self, id: usize) -> bool {
        self.field(id).is_some()
    }

    #[inline(always)]
    /// Returns whether a generated accessor has materialized this field.
    pub fn was_accessed(&self, id: usize) -> bool {
        let word = id / 64;
        let bit = id % 64;
        (self.accessed[word] & (1u64 << bit)) != 0
    }

    pub fn has_any_accessed(&self) -> bool {
        self.accessed.iter().any(|&word| word != 0)
    }

    /// Returns the untouched source words, or `None` after any field access.
    pub fn clean_words(&self) -> Option<&[u32]> {
        if self.has_any_accessed() {
            None
        } else {
            self.words
        }
    }

    /// Lazily decodes a field into its generated storage slot.
    pub fn get_field<'b, T: MutableField<'a>>(&mut self, id: usize, slot: &'b mut T) -> &'b mut T {
        if !self.was_accessed(id) {
            let word = id / 64;
            let bit = id % 64;
            self.accessed[word] |= 1u64 << bit;
            *slot = self.field(id).and_then(T::decode).unwrap_or_default();
        }
        slot
    }

    #[inline(always)]
    /// Serializes one generated field, reusing untouched source words when safe.
    pub fn serialize_field<T: MutableField<'a>>(
        &self,
        id: usize,
        field: &T,
        buffer: &mut Buffer,
        unit: &mut Unit,
    ) -> Result<(), MutableError> {
        if !self.was_accessed(id) {
            *unit = self
                .field(id)
                .and_then(T::detect)
                .map(|words| copy_words(words, buffer, true))
                .unwrap_or_else(Unit::empty);
            return Ok(());
        }

        if field.is_empty_field() {
            *unit = Unit::empty();
            return Ok(());
        }

        *unit = field.encode(buffer)?;
        if T::omits_default_after_encode() && should_omit_message_unit(unit) {
            drop_present_unit(buffer, unit);
        } else if T::folds_when_present() {
            fold_field(buffer, unit);
        } else {
            debug_assert!(!T::omits_default_after_encode());
            debug_assert!(!unit.is_segment());
        }
        Ok(())
    }
}

fn should_omit_message_unit(unit: &Unit) -> bool {
    unit.size() == 1
}

fn drop_present_unit(buffer: &mut Buffer, unit: &mut Unit) {
    if unit.is_segment() {
        let seg = unit.segment_info();
        debug_assert_eq!(seg.pos, buffer.len());
        buffer.shrink(seg.len);
    }
    *unit = Unit::empty();
}

#[inline(always)]
/// Copies an already encoded field into `buffer`, optionally folding small data
/// into the returned [`Unit`].
pub fn copy_words(words: &[u32], buffer: &mut Buffer, fold: bool) -> Unit {
    if fold && words.len() < 4 {
        return Unit::inline(words);
    }
    let last = buffer.len();
    buffer.put_words(words);
    Unit::segment(last, buffer.len())
}

macro_rules! impl_scalar_field {
    ($ty:ty) => {
        impl<'a> MutableField<'a> for $ty {
            fn decode(field: FieldView<'a>) -> Option<Self> {
                field.scalar::<$ty>()
            }

            fn detect(field: FieldView<'a>) -> Option<&'a [u32]> {
                field.detect_scalar()
            }

            fn folds_when_present() -> bool
            where
                Self: Sized,
            {
                false
            }

            fn is_empty_field(&self) -> bool {
                *self == 0 as $ty
            }

            fn omits_default_after_encode() -> bool
            where
                Self: Sized,
            {
                false
            }

            fn encode(&self, _buffer: &mut Buffer) -> Result<Unit, MutableError> {
                Ok(serialize_scalar::<$ty>(*self))
            }
        }

        impl<'a> MutableArrayElement<'a> for $ty {
            fn decode_array(words: &'a [u32]) -> Option<Vec<Self>> {
                Some(
                    ArrayView::new(words)?
                        .scalars::<$ty>()?
                        .iter()
                        .collect::<Vec<_>>(),
                )
            }

            fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError> {
                let width = <$ty as Scalar>::WIDTH as u32;
                if values.is_empty() {
                    return Ok(Unit::inline(&[width]));
                }
                let last = buffer.len();
                for value in values.iter().rev() {
                    value
                        .write_words(buffer.expand(<$ty as Scalar>::WIDTH))
                        .expect("scalar width must match");
                }
                buffer.put(((values.len() as u32) << 2) | width);
                Ok(Unit::segment(last, buffer.len()))
            }
        }
    };
}

macro_rules! impl_scalar_map_key {
    ($ty:ty, $map_kind:ident) => {
        impl<'a> MutableMapKey<'a> for $ty {
            fn decode_key(field: FieldView<'a>) -> Option<Self> {
                field.scalar::<$ty>()
            }

            fn detect_key(field: FieldView<'a>) -> Option<&'a [u32]> {
                field.detect_scalar()
            }

            fn encode_key(&self, _buffer: &mut Buffer) -> Result<Unit, MutableError> {
                Ok(serialize_scalar::<$ty>(*self))
            }

            fn key_kind() -> MutableMapKeyKind {
                MutableMapKeyKind::$map_kind
            }

            fn write_key_bytes<'b>(&'b self, scratch: &'b mut [u8; 8]) -> &'b [u8] {
                let bytes = MapKey::as_key_bytes(self);
                let len = bytes.len();
                scratch[..len].copy_from_slice(&bytes);
                &scratch[..len]
            }
        }
    };
}

impl<'a> MutableField<'a> for bool {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        field.scalar::<bool>()
    }

    fn detect(field: FieldView<'a>) -> Option<&'a [u32]> {
        field.detect_scalar()
    }

    fn folds_when_present() -> bool
    where
        Self: Sized,
    {
        false
    }

    fn is_empty_field(&self) -> bool {
        !*self
    }

    fn omits_default_after_encode() -> bool
    where
        Self: Sized,
    {
        false
    }

    fn encode(&self, _buffer: &mut Buffer) -> Result<Unit, MutableError> {
        Ok(serialize_bool(*self))
    }
}

impl<'a> MutableArrayElement<'a> for bool {
    fn decode_array(words: &'a [u32]) -> Option<Vec<Self>> {
        Some(StringView::new(words)?.as_bool_array().iter().collect())
    }

    fn detect_array_field(field: FieldView<'a>) -> Option<&'a [u32]> {
        field.detect_string()
    }

    fn detect_array_words(words: &'a [u32]) -> Option<&'a [u32]> {
        StringView::detect(words)
    }

    fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError> {
        let bytes = values
            .iter()
            .map(|value| u8::from(*value))
            .collect::<Vec<_>>();
        serialize_bytes(&bytes, buffer).ok_or_else(|| MutableError::SerializeFailed {
            descriptor: "<array>".to_owned(),
            field: "<bool-array>".to_owned(),
        })
    }
}

impl_scalar_field!(i32);
impl_scalar_field!(u32);
impl_scalar_field!(i64);
impl_scalar_field!(u64);
impl_scalar_field!(f32);
impl_scalar_field!(f64);
impl_scalar_map_key!(i32, I32);
impl_scalar_map_key!(u32, U32);
impl_scalar_map_key!(i64, I64);
impl_scalar_map_key!(u64, U64);

impl<'a> MutableField<'a> for String {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        Some(field.string()?.as_str()?.to_owned())
    }

    fn detect(field: FieldView<'a>) -> Option<&'a [u32]> {
        field.detect_string()
    }

    fn is_empty_field(&self) -> bool {
        self.is_empty()
    }

    fn omits_default_after_encode() -> bool
    where
        Self: Sized,
    {
        false
    }

    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        serialize_str(self, buffer).ok_or_else(|| MutableError::SerializeFailed {
            descriptor: "<string>".to_owned(),
            field: "<encode>".to_owned(),
        })
    }
}

impl<'a> MutableArrayElement<'a> for String {
    fn decode_array(words: &'a [u32]) -> Option<Vec<Self>> {
        let array = ArrayView::new(words)?;
        let mut values = Vec::with_capacity(array.len());
        for item in array.iter() {
            values.push(Self::decode(item)?);
        }
        Some(values)
    }

    fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError> {
        encode_object_array(values, buffer)
    }
}

impl<'a> MutableMapKey<'a> for String {
    fn decode_key(field: FieldView<'a>) -> Option<Self> {
        Some(field.string()?.as_str()?.to_owned())
    }

    fn detect_key(field: FieldView<'a>) -> Option<&'a [u32]> {
        field.detect_string()
    }

    fn encode_key(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        serialize_str(self, buffer).ok_or_else(|| MutableError::SerializeFailed {
            descriptor: "<map>".to_owned(),
            field: "<string-key>".to_owned(),
        })
    }

    fn key_kind() -> MutableMapKeyKind {
        MutableMapKeyKind::String
    }

    fn borrowed_key_bytes(&self) -> Option<&[u8]> {
        Some(self.as_bytes())
    }

    fn write_key_bytes<'b>(&'b self, _scratch: &'b mut [u8; 8]) -> &'b [u8] {
        self.as_bytes()
    }
}

impl<'a> MutableField<'a> for Vec<u8> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        Some(field.string()?.as_bytes().to_vec())
    }

    fn detect(field: FieldView<'a>) -> Option<&'a [u32]> {
        field.detect_string()
    }

    fn is_empty_field(&self) -> bool {
        self.is_empty()
    }

    fn omits_default_after_encode() -> bool
    where
        Self: Sized,
    {
        false
    }

    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        serialize_bytes(self, buffer).ok_or_else(|| MutableError::SerializeFailed {
            descriptor: "<bytes>".to_owned(),
            field: "<encode>".to_owned(),
        })
    }
}

impl<'a> MutableArrayElement<'a> for Vec<u8> {
    fn decode_array(words: &'a [u32]) -> Option<Vec<Self>> {
        let array = ArrayView::new(words)?;
        let mut values = Vec::with_capacity(array.len());
        for item in array.iter() {
            values.push(Self::decode(item)?);
        }
        Some(values)
    }

    fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError> {
        encode_object_array(values, buffer)
    }
}

impl<'a, T: MutableArrayElement<'a>> MutableField<'a> for MutableArray<'a, T> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        Self::from_words(field.object_words()?)
    }

    #[inline(always)]
    fn detect(field: FieldView<'a>) -> Option<&'a [u32]> {
        Self::detect_array_field(field)
    }

    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        self.encode_to_unit(buffer)
    }

    fn is_empty_field(&self) -> bool {
        self.values.is_empty()
    }

    fn omits_default_after_encode() -> bool
    where
        Self: Sized,
    {
        false
    }

    fn is_dirty(&self) -> bool {
        self.dirty || self.values.iter().any(MutableField::has_nested_dirty)
    }

    fn has_nested_dirty(&self) -> bool {
        self.is_dirty()
    }
}

impl<'a, K: MutableMapKey<'a>, V: MutableField<'a>> MutableField<'a> for MutableMap<'a, K, V> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        Self::from_words(field.object_words()?)
    }

    #[inline(always)]
    fn detect(field: FieldView<'a>) -> Option<&'a [u32]> {
        detect_map_words::<K, V>(field.object_words()?)
    }

    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        self.encode_to_unit(buffer)
    }

    fn is_empty_field(&self) -> bool {
        self.entries.is_empty()
    }

    fn omits_default_after_encode() -> bool
    where
        Self: Sized,
    {
        false
    }

    fn is_dirty(&self) -> bool {
        self.dirty
            || self
                .entries
                .iter()
                .any(|(_, value)| value.has_nested_dirty())
    }

    fn has_nested_dirty(&self) -> bool {
        self.is_dirty()
    }
}

impl<'a, T: MutableArrayElement<'a>> MutableArrayElement<'a> for MutableArray<'a, T> {
    fn decode_array(words: &'a [u32]) -> Option<Vec<Self>> {
        let array = ArrayView::new(words)?;
        let mut values = Vec::with_capacity(array.len());
        for item in array.iter() {
            values.push(Self::from_words(item.object_words()?)?);
        }
        Some(values)
    }

    #[inline(always)]
    fn detect_array_field(field: FieldView<'a>) -> Option<&'a [u32]> {
        field
            .object_words()
            .and_then(detect_array_words::<T>)
            .or_else(|| T::detect_array_field(field))
    }

    fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError> {
        encode_object_array(values, buffer)
    }
}

impl<'a, K: MutableMapKey<'a>, V: MutableField<'a>> MutableArrayElement<'a>
    for MutableMap<'a, K, V>
{
    fn decode_array(words: &'a [u32]) -> Option<Vec<Self>> {
        let array = ArrayView::new(words)?;
        let mut values = Vec::with_capacity(array.len());
        for item in array.iter() {
            values.push(Self::from_words(item.object_words()?)?);
        }
        Some(values)
    }

    fn encode_array(values: &[Self], buffer: &mut Buffer) -> Result<Unit, MutableError> {
        encode_object_array(values, buffer)
    }
}

impl<'a, T: MutableField<'a>> MutableField<'a> for Box<T> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        Some(Box::new(T::decode(field)?))
    }

    #[inline(always)]
    fn detect(field: FieldView<'a>) -> Option<&'a [u32]> {
        T::detect(field)
    }

    #[inline(always)]
    fn encode(&self, buffer: &mut Buffer) -> Result<Unit, MutableError> {
        self.as_ref().encode(buffer)
    }

    #[inline(always)]
    fn is_dirty(&self) -> bool {
        self.as_ref().is_dirty()
    }

    #[inline(always)]
    fn has_nested_dirty(&self) -> bool {
        self.as_ref().has_nested_dirty()
    }
}

fn encode_object_array<'a, T: MutableField<'a>>(
    values: &[T],
    buffer: &mut Buffer,
) -> Result<Unit, MutableError> {
    const STACK_UNITS: usize = 32;

    if values.is_empty() {
        return Ok(Unit::inline(&[1]));
    }
    let last = buffer.len();
    if values.len() <= STACK_UNITS {
        let mut units = array::from_fn::<_, STACK_UNITS, _>(|_| Unit::empty());
        for i in (0..values.len()).rev() {
            units[i] = values[i].encode(buffer)?;
        }
        return serialize_array_at_mut(&mut units[..values.len()], buffer, last).ok_or_else(|| {
            MutableError::SerializeFailed {
                descriptor: "<array>".to_owned(),
                field: "<encode>".to_owned(),
            }
        });
    }

    let mut units = vec![Unit::empty(); values.len()];
    for i in (0..values.len()).rev() {
        units[i] = values[i].encode(buffer)?;
    }
    serialize_array_at_mut(&mut units, buffer, last).ok_or_else(|| MutableError::SerializeFailed {
        descriptor: "<array>".to_owned(),
        field: "<encode>".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{MutableArray, MutableMap, drop_present_unit, should_omit_message_unit};
    use crate::mutable::MutableField;
    use crate::{ArrayView, Buffer, MapView, Unit};

    #[test]
    fn array_ex_iter_mut_marks_collection_dirty() {
        let mut array = MutableArray::from(vec![1u32, 2u32]);
        assert!(!array.is_dirty());

        for value in &mut array {
            *value += 10;
        }

        assert!(array.is_dirty());
        assert_eq!(array.as_slice(), &[11, 12]);
    }

    #[test]
    fn map_ex_get_mut_marks_collection_dirty() {
        let mut map = MutableMap::from_iter([("alpha".to_owned(), 1u32)]);
        assert!(!map.is_dirty());

        *map.get_mut("alpha").unwrap() = 7;

        assert!(map.is_dirty());
        assert_eq!(map.get("alpha"), Some(&7));
    }

    #[test]
    fn map_ex_mut_into_iter_marks_collection_dirty() {
        let mut map =
            MutableMap::from_iter([("alpha".to_owned(), 1u32), ("beta".to_owned(), 2u32)]);
        assert!(!map.is_dirty());

        for (_, value) in &mut map {
            *value *= 2;
        }

        assert!(map.is_dirty());
        assert_eq!(map.get("alpha"), Some(&2));
        assert_eq!(map.get("beta"), Some(&4));
    }

    #[test]
    fn map_ex_supports_borrowed_string_lookup() {
        let mut map =
            MutableMap::from_iter([("alpha".to_owned(), 1u32), ("beta".to_owned(), 2u32)]);

        assert!(map.contains_key("alpha"));
        assert_eq!(map.get("beta"), Some(&2));
        assert_eq!(map.remove("alpha"), Some(1));
        assert!(!map.contains_key("alpha"));
    }

    #[test]
    fn default_present_units_are_omitted_by_shape() {
        assert!(should_omit_message_unit(&Unit::inline(&[0])));
        assert!(should_omit_message_unit(&Unit::inline(&[1])));
        assert!(!should_omit_message_unit(&Unit::inline(&[0, 0])));
        assert!(!should_omit_message_unit(&Unit::inline(&[2, 0])));
    }

    #[test]
    fn dropping_segment_present_unit_rewinds_buffer() {
        let mut buffer = Buffer::new();
        buffer.put(0);
        let mut unit = Unit::segment(0, buffer.len());

        assert!(should_omit_message_unit(&unit));
        drop_present_unit(&mut buffer, &mut unit);

        assert!(buffer.is_empty());
        assert!(unit.is_empty());
    }

    #[test]
    fn mutable_nested_float_arrays_roundtrip() {
        let mut rows = MutableArray::new();
        rows.push(MutableArray::new());
        rows.push(MutableArray::from(vec![7.0f32, 8.0, 9.0]));

        let mut buffer = Buffer::new();
        let unit = rows.encode(&mut buffer).unwrap();
        assert!(unit.is_segment());

        let view = ArrayView::new(buffer.view()).unwrap();
        assert_eq!(view.len(), 2);

        let row0 = ArrayView::new(view.field(0).unwrap().object_words().unwrap()).unwrap();
        assert!(row0.scalars::<f32>().unwrap().is_empty());

        let row1_words = view.field(1).unwrap().object_words().unwrap();
        let row1 = ArrayView::new(row1_words).unwrap();
        assert_eq!(row1.width(), 1, "row1 words: {:?}", row1_words);
        assert_eq!(
            row1.scalars::<f32>().unwrap().iter().collect::<Vec<_>>(),
            vec![7.0, 8.0, 9.0]
        );
    }

    #[test]
    fn empty_mutable_map_encodes_as_inline_empty_map() {
        let map = MutableMap::<String, i32>::new();
        let mut buffer = Buffer::new();

        let unit = map.encode_to_unit(&mut buffer).unwrap();

        assert_eq!(unit.inline_words(), &[5u32 << 28]);
        assert_eq!(buffer.len(), 0);
        assert_eq!(MapView::new(unit.inline_words()).unwrap().len(), 0);
    }

    #[test]
    fn mutable_array_with_empty_map_element_roundtrips() {
        let map = MutableMap::<String, i32>::new();
        let maps = MutableArray::from(vec![map]);
        let mut buffer = Buffer::new();

        maps.encode_to_unit(&mut buffer).unwrap();

        let array = ArrayView::new(buffer.view()).unwrap();
        assert_eq!(array.len(), 1);
        let map_words = array.field(0).unwrap().object_words().unwrap();
        assert_eq!(MapView::new(map_words).unwrap().len(), 0);
    }

    #[test]
    fn mutable_string_key_float_array_map_roundtrip() {
        let mut map = MutableMap::new();
        map.insert(
            "lv5".to_owned(),
            MutableArray::from(vec![51.0f32, 52.0, 53.0]),
        );
        map.insert("lv9".to_owned(), MutableArray::from(vec![91.0f32, 92.0]));

        let mut buffer = Buffer::new();
        let unit = map.encode(&mut buffer).unwrap();
        assert!(unit.is_segment());

        let view = MapView::new(buffer.view()).unwrap();
        let available = view
            .iter()
            .filter_map(|pair| {
                pair.key()
                    .string()
                    .and_then(|key| key.as_str().map(str::to_owned))
            })
            .collect::<Vec<_>>();
        let lv5 = view
            .find_str("lv5")
            .unwrap_or_else(|| panic!("available keys: {available:?}, words: {:?}", buffer.view()))
            .value()
            .array()
            .unwrap();
        assert_eq!(
            lv5.scalars::<f32>().unwrap().iter().collect::<Vec<_>>(),
            vec![51.0, 52.0, 53.0]
        );
        let lv9 = view.find_str("lv9").unwrap().value().array().unwrap();
        assert_eq!(
            lv9.scalars::<f32>().unwrap().iter().collect::<Vec<_>>(),
            vec![91.0, 92.0]
        );
    }
}
