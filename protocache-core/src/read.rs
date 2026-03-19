use core::marker::PhantomData;

use crate::common::{Scalar, word_size};
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

impl MapKey for i32 {
    fn as_key_bytes(&self) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
}

impl MapKey for u32 {
    fn as_key_bytes(&self) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
}

impl MapKey for i64 {
    fn as_key_bytes(&self) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
}

impl MapKey for u64 {
    fn as_key_bytes(&self) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StringView<'a> {
    bytes: &'a [u8],
}

impl<'a> StringView<'a> {
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

    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        Some(words.get(..Self::detect_len(words)?)?)
    }

    pub fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub fn as_str(self) -> Option<&'a str> {
        core::str::from_utf8(self.bytes).ok()
    }

    pub fn as_bool_array(self) -> BoolArray<'a> {
        BoolArray { bytes: self.bytes }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BoolArray<'a> {
    bytes: &'a [u8],
}

impl<'a> BoolArray<'a> {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<bool> {
        Some(*self.bytes.get(index)? != 0)
    }

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
    pub fn raw_words(self) -> Option<&'a [u32]> {
        self.tail.get(..self.width)
    }

    pub fn object_words(self) -> Option<&'a [u32]> {
        let first = *self.tail.first()?;
        if (first & 3) == 3 {
            self.tail.get((first >> 2) as usize..)
        } else {
            Some(self.tail)
        }
    }

    pub fn scalar<T: Scalar>(self) -> Option<T> {
        T::from_words(self.raw_words()?)
    }

    pub fn detect_scalar(self) -> Option<&'a [u32]> {
        self.raw_words()
    }

    pub fn string(self) -> Option<StringView<'a>> {
        StringView::new(self.object_words()?)
    }

    pub fn detect_string(self) -> Option<&'a [u32]> {
        StringView::detect(self.object_words()?)
    }

    pub fn message(self) -> Option<MessageView<'a>> {
        MessageView::new(self.object_words()?)
    }

    pub fn detect_message(self) -> Option<&'a [u32]> {
        MessageView::detect(self.object_words()?)
    }

    pub fn array(self) -> Option<ArrayView<'a>> {
        ArrayView::new(self.object_words()?)
    }

    pub fn detect_array(self) -> Option<&'a [u32]> {
        ArrayView::detect(self.object_words()?)
    }

    pub fn map(self) -> Option<MapView<'a>> {
        MapView::new(self.object_words()?)
    }

    pub fn detect_map(self) -> Option<&'a [u32]> {
        MapView::detect(self.object_words()?)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MessageView<'a> {
    words: &'a [u32],
}

impl<'a> MessageView<'a> {
    pub fn new(words: &'a [u32]) -> Option<Self> {
        let head = *words.first()?;
        let section = (head & 0xff) as usize;
        let body = 1usize.checked_add(section.checked_mul(2)?)?;
        words.get(..body)?;
        Some(Self { words })
    }

    pub fn from_words(words: &'a [u32]) -> Option<Self> {
        Self::new(words)
    }

    pub fn raw_words(self) -> &'a [u32] {
        self.words
    }

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

    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        Some(words.get(..Self::detect_len(words)?)?)
    }

    pub fn has_field(self, id: usize) -> bool {
        self.field(id).is_some()
    }

    pub fn field(self, id: usize) -> Option<FieldView<'a>> {
        let head = *self.words.first()?;
        let section = (head & 0xff) as usize;
        let body_index = 1usize.checked_add(section.checked_mul(2)?)?;
        let body = self.words.get(body_index..)?;

        let (width, off) = if id < 12 {
            let mut v = head >> 8;
            let width = ((v >> (id * 2)) & 3) as usize;
            if width == 0 {
                return None;
            }
            v &= !(u32::MAX << (id * 2));
            (width, count32(v))
        } else {
            let section_index = (id - 12) / 25;
            let bit_index = (id - 12) % 25;
            if section_index >= section {
                return None;
            }
            let sec_words = self.words.get(1 + section_index * 2..1 + section_index * 2 + 2)?;
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

        Some(FieldView {
            tail: body.get(off..)?,
            width,
        })
    }

    pub fn scalar<T: Scalar>(self, id: usize) -> Option<T> {
        self.field(id)?.scalar::<T>()
    }

    pub fn string(self, id: usize) -> Option<StringView<'a>> {
        self.field(id)?.string()
    }

    pub fn bytes(self, id: usize) -> Option<&'a [u8]> {
        Some(self.string(id)?.as_bytes())
    }

    pub fn bools(self, id: usize) -> Option<BoolArray<'a>> {
        Some(self.string(id)?.as_bool_array())
    }

    pub fn message(self, id: usize) -> Option<MessageView<'a>> {
        self.field(id)?.message()
    }

    pub fn array(self, id: usize) -> Option<ArrayView<'a>> {
        self.field(id)?.array()
    }

    pub fn map(self, id: usize) -> Option<MapView<'a>> {
        self.field(id)?.map()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ArrayView<'a> {
    body: &'a [u32],
    len: usize,
    width: usize,
}

impl<'a> ArrayView<'a> {
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

    pub fn detect_len(words: &'a [u32]) -> Option<usize> {
        let head = *words.first()?;
        let len = (head >> 2) as usize;
        let width = (head & 3) as usize;
        if width == 0 {
            return None;
        }
        1usize.checked_add(len.checked_mul(width)?)
    }

    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        Some(words.get(..Self::detect_len(words)?)?)
    }

    pub fn len(self) -> usize {
        self.len
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    pub fn width(self) -> usize {
        self.width
    }

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

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.array.field(self.index)?;
        self.index += 1;
        Some(item)
    }
}

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
    pub fn new(array: ArrayView<'a>) -> Self {
        Self {
            array,
            _marker: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.array.len()
    }

    pub fn is_empty(&self) -> bool {
        self.array.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<T> {
        T::decode(self.array.field(index)?)
    }

    pub fn iter(&self) -> ViewArrayIter<'a, T> {
        ViewArrayIter {
            array: *self,
            index: 0,
        }
    }

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

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.array.get(self.index)?;
        self.index += 1;
        Some(value)
    }
}

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
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<T> {
        if index >= self.len {
            return None;
        }
        let start = index.checked_mul(T::WIDTH)?;
        T::from_words(self.words.get(start..start + T::WIDTH)?)
    }

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

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.array.get(self.index)?;
        self.index += 1;
        Some(value)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PairView<'a> {
    tail: &'a [u32],
    key_width: usize,
    value_width: usize,
}

impl<'a> PairView<'a> {
    pub fn key(self) -> FieldView<'a> {
        FieldView {
            tail: self.tail,
            width: self.key_width,
        }
    }

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
}

impl<'a> MapView<'a> {
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
        body.get(..index.len().checked_mul(key_width + value_width)?)?;
        Some(Self {
            index,
            body,
            len: index.len(),
            key_width,
            value_width,
        })
    }

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

    pub fn detect(words: &'a [u32]) -> Option<&'a [u32]> {
        Some(words.get(..Self::detect_len(words)?)?)
    }

    pub fn len(self) -> usize {
        self.len
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }

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

    pub fn iter(self) -> MapIter<'a> {
        MapIter {
            map: self,
            index: 0,
        }
    }

    pub fn find_bytes(self, key: &[u8]) -> Option<PairView<'a>> {
        let pos = self.index.locate(key)?;
        let pair = self.pair(pos)?;
        if pair.key().string()?.as_bytes() == key {
            Some(pair)
        } else {
            None
        }
    }

    pub fn find_str(self, key: &str) -> Option<PairView<'a>> {
        self.find_bytes(key.as_bytes())
    }

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

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.map.pair(self.index)?;
        self.index += 1;
        Some(item)
    }
}

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
    pub fn new(map: MapView<'a>) -> Self {
        Self {
            map,
            _key: PhantomData,
            _value: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<(K, V)> {
        let pair = self.map.pair(index)?;
        Some((K::decode(pair.key())?, V::decode(pair.value())?))
    }

    pub fn iter(&self) -> ViewMapIter<'a, K, V> {
        ViewMapIter {
            map: *self,
            index: 0,
        }
    }

    pub fn raw(&self) -> MapView<'a> {
        self.map
    }
}

impl<'a, V: FieldDecode<'a>> ViewMap<'a, StringView<'a>, V> {
    pub fn find_str(&self, key: &str) -> Option<(StringView<'a>, V)> {
        let pair = self.map.find_str(key)?;
        Some((StringView::decode(pair.key())?, V::decode(pair.value())?))
    }
}

impl<'a, K: FieldDecode<'a> + MapKey + Scalar + PartialEq, V: FieldDecode<'a>> ViewMap<'a, K, V> {
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

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.map.get(self.index)?;
        self.index += 1;
        Some(value)
    }
}

pub trait GeneratedMessage<'a>: Sized {
    fn from_message_view(view: MessageView<'a>) -> Self;
}

pub trait GeneratedDescriptor {
    const FULL_NAME: &'static str;
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

fn bytes_of_words(words: &[u32]) -> &[u8] {
    // u8 alignment is 1, so viewing the word buffer as bytes is safe.
    unsafe { core::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 4) }
}

fn count32(v: u32) -> usize {
    ((v & 0xaaaa_aaaa).count_ones() + v.count_ones()) as usize
}

fn count64(v: u64) -> usize {
    ((v & 0xaaaa_aaaa_aaaa_aaaa).count_ones() + v.count_ones()) as usize
}
