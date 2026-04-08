//! Access surface matching `access.h`.

use core::marker::PhantomData;

use crate::utils::{Scalar, word_size};
use crate::perfect_hash::PerfectHashView;

pub trait MapKey {
    fn as_key_bytes(&self) -> Vec<u8>;
}

pub trait FieldDecode<'a>: Sized {
    fn decode(field: FieldView<'a>) -> Option<Self>;
}

impl<'a, T: Scalar> FieldDecode<'a> for T {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        field.scalar::<T>()
    }
}

macro_rules! impl_map_key {
    ($($ty:ty),* $(,)?) => {
        $(
            impl MapKey for $ty {
                fn as_key_bytes(&self) -> Vec<u8> {
                    self.to_le_bytes().to_vec()
                }
            }
        )*
    };
}

impl_map_key!(i32, u32, i64, u64);

#[derive(Clone, Copy, Debug)]
pub struct StringView<'a> {
    bytes: &'a [u8],
}

impl<'a> StringView<'a> {
    #[inline(always)]
    pub fn new(words: &'a [u32]) -> Option<Self> {
        let raw = bytes_of_words(words);
        let first = *raw.first()?;
        if (first & 3) != 0 {
            return None;
        }

        let mut mark = 0usize;
        let mut shift = 0usize;
        let mut used = 0usize;
        while shift < 32 {
            let byte = *raw.get(used)?;
            used += 1;
            if (byte & 0x80) != 0 {
                mark |= ((byte & 0x7f) as usize) << shift;
            } else {
                mark |= (byte as usize) << shift;
                let byte_len = mark >> 2;
                let end = used.checked_add(byte_len)?;
                return Some(Self {
                    bytes: raw.get(used..end)?,
                });
            }
            shift += 7;
        }
        None
    }

    #[inline(always)]
    pub fn detect_len(words: &'a [u32]) -> Option<usize> {
        let raw = bytes_of_words(words);
        let first = *raw.first()?;
        if (first & 3) != 0 {
            return None;
        }

        let mut mark = 0usize;
        let mut shift = 0usize;
        let mut used = 0usize;
        while shift < 32 {
            let byte = *raw.get(used)?;
            used += 1;
            if (byte & 0x80) != 0 {
                mark |= ((byte & 0x7f) as usize) << shift;
            } else {
                mark |= (byte as usize) << shift;
                let total = used.checked_add(mark >> 2)?;
                return Some(word_size(total));
            }
            shift += 7;
        }
        None
    }

    #[inline(always)]
    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        words.get(..Self::detect_len(words)?)
    }

    #[inline(always)]
    pub fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }

    #[inline(always)]
    pub fn as_str(self) -> Option<&'a str> {
        core::str::from_utf8(self.bytes).ok()
    }

    #[inline(always)]
    pub fn as_bool_array(self) -> BoolArray<'a> {
        BoolArray { bytes: self.bytes }
    }
}

impl AsRef<[u8]> for StringView<'_> {
    fn as_ref(&self) -> &[u8] {
        self.bytes
    }
}

impl<'a> core::fmt::Display for StringView<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.as_str() {
            Some(value) => f.write_str(value),
            None => write!(f, "{:?}", self.bytes),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BoolArray<'a> {
    bytes: &'a [u8],
}

impl<'a> BoolArray<'a> {
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<bool> {
        Some(*self.bytes.get(index)? != 0)
    }

    #[inline(always)]
    pub fn iter(&self) -> impl Iterator<Item = bool> + 'a {
        self.bytes.iter().copied().map(|v| v != 0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FieldView<'a> {
    tail: &'a [u32],
    width: usize,
}

impl<'a> FieldView<'a> {
    #[inline(always)]
    pub fn raw_words(self) -> Option<&'a [u32]> {
        self.tail.get(..self.width)
    }

    #[inline(always)]
    pub fn expect_raw_words(self) -> &'a [u32] {
        &self.tail[..self.width]
    }

    #[inline(always)]
    pub fn object_words(self) -> Option<&'a [u32]> {
        let first = *self.tail.first()?;
        if (first & 3) == 3 {
            self.tail.get((first >> 2) as usize..)
        } else {
            Some(self.tail)
        }
    }

    #[inline(always)]
    pub fn expect_object_words(self) -> &'a [u32] {
        let first = self.tail[0];
        if (first & 3) == 3 {
            &self.tail[(first >> 2) as usize..]
        } else {
            self.tail
        }
    }

    #[inline(always)]
    pub fn scalar<T: Scalar>(self) -> Option<T> {
        T::from_words(self.raw_words()?)
    }

    #[inline(always)]
    pub fn expect_scalar<T: Scalar>(self) -> T {
        T::from_words(self.expect_raw_words()).expect("invalid scalar field")
    }

    #[inline(always)]
    pub fn detect_scalar(self) -> Option<&'a [u32]> {
        self.raw_words()
    }

    #[inline(always)]
    pub fn string(self) -> Option<StringView<'a>> {
        StringView::new(self.object_words()?)
    }

    #[inline(always)]
    pub fn expect_string(self) -> StringView<'a> {
        StringView::new(self.expect_object_words()).expect("invalid string field")
    }

    #[inline(always)]
    pub fn detect_string(self) -> Option<&'a [u32]> {
        StringView::detect(self.object_words()?)
    }

    #[inline(always)]
    pub fn message(self) -> Option<MessageView<'a>> {
        MessageView::new(self.object_words()?)
    }

    #[inline(always)]
    pub fn expect_message(self) -> MessageView<'a> {
        MessageView::new(self.expect_object_words()).expect("invalid message field")
    }

    #[inline(always)]
    pub fn detect_message(self) -> Option<&'a [u32]> {
        MessageView::detect(self.object_words()?)
    }

    #[inline(always)]
    pub fn array(self) -> Option<ArrayView<'a>> {
        ArrayView::new(self.object_words()?)
    }

    #[inline(always)]
    pub fn expect_array(self) -> ArrayView<'a> {
        ArrayView::new(self.expect_object_words()).expect("invalid array field")
    }

    #[inline(always)]
    pub fn detect_array(self) -> Option<&'a [u32]> {
        ArrayView::detect(self.object_words()?)
    }

    #[inline(always)]
    pub fn map(self) -> Option<MapView<'a>> {
        MapView::new(self.object_words()?)
    }

    #[inline(always)]
    pub fn expect_map(self) -> MapView<'a> {
        MapView::new(self.expect_object_words()).expect("invalid map field")
    }

    #[inline(always)]
    pub fn detect_map(self) -> Option<&'a [u32]> {
        MapView::detect(self.object_words()?)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MessageView<'a> {
    head: u32,
    words: &'a [u32],
    body: &'a [u32],
    section: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MessageLayout {
    head: u32,
    body_offset: usize,
    section: usize,
}

impl<'a> MessageView<'a> {
    #[inline(always)]
    pub fn new(words: &'a [u32]) -> Option<Self> {
        let layout = Self::layout(words)?;
        let body = words.get(layout.body_offset..)?;
        Some(Self {
            head: layout.head,
            words,
            body,
            section: layout.section,
        })
    }

    #[inline(always)]
    pub fn from_words(words: &'a [u32]) -> Option<Self> {
        Self::new(words)
    }

    #[inline(always)]
    pub fn raw_words(self) -> &'a [u32] {
        self.words
    }

    #[inline(always)]
    pub fn detect_len(words: &'a [u32]) -> Option<usize> {
        let head = *words.first()?;
        let section = (head & 0xff) as usize;
        let mut tail = 1usize.checked_add(section.checked_mul(2)?)?;
        if section == 0 {
            tail = tail.checked_add(count32(head))?;
        } else {
            let sec_words = words.get(tail - 2..tail)?;
            let raw = bytes_of_words(sec_words);
            let sec = u64::from_le_bytes(raw.try_into().ok()?);
            tail = tail.checked_add(count64(sec << 14))?;
            tail = tail.checked_add((sec >> 50) as usize)?;
        }
        words.get(..tail)?;
        Some(tail)
    }

    #[inline(always)]
    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        words.get(..Self::detect_len(words)?)
    }

    #[inline(always)]
    pub fn has_field(self, id: usize) -> bool {
        self.field(id).is_some()
    }

    #[inline(always)]
    pub fn field(self, id: usize) -> Option<FieldView<'a>> {
        Self::field_in(
            self.words,
            &MessageLayout {
                head: self.head,
                body_offset: self.words.len() - self.body.len(),
                section: self.section,
            },
            id,
        )
    }

    #[inline(always)]
    pub fn expect_field(self, id: usize) -> FieldView<'a> {
        self.field(id).expect("missing message field")
    }

    #[inline(always)]
    pub fn scalar<T: Scalar>(self, id: usize) -> Option<T> {
        self.field(id)?.scalar::<T>()
    }

    #[inline(always)]
    pub fn string(self, id: usize) -> Option<StringView<'a>> {
        self.field(id)?.string()
    }

    #[inline(always)]
    pub fn bytes(self, id: usize) -> Option<&'a [u8]> {
        Some(self.string(id)?.as_bytes())
    }

    #[inline(always)]
    pub fn bools(self, id: usize) -> Option<BoolArray<'a>> {
        Some(self.string(id)?.as_bool_array())
    }

    #[inline(always)]
    pub fn message(self, id: usize) -> Option<MessageView<'a>> {
        self.field(id)?.message()
    }

    #[inline(always)]
    pub fn array(self, id: usize) -> Option<ArrayView<'a>> {
        self.field(id)?.array()
    }

    #[inline(always)]
    pub fn map(self, id: usize) -> Option<MapView<'a>> {
        self.field(id)?.map()
    }

    #[inline(always)]
    pub(crate) fn layout(words: &[u32]) -> Option<MessageLayout> {
        let head = *words.first()?;
        let section = (head & 0xff) as usize;
        let body_offset = 1usize.checked_add(section.checked_mul(2)?)?;
        words.get(body_offset..)?;
        Some(MessageLayout {
            head,
            body_offset,
            section,
        })
    }

    #[inline(always)]
    pub(crate) fn field_in<'b>(
        words: &'b [u32],
        layout: &MessageLayout,
        id: usize,
    ) -> Option<FieldView<'b>> {
        let (width, off) = if id < 12 {
            let mut v = layout.head >> 8;
            let width = ((v >> (id * 2)) & 3) as usize;
            if width == 0 {
                return None;
            }
            v &= !(u32::MAX << (id * 2));
            (width, count32(v))
        } else {
            let section_index = (id - 12) / 25;
            let bit_index = (id - 12) % 25;
            if section_index >= layout.section {
                return None;
            }
            let sec_words = words.get(1 + section_index * 2..1 + section_index * 2 + 2)?;
            let raw = bytes_of_words(sec_words);
            let vec = u64::from_le_bytes(raw.try_into().ok()?);
            let width = ((vec >> (bit_index * 2)) & 3) as usize;
            if width == 0 {
                return None;
            }
            let mask = if bit_index == 0 {
                0
            } else {
                (1u64 << (bit_index * 2)) - 1
            };
            (width, count64(vec & mask) + (vec >> 50) as usize)
        };

        let body = words.get(layout.body_offset..)?;
        Some(FieldView {
            tail: body.get(off..)?,
            width,
        })
    }

}

#[derive(Clone, Copy, Debug)]
pub struct ArrayView<'a> {
    body: &'a [u32],
    len: usize,
    width: usize,
}

impl<'a> ArrayView<'a> {
    #[inline(always)]
    pub fn new(words: &'a [u32]) -> Option<Self> {
        let head = *words.first()?;
        let len = (head >> 2) as usize;
        let width = (head & 3) as usize;
        if width == 0 {
            return None;
        }
        let body = words.get(1..)?;
        body.get(..len.checked_mul(width)?)?;
        Some(Self { body, len, width })
    }

    #[inline(always)]
    pub fn detect_len(words: &'a [u32]) -> Option<usize> {
        let head = *words.first()?;
        let len = (head >> 2) as usize;
        let width = (head & 3) as usize;
        if width == 0 {
            return None;
        }
        1usize.checked_add(len.checked_mul(width)?)
    }

    #[inline(always)]
    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        words.get(..Self::detect_len(words)?)
    }

    #[inline(always)]
    pub fn len(self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn width(self) -> usize {
        self.width
    }

    #[inline(always)]
    pub(crate) fn total_words(self) -> usize {
        1 + self.len * self.width
    }

    #[inline(always)]
    pub fn field(self, index: usize) -> Option<FieldView<'a>> {
        if index >= self.len {
            return None;
        }
        let start = index.checked_mul(self.width)?;
        Some(FieldView {
            tail: self.body.get(start..)?,
            width: self.width,
        })
    }

    #[inline(always)]
    pub fn expect_field(self, index: usize) -> FieldView<'a> {
        let start = index * self.width;
        FieldView {
            tail: &self.body[start..],
            width: self.width,
        }
    }

    #[inline(always)]
    pub fn scalars<T: Scalar>(self) -> Option<ScalarArray<'a, T>> {
        if self.width != T::WIDTH {
            return None;
        }
        Some(ScalarArray {
            words: self.body.get(..self.len * self.width)?,
            len: self.len,
            _marker: PhantomData,
        })
    }

    #[inline(always)]
    pub fn expect_scalars<T: Scalar>(self) -> ScalarArray<'a, T> {
        assert_eq!(self.width, T::WIDTH);
        ScalarArray {
            words: &self.body[..self.len * self.width],
            len: self.len,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    pub fn iter(self) -> ArrayIter<'a> {
        ArrayIter {
            array: self,
            index: 0,
        }
    }
}

pub struct ArrayIter<'a> {
    array: ArrayView<'a>,
    index: usize,
}

impl<'a> Iterator for ArrayIter<'a> {
    type Item = FieldView<'a>;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        let item = self.array.field(self.index)?;
        self.index += 1;
        Some(item)
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.array.len.saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for ArrayIter<'_> {}
impl core::iter::FusedIterator for ArrayIter<'_> {}

pub struct ViewArray<'a, T> {
    array: ArrayView<'a>,
    _marker: PhantomData<T>,
}

impl<'a, T> Copy for ViewArray<'a, T> {}

impl<'a, T> Clone for ViewArray<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T: FieldDecode<'a>> ViewArray<'a, T> {
    #[inline(always)]
    pub fn new(array: ArrayView<'a>) -> Self {
        Self {
            array,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.array.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.array.is_empty()
    }

    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<T> {
        T::decode(self.array.field(index)?)
    }

    #[inline(always)]
    pub fn iter(&self) -> ViewArrayIter<'a, T> {
        ViewArrayIter {
            array: *self,
            index: 0,
        }
    }

    #[inline(always)]
    pub fn raw(&self) -> ArrayView<'a> {
        self.array
    }
}

pub struct ViewArrayIter<'a, T> {
    array: ViewArray<'a, T>,
    index: usize,
}

impl<'a, T: FieldDecode<'a>> Iterator for ViewArrayIter<'a, T> {
    type Item = T;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        let value = self.array.get(self.index)?;
        self.index += 1;
        Some(value)
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.array.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl<'a, T: FieldDecode<'a>> ExactSizeIterator for ViewArrayIter<'a, T> {}
impl<'a, T: FieldDecode<'a>> core::iter::FusedIterator for ViewArrayIter<'a, T> {}

pub struct ScalarArray<'a, T> {
    words: &'a [u32],
    len: usize,
    _marker: PhantomData<T>,
}

impl<'a, T> Copy for ScalarArray<'a, T> {}

impl<'a, T> Clone for ScalarArray<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T: Scalar> ScalarArray<'a, T> {
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<T> {
        if index >= self.len {
            return None;
        }
        let start = index.checked_mul(T::WIDTH)?;
        T::from_words(self.words.get(start..start + T::WIDTH)?)
    }

    #[inline(always)]
    pub fn iter(&self) -> ScalarArrayIter<'a, T> {
        ScalarArrayIter {
            array: *self,
            index: 0,
        }
    }
}

pub struct ScalarArrayIter<'a, T> {
    array: ScalarArray<'a, T>,
    index: usize,
}

impl<'a, T: Scalar> Iterator for ScalarArrayIter<'a, T> {
    type Item = T;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        let value = self.array.get(self.index)?;
        self.index += 1;
        Some(value)
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.array.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl<T: Scalar> ExactSizeIterator for ScalarArrayIter<'_, T> {}
impl<T: Scalar> core::iter::FusedIterator for ScalarArrayIter<'_, T> {}

#[derive(Clone, Copy, Debug)]
pub struct PairView<'a> {
    tail: &'a [u32],
    key_width: usize,
    value_width: usize,
}

impl<'a> PairView<'a> {
    #[inline(always)]
    pub fn key(self) -> FieldView<'a> {
        FieldView {
            tail: self.tail,
            width: self.key_width,
        }
    }

    #[inline(always)]
    pub fn value(self) -> FieldView<'a> {
        FieldView {
            tail: &self.tail[self.key_width..],
            width: self.value_width,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MapView<'a> {
    index: PerfectHashView<'a>,
    body: &'a [u32],
    len: usize,
    key_width: usize,
    value_width: usize,
    total_words: usize,
}

impl<'a> MapView<'a> {
    #[inline(always)]
    pub fn new(words: &'a [u32]) -> Option<Self> {
        let head = *words.first()?;
        let key_width = ((head >> 30) & 3) as usize;
        let value_width = ((head >> 28) & 3) as usize;
        if key_width == 0 || value_width == 0 {
            return None;
        }

        let index = PerfectHashView::new(bytes_of_words(words)).ok()?;
        let body_offset = word_size(index.data_size());
        let body = words.get(body_offset..)?;
        let pair_words = index.len().checked_mul(key_width + value_width)?;
        body.get(..pair_words)?;
        Some(Self {
            index,
            body,
            len: index.len(),
            key_width,
            value_width,
            total_words: body_offset.checked_add(pair_words)?,
        })
    }

    #[inline(always)]
    pub fn detect_len(words: &'a [u32]) -> Option<usize> {
        let head = *words.first()?;
        let key_width = ((head >> 30) & 3) as usize;
        let value_width = ((head >> 28) & 3) as usize;
        if key_width == 0 || value_width == 0 {
            return None;
        }
        let index = PerfectHashView::new(bytes_of_words(words)).ok()?;
        word_size(index.data_size()).checked_add(index.len().checked_mul(key_width + value_width)?)
    }

    #[inline(always)]
    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        words.get(..Self::detect_len(words)?)
    }

    #[inline(always)]
    pub fn len(self) -> usize {
        self.len
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub(crate) fn total_words(self) -> usize {
        self.total_words
    }

    #[inline(always)]
    pub fn pair(self, index: usize) -> Option<PairView<'a>> {
        if index >= self.len {
            return None;
        }
        let width = self.key_width + self.value_width;
        let start = index.checked_mul(width)?;
        Some(PairView {
            tail: self.body.get(start..)?,
            key_width: self.key_width,
            value_width: self.value_width,
        })
    }

    #[inline(always)]
    pub fn expect_pair(self, index: usize) -> PairView<'a> {
        let width = self.key_width + self.value_width;
        let start = index * width;
        PairView {
            tail: &self.body[start..],
            key_width: self.key_width,
            value_width: self.value_width,
        }
    }


    #[inline(always)]
    pub fn iter(self) -> MapIter<'a> {
        MapIter {
            map: self,
            index: 0,
        }
    }

    #[inline(always)]
    pub fn find_bytes(self, key: &[u8]) -> Option<PairView<'a>> {
        let pos = self.index.locate(key)?;
        let pair = self.pair(pos)?;
        if pair.key().string()?.as_bytes() == key {
            Some(pair)
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn find_str(self, key: &str) -> Option<PairView<'a>> {
        self.find_bytes(key.as_bytes())
    }

    #[inline(always)]
    pub fn find_scalar<K: MapKey + Scalar + PartialEq>(self, key: K) -> Option<PairView<'a>> {
        let key_bytes = key.as_key_bytes();
        let pos = self.index.locate(&key_bytes)?;
        let pair = self.pair(pos)?;
        if pair.key().scalar::<K>()? == key {
            Some(pair)
        } else {
            None
        }
    }
}

pub struct MapIter<'a> {
    map: MapView<'a>,
    index: usize,
}

impl<'a> Iterator for MapIter<'a> {
    type Item = PairView<'a>;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        let item = self.map.pair(self.index)?;
        self.index += 1;
        Some(item)
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.map.len.saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for MapIter<'_> {}
impl core::iter::FusedIterator for MapIter<'_> {}

pub struct ViewMap<'a, K, V> {
    map: MapView<'a>,
    _key: PhantomData<K>,
    _value: PhantomData<V>,
}

impl<'a, K, V> Copy for ViewMap<'a, K, V> {}

impl<'a, K, V> Clone for ViewMap<'a, K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, K: FieldDecode<'a>, V: FieldDecode<'a>> ViewMap<'a, K, V> {
    #[inline(always)]
    pub fn new(map: MapView<'a>) -> Self {
        Self {
            map,
            _key: PhantomData,
            _value: PhantomData,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[inline(always)]
    pub fn get(&self, index: usize) -> Option<(K, V)> {
        let pair = self.map.pair(index)?;
        Some((K::decode(pair.key())?, V::decode(pair.value())?))
    }

    #[inline(always)]
    pub fn iter(&self) -> ViewMapIter<'a, K, V> {
        ViewMapIter {
            map: *self,
            index: 0,
        }
    }

    #[inline(always)]
    pub fn raw(&self) -> MapView<'a> {
        self.map
    }
}

impl<'a, V: FieldDecode<'a>> ViewMap<'a, StringView<'a>, V> {
    #[inline(always)]
    pub fn find_str(&self, key: &str) -> Option<(StringView<'a>, V)> {
        let pair = self.map.find_str(key)?;
        Some((StringView::decode(pair.key())?, V::decode(pair.value())?))
    }
}

impl<'a, K: FieldDecode<'a> + MapKey + Scalar + PartialEq, V: FieldDecode<'a>> ViewMap<'a, K, V> {
    #[inline(always)]
    pub fn find_scalar(&self, key: K) -> Option<(K, V)> {
        let pair = self.map.find_scalar(key)?;
        Some((K::decode(pair.key())?, V::decode(pair.value())?))
    }
}

pub struct ViewMapIter<'a, K, V> {
    map: ViewMap<'a, K, V>,
    index: usize,
}

impl<'a, K: FieldDecode<'a>, V: FieldDecode<'a>> Iterator for ViewMapIter<'a, K, V> {
    type Item = (K, V);

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        let value = self.map.get(self.index)?;
        self.index += 1;
        Some(value)
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.map.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl<'a, K: FieldDecode<'a>, V: FieldDecode<'a>> ExactSizeIterator for ViewMapIter<'a, K, V> {}
impl<'a, K: FieldDecode<'a>, V: FieldDecode<'a>> core::iter::FusedIterator
    for ViewMapIter<'a, K, V>
{
}

impl<'a> IntoIterator for BoolArray<'a> {
    type Item = bool;
    type IntoIter = core::iter::Map<core::iter::Copied<core::slice::Iter<'a, u8>>, fn(u8) -> bool>;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        fn as_bool(value: u8) -> bool {
            value != 0
        }

        self.bytes.iter().copied().map(as_bool)
    }
}

impl<'a, T: Scalar> IntoIterator for ScalarArray<'a, T> {
    type Item = T;
    type IntoIter = ScalarArrayIter<'a, T>;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: FieldDecode<'a>> IntoIterator for ViewArray<'a, T> {
    type Item = T;
    type IntoIter = ViewArrayIter<'a, T>;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K: FieldDecode<'a>, V: FieldDecode<'a>> IntoIterator for ViewMap<'a, K, V> {
    type Item = (K, V);
    type IntoIter = ViewMapIter<'a, K, V>;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> FieldDecode<'a> for StringView<'a> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        field.string()
    }
}

impl<'a> FieldDecode<'a> for MessageView<'a> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        field.message()
    }
}

impl<'a> FieldDecode<'a> for ArrayView<'a> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        field.array()
    }
}

impl<'a> FieldDecode<'a> for MapView<'a> {
    fn decode(field: FieldView<'a>) -> Option<Self> {
        field.map()
    }
}

#[inline(always)]
pub fn detect_slice_end(words: &[u32], detected: &[u32], end: &mut usize) -> Option<()> {
    let offset = unsafe { detected.as_ptr().offset_from(words.as_ptr()) as usize };
    *end = (*end).max(offset.checked_add(detected.len())?);
    Some(())
}

#[inline(always)]
pub fn detect_array_with<'a>(
    words: &'a [u32],
    mut detect: impl FnMut(FieldView<'a>) -> Option<&'a [u32]>,
) -> Option<&'a [u32]> {
    let array = ArrayView::new(words)?;
    let end = array.total_words();
    let base_end = unsafe { words.as_ptr().add(end) };
    for index in (0..array.len()).rev() {
        let detected = detect(array.expect_field(index))?;
        if unsafe { detected.as_ptr().add(detected.len()) } > base_end {
            let offset = unsafe { detected.as_ptr().offset_from(words.as_ptr()) as usize };
            return words.get(..offset.checked_add(detected.len())?);
        }
    }
    words.get(..end)
}

#[inline(always)]
pub fn detect_map_with<'a>(
    words: &'a [u32],
    mut detect_key: impl FnMut(FieldView<'a>) -> Option<&'a [u32]>,
    mut detect_value: impl FnMut(FieldView<'a>) -> Option<&'a [u32]>,
) -> Option<&'a [u32]> {
    let map = MapView::new(words)?;
    let end = map.total_words();
    let base_end = unsafe { words.as_ptr().add(end) };
    for index in (0..map.len()).rev() {
        let pair = map.expect_pair(index);
        let detected = detect_value(pair.value())?;
        if unsafe { detected.as_ptr().add(detected.len()) } > base_end {
            let offset = unsafe { detected.as_ptr().offset_from(words.as_ptr()) as usize };
            return words.get(..offset.checked_add(detected.len())?);
        }

        let detected = detect_key(pair.key())?;
        if unsafe { detected.as_ptr().add(detected.len()) } > base_end {
            let offset = unsafe { detected.as_ptr().offset_from(words.as_ptr()) as usize };
            return words.get(..offset.checked_add(detected.len())?);
        }
    }
    words.get(..end)
}

#[inline(always)]
fn bytes_of_words(words: &[u32]) -> &[u8] {
    // u8 alignment is 1, so viewing the word buffer as bytes is safe.
    unsafe { core::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 4) }
}

#[inline(always)]
fn count32(v: u32) -> usize {
    ((v & 0xaaaa_aaaa).count_ones() + v.count_ones()) as usize
}

#[inline(always)]
fn count64(v: u64) -> usize {
    ((v & 0xaaaa_aaaa_aaaa_aaaa).count_ones() + v.count_ones()) as usize
}
