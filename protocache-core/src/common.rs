use core::mem;

pub type Words<'a> = &'a [u32];
pub type Bytes<'a> = &'a [u8];
pub type EnumValue = i32;

pub const fn word_size(size: usize) -> usize {
    (size + 3) / 4
}

pub trait Scalar: Copy + Sized {
    const WIDTH: usize;
    fn from_words(words: &[u32]) -> Option<Self>;
    fn write_words(self, words: &mut [u32]) -> Option<()>;
}

macro_rules! impl_scalar {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Scalar for $ty {
                const WIDTH: usize = word_size(mem::size_of::<$ty>());

                fn from_words(words: &[u32]) -> Option<Self> {
                    if words.len() != Self::WIDTH {
                        return None;
                    }
                    let mut bytes = [0u8; mem::size_of::<$ty>()];
                    let raw = unsafe {
                        core::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * mem::size_of::<u32>())
                    };
                    bytes.copy_from_slice(raw.get(..mem::size_of::<$ty>())?);
                    Some(<$ty>::from_le_bytes(bytes))
                }

                fn write_words(self, words: &mut [u32]) -> Option<()> {
                    if words.len() != Self::WIDTH {
                        return None;
                    }
                    words.fill(0);
                    let raw = unsafe {
                        core::slice::from_raw_parts_mut(
                            words.as_mut_ptr().cast::<u8>(),
                            words.len() * mem::size_of::<u32>(),
                        )
                    };
                    raw.get_mut(..mem::size_of::<$ty>())?
                        .copy_from_slice(&self.to_le_bytes());
                    Some(())
                }
            }
        )*
    };
}

impl_scalar!(u32, i32, u64, i64, f32, f64);

impl Scalar for bool {
    const WIDTH: usize = 1;

    fn from_words(words: &[u32]) -> Option<Self> {
        Some(u32::from_words(words)? != 0)
    }

    fn write_words(self, words: &mut [u32]) -> Option<()> {
        if words.len() != Self::WIDTH {
            return None;
        }
        words[0] = u32::from(self);
        Some(())
    }
}
