use core::{marker::PhantomData, mem::MaybeUninit, ops::{Index, IndexMut}};



#[derive(Debug, Clone, Copy)]
pub enum RingBufferError {
    Overflow,
}

impl RingBufferError {
    pub fn result_ignore_overflow(r: Result<(), RingBufferError>) -> Result<(), RingBufferError> {
        r.or_else(|e| match e {
            RingBufferError::Overflow => Ok(()),
            _ => Err(e), // TODO: fix `unreachable_patterns` warning
        })
    }
}


// `Sealed` is in private module because without mod `Sealed` would not be `pub` and compiler would emit `private_bounds` warning on `OnOverflow`.
// This way `Sealed` can be `pub` but it is still hidden from users because it is in non `pub` module.
mod private {
    pub trait Sealed {}
}

pub trait OnOverflow: private::Sealed {}

pub struct Overwrite;
pub struct Ignore;

impl private::Sealed for Ignore {}
impl private::Sealed for Overwrite {}
impl OnOverflow for Ignore {}
impl OnOverflow for Overwrite {}

pub struct RingBuffer<T, const N: usize, OVERFLOW: OnOverflow> {
    buf: [MaybeUninit<T>; N],
    pos: usize,
    len: usize,
    phantom: PhantomData<OVERFLOW>,
}

impl<T, const N: usize, OVERFLOW: OnOverflow> RingBuffer<T, N, OVERFLOW> {
    const ELEM_UNINIT: MaybeUninit<T> = MaybeUninit::<T>::uninit();

    pub const fn new() -> Self {
        Self {
            buf: [Self::ELEM_UNINIT; N],
            pos: 0,
            len: 0,
            phantom: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            None
        } else {
            // SAFETY
            // When `len` is non zero the element at `pos` is inside intialized range.
            // After `pop_front`, the value at `pos` is no longer part of initialized range so no duplication happens.
            let v = unsafe { self.buf[self.pos].assume_init_read() };
            self.pos = (self.pos + 1) % N;
            self.len -= 1;

            Some(v)
        }
    }

    pub fn pop_back(&mut self) -> Option<T> {
        if self.len == 0 {
            None
        } else {
            // SAFETY
            // When `len` is non zero the element at `pos` is insi intialized range.
            // After `pop_front`, the value at `pos` is no longer part of initialized range so no duplication happens.
            let v = unsafe { self.buf[(self.pos + self.len - 1) % N].assume_init_read() };
            self.len -= 1;

            Some(v)
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if index < self.len {
            // SAFETY value is in initialized range
            Some(unsafe { self.buf[(self.pos + index) % N].assume_init_ref() })
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index < self.len {
            // SAFETY value is in initialized range
            Some(unsafe { self.buf[(self.pos + index) % N].assume_init_mut() })
        } else {
            None
        }
    }

    pub fn front(&self) -> Option<&T> {
        self.get(0)
    }

    pub fn front_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
    }

    pub fn back(&self) -> Option<&T> {
        if self.len != 0 {
            // SAFETY value is in initialized range
            Some(unsafe { self.buf[(self.pos + self.len - 1) % N].assume_init_ref() })
        } else {
            None
        }
    }

    pub fn back_mut(&mut self) -> Option<&mut T> {
        if self.len != 0 {
            // SAFETY value is in initialized range
            Some(unsafe { self.buf[(self.pos + self.len - 1) % N].assume_init_mut() })
        } else {
            None
        }
    }
}

impl<T, const N: usize, OVERFLOW: OnOverflow> Index<usize> for RingBuffer<T, N, OVERFLOW> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("RingBuffer::index(...) - out of bounds access")
    }
}

impl<T, const N: usize, OVERFLOW: OnOverflow> IndexMut<usize> for RingBuffer<T, N, OVERFLOW> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.get_mut(index).expect("RingBuffer::index_mut(...) - out of bounds access")
    }
}

impl<T, const N: usize> RingBuffer<T, N, Ignore> {
    pub fn push_back(&mut self, v: T) -> Result<(), RingBufferError> {
        if self.len == N {
            return Err(RingBufferError::Overflow);
        }

        // value is outside if the initialized range (`len != N`), so no leak happens
        self.buf[(self.pos + self.len) % N] = MaybeUninit::new(v);
        self.len += 1;

        Ok(())
    }

    fn extend_prepare_empty_range(&mut self) -> (usize, usize) {
        if self.len == 0 {
            self.pos = 0;
            (0, N)
        } else {
            (
                (self.pos + self.len) % N,
                if self.pos == 0 { N } else { self.pos }
            )
        }
    }

    pub fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) -> Result<(), RingBufferError> {
        let mut iter = iter.into_iter();

        if self.len == N {
            return match iter.next() {
                Some(_) => Err(RingBufferError::Overflow),
                None => Ok(())
            };
        }

        let (extend_start, extend_end) = self.extend_prepare_empty_range();

        if extend_start < extend_end {
            self.len += iter.by_ref().zip(self.buf[extend_start..extend_end].iter_mut()).map(|(v, buf_elem)| { *buf_elem = MaybeUninit::new(v); }).count();
        } else {
            self.len += iter.by_ref().zip(self.buf[extend_start..].iter_mut()).map(|(v, buf_elem)| { *buf_elem = MaybeUninit::new(v); }).count();
            self.len += iter.by_ref().zip(self.buf[..extend_end].iter_mut()).map(|(v, buf_elem)| { *buf_elem = MaybeUninit::new(v); }).count();
        }

        match iter.next() {
            Some(_) => Err(RingBufferError::Overflow),
            None => Ok(())
        }
    }
}

impl<'a, T: Copy + 'a, const N: usize> RingBuffer<T, N, Ignore> {
    pub fn extend_from_refs<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) -> Result<(), RingBufferError> {
        self.extend(iter.into_iter().copied())
    }
}

impl<T: Copy, const N: usize> RingBuffer<T, N, Ignore> {
    fn extend_from_slice_continous<'a>(&mut self, start: usize, end: usize, s: &'a [T]) -> &'a [T] {
        let len = end - start;

        if s.len() < len {
            MaybeUninit::copy_from_slice(&mut self.buf[start..(start + s.len())], s);
            self.len += s.len();
            &[]
        } else {
            MaybeUninit::copy_from_slice(&mut self.buf[start..end], &s[..len]);
            self.len += len;
            &s[len..]
        }
    }

    fn extend_len_into_result(len: usize) -> Result<(), RingBufferError> {
        if len == 0 {
            Ok(())
        } else {
            Err(RingBufferError::Overflow)
        }
    }

    pub fn extend_from_slice(&mut self, s: &[T]) -> Result<(), RingBufferError> {
        if s.len() == 0 {
            return Ok(());
        }

        if self.len == N {
            return Err(RingBufferError::Overflow);
        }

        let (extend_start, extend_end) = self.extend_prepare_empty_range();

        if extend_start < extend_end {
            Self::extend_len_into_result(self.extend_from_slice_continous(extend_start, extend_end, s).len())
        } else {
            let s = self.extend_from_slice_continous(extend_start, N, s);
            if s.len() == 0 {
                Ok(())
            } else {
                Self::extend_len_into_result(self.extend_from_slice_continous(0, extend_end, s).len())
            }
        }
    }
}

impl<T, const N: usize> RingBuffer<T, N, Overwrite> {
    pub fn push_back(&mut self, v: T) {
        if self.len == N {
            // SAFETY `len` == N, so all values are initialized, which means that also value at `pos` is initialized
            unsafe { self.buf[self.pos].assume_init_drop() };
            // value at `pos` is unititialized by line above, so no leak happens
            self.buf[self.pos] = MaybeUninit::new(v);

            self.pos += 1;
        } else {
            // value outside initilized range is acessed, so no leak happens
            self.buf[(self.pos + self.len) % N] = MaybeUninit::new(v);
            self.len += 1;
        }
    }

    // TODO: other methods - extend, extend_from_refs, extend_from_slice
}