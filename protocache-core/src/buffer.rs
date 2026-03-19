#[derive(Clone, Debug, Default)]
pub struct Buffer {
    data: Vec<u32>,
    off: usize,
}

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity_words(words: usize) -> Self {
        Self {
            data: vec![0; words],
            off: words,
        }
    }

    pub fn clear(&mut self) {
        self.off = self.data.len();
    }

    pub fn allocated_words(&self) -> usize {
        self.data.len()
    }

    pub fn len(&self) -> usize {
        self.data.len().saturating_sub(self.off)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn view(&self) -> &[u32] {
        &self.data[self.off..]
    }

    pub fn head(&self) -> &[u32] {
        self.view()
    }

    pub fn head_mut(&mut self) -> &mut [u32] {
        let off = self.off;
        &mut self.data[off..]
    }

    pub fn at_from_end(&self, words: usize) -> Option<&[u32]> {
        if words > self.data.len() {
            return None;
        }
        let start = self.data.len() - words;
        Some(&self.data[start..])
    }

    pub fn at_from_end_mut(&mut self, words: usize) -> Option<&mut [u32]> {
        if words > self.data.len() {
            return None;
        }
        let start = self.data.len() - words;
        Some(&mut self.data[start..])
    }

    pub fn reserve_words(&mut self, words: usize) {
        if words > self.data.len() {
            self.grow(words);
        }
    }

    pub fn expand(&mut self, delta: usize) -> &mut [u32] {
        if self.off < delta {
            let needed = self.len().saturating_add(delta);
            self.grow(needed);
        }
        self.off -= delta;
        &mut self.data[self.off..self.off + delta]
    }

    pub fn shrink(&mut self, delta: usize) {
        assert!(delta <= self.len());
        self.off += delta;
    }

    pub fn put(&mut self, value: u32) {
        self.expand(1)[0] = value;
    }

    pub fn put_words(&mut self, words: &[u32]) {
        self.expand(words.len()).copy_from_slice(words);
    }

    fn grow(&mut self, min_words: usize) {
        let active = self.view().to_vec();
        let mut new_words = self.data.len().max(8);
        while new_words < min_words {
            new_words = new_words.saturating_mul(2);
        }
        if new_words < min_words {
            new_words = min_words;
        }

        self.data = vec![0; new_words];
        self.off = new_words - active.len();
        self.data[self.off..].copy_from_slice(&active);
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    #[test]
    fn put_prepends_words() {
        let mut buffer = Buffer::new();
        buffer.put(3);
        buffer.put(2);
        buffer.put(1);
        assert_eq!(buffer.view(), &[1, 2, 3]);
    }

    #[test]
    fn expand_preserves_existing_payload() {
        let mut buffer = Buffer::with_capacity_words(4);
        buffer.put_words(&[3, 4]);
        buffer.put_words(&[1, 2]);
        assert_eq!(buffer.view(), &[1, 2, 3, 4]);

        buffer.put_words(&[0]);
        assert_eq!(buffer.view(), &[0, 1, 2, 3, 4]);
        assert!(buffer.allocated_words() >= 5);
    }

    #[test]
    fn shrink_discards_from_front() {
        let mut buffer = Buffer::new();
        buffer.put_words(&[1, 2, 3, 4]);
        buffer.shrink(2);
        assert_eq!(buffer.view(), &[3, 4]);
        buffer.clear();
        assert!(buffer.is_empty());
    }
}
