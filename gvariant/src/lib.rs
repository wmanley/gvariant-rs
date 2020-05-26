use std::{
    convert::TryInto,
    error::Error,
    ffi::CStr,
    fmt::{Debug, Display},
    marker::PhantomData,
};

use ref_cast::RefCast;

pub mod aligned_bytes;
use offset::align_offset;

pub mod casting;
pub mod offset;

use aligned_bytes::{empty_aligned, AlignedSlice};
use casting::{AlignOf, AllBitPatternsValid};

pub trait Cast: casting::AlignOf + casting::AllBitPatternsValid + 'static {
    fn default_ref() -> &'static Self;
    fn try_from_aligned_slice(
        slice: &AlignedSlice<Self::AlignOf>,
    ) -> Result<&Self, casting::WrongSize>;
    fn try_from_aligned_slice_mut(
        slice: &mut AlignedSlice<Self::AlignOf>,
    ) -> Result<&mut Self, casting::WrongSize>;
    fn from_aligned_slice(slice: &AlignedSlice<Self::AlignOf>) -> &Self {
        match Self::try_from_aligned_slice(slice) {
            Ok(x) => x,
            Err(_) => Self::default_ref(),
        }
    }
}

macro_rules! impl_cast_for {
    ($t:ty, $default:expr) => {
        impl Cast for $t {
            fn default_ref() -> &'static Self {
                &$default
            }
            fn try_from_aligned_slice(
                slice: &AlignedSlice<Self::AlignOf>,
            ) -> Result<&Self, casting::WrongSize> {
                casting::try_cast_slice_to::<Self>(slice)
            }
            fn try_from_aligned_slice_mut(
                slice: &mut AlignedSlice<Self::AlignOf>,
            ) -> Result<&mut Self, casting::WrongSize> {
                casting::try_cast_slice_to_mut::<Self>(slice)
            }
        }
    };
}

impl_cast_for!(Bool, Bool(0u8));
impl_cast_for!(u8, 0);
impl_cast_for!(u16, 0);
impl_cast_for!(i16, 0);
impl_cast_for!(u32, 0);
impl_cast_for!(i32, 0);
impl_cast_for!(u64, 0);
impl_cast_for!(i64, 0);
impl_cast_for!(f64, 0.);

// Array of fixed size types

#[derive(RefCast)]
#[repr(transparent)]
pub struct Str {
    data: [u8],
}

impl Str {
    pub fn to_bytes(&self) -> &[u8] {
        let d: &[u8] = self.data.as_ref();
        match d.last() {
            Some(b'\0') => &d[..d.len() - 1],
            _ => b"",
        }
    }
    pub fn to_cstr(&self) -> &CStr {
        let mut d: &[u8] = self.data.as_ref();
        match d.last() {
            Some(b'\0') => (),
            _ => d = b"\0",
        }
        CStr::from_bytes_with_nul(&d[..=d.iter().position(|x| *x == b'\0').unwrap()]).unwrap()
    }
}
unsafe impl AllBitPatternsValid for Str {}
unsafe impl AlignOf for Str {
    type AlignOf = aligned_bytes::A1;
}

impl Cast for Str {
    fn default_ref() -> &'static Self {
        unsafe { &*(b"" as *const [u8] as *const Str) }
    }
    fn try_from_aligned_slice(
        slice: &AlignedSlice<Self::AlignOf>,
    ) -> Result<&Self, casting::WrongSize> {
        Ok(Self::ref_cast(slice.as_ref()))
    }
    fn try_from_aligned_slice_mut(
        slice: &mut AlignedSlice<Self::AlignOf>,
    ) -> Result<&mut Self, casting::WrongSize> {
        Ok(Self::ref_cast_mut(slice.as_mut()))
    }
}

impl PartialEq for Str {
    fn eq(&self, other: &Self) -> bool {
        self.to_cstr() == other.to_cstr()
    }
}

pub struct Variant {}

#[derive(Debug)]
pub enum NonNormal {
    NotNullTerminated,
    WrongSize,
}
impl Display for NonNormal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GVariant data not in normal form: {:?}", self)
    }
}
impl Error for NonNormal {}

// #### 2.5.3.1 Fixed Width Arrays
//
// In this case, the serialised form of each array element is packed
// sequentially, with no extra padding or framing, to obtain the array. Since
// all fixed-sized values have a size that is a multiple of their alignment
// requirement, and since all elements in the array will have the same alignment
// requirements, all elements are automatically aligned.
//
// The length of the array can be determined by taking the size of the array and
// dividing by the fixed element size. This will always work since all
// fixed-size values have a non-zero size.
//
// We implement this a normal rust slice.

impl<'a, T: Cast + casting::AlignOf + AllBitPatternsValid + Sized + 'static> Cast for [T] {
    fn default_ref() -> &'static Self {
        &[]
    }
    fn try_from_aligned_slice(
        slice: &AlignedSlice<Self::AlignOf>,
    ) -> Result<&Self, casting::WrongSize> {
        casting::cast_slice::<Self::AlignOf, T>(slice)
    }
    fn try_from_aligned_slice_mut(
        _: &mut AlignedSlice<Self::AlignOf>,
    ) -> Result<&mut Self, casting::WrongSize> {
        todo!()
    }
}

// 2.3.6 Framing Offsets
//
// If a container contains non-fixed-size child elements, it is the
// responsibility of the container to be able to determine their sizes. This is
// done using framing offsets.
//
// A framing offset is an integer of some predetermined size. The size is always
// a power of 2. The size is determined from the overall size of the container
// byte sequence. It is chosen to be just large enough to reference each of the
// byte boundaries in the container.
//
// As examples, a container of size 0 would have framing offsets of size 0
// (since no bits are required to represent no choice). A container of sizes 1
// through 255 would have framing offsets of size 1 (since 256 choices can be
// represented with a single byte). A container of sizes 256 through 65535 would
// have framing offsets of size 2. A container of size 65536 would have framing
// offsets of size 4.
//
// There is no theoretical upper limit in how large a framing offset can be.
// This fact (along with the absence of other limitations in the serialisation
// format) allows for values of arbitrary size.
//
// When serialising, the proper framing offset size must be determined by “trial
// and error” — checking each size to determine if it will work. It is possible,
// since the size of the offsets is included in the size of the container, that
// having larger offsets might bump the size of the container up into the next
// category, which would then require larger offsets. Such containers, however,
// would not be considered to be in “normal form”. The smallest possible offset
// size must be used if the serialised data is to be in normal form.
//
// Framing offsets always appear at the end of containers and are unaligned.
// They are always stored in little-endian byte order.

#[derive(Debug, Copy, Clone)]
pub enum OffsetSize {
    U0 = 0,
    U1 = 1,
    U2 = 2,
    U4 = 4,
    U8 = 8,
}

pub fn offset_size(len: usize) -> OffsetSize {
    match len {
        0 => OffsetSize::U0,
        0x1..=0xFF => OffsetSize::U1,
        0x100..=0xFFFF => OffsetSize::U2,
        0x10000..=0xFFFFFFFF => OffsetSize::U4,
        0x100000000..=0xFFFFFFFFFFFFFFFF => OffsetSize::U8,
        _ => unreachable!(),
    }
}

fn read_uint(data: &[u8], size: OffsetSize, n: usize) -> usize {
    let s = n * size as usize;
    match size {
        OffsetSize::U0 => 0,
        OffsetSize::U1 => data[s] as usize,
        OffsetSize::U2 => u16::from_le_bytes(data[s..s + 2].try_into().unwrap()) as usize,
        OffsetSize::U4 => u32::from_le_bytes(data[s..s + 4].try_into().unwrap()) as usize,
        OffsetSize::U8 => u64::from_le_bytes(data[s..s + 8].try_into().unwrap()) as usize,
    }
}

fn read_last_frame_offset(data: &[u8]) -> (OffsetSize, usize) {
    let osz = offset_size(data.len());
    (osz, read_uint(&data[data.len() - osz as usize..], osz, 0))
}

// Non-fixed width arrays

#[derive(RefCast, Debug)]
#[repr(transparent)]
pub struct NonFixedWidthArray<T: Cast + ?Sized> {
    data: AlignedSlice<T::AlignOf>,
}

unsafe impl<T: Cast + ?Sized> AlignOf for NonFixedWidthArray<T> {
    type AlignOf = T::AlignOf;
}
unsafe impl<T: Cast + ?Sized> AllBitPatternsValid for NonFixedWidthArray<T> {}
impl<T: Cast + ?Sized> Cast for NonFixedWidthArray<T> {
    fn default_ref() -> &'static Self {
        Self::ref_cast(empty_aligned())
    }
    fn try_from_aligned_slice(
        slice: &AlignedSlice<Self::AlignOf>,
    ) -> Result<&Self, casting::WrongSize> {
        Ok(Self::ref_cast(slice))
    }
    fn try_from_aligned_slice_mut(
        slice: &mut AlignedSlice<Self::AlignOf>,
    ) -> Result<&mut Self, casting::WrongSize> {
        Ok(Self::ref_cast_mut(slice))
    }
}

impl<T: Cast + ?Sized> NonFixedWidthArray<T> {
    pub fn len(&self) -> usize {
        if self.data.is_empty() {
            0
        } else {
            // Since determining the length of the array relies on our ability
            // to count the number of framing offsets and since the number of
            // framing offsets is determined from how much space they take up,
            // zero byte framing offsets are not permitted in arrays, even in
            // the case where all other serialised data has a size of zero. This
            // special exception avoids having to divide zero by zero and wonder
            // what the answer is.
            let (osz, lfo) = read_last_frame_offset(&self.data);
            match osz {
                OffsetSize::U0 => unreachable!(),
                x => (self.data.len() - lfo) / (x as usize),
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
pub struct NonFixedWidthArrayIterator<'a, Item: Cast + ?Sized> {
    slice: &'a NonFixedWidthArray<Item>,
    next_start: usize,
    offset_idx: usize,
    offset_size: OffsetSize,
}
impl<'a, Item: Cast + 'static + ?Sized> Iterator for NonFixedWidthArrayIterator<'a, Item> {
    type Item = &'a Item;
    fn next(&mut self) -> Option<Self::Item> {
        if self.offset_idx == self.slice.data.len() {
            None
        } else {
            let start = align_offset::<Item::AlignOf>(self.next_start);
            let end = read_uint(
                &self.slice.data.as_ref()[self.offset_idx..],
                self.offset_size,
                0,
            );
            self.offset_idx += self.offset_size as usize;
            self.next_start = end;
            if end < start || end >= self.slice.data.len() {
                // If the framing offsets (or calculations based on them)
                // indicate that any part of the byte sequence of a child value
                // would fall outside of the byte sequence of the parent then
                // the child is given the default value for its type.
                Some(Item::try_from_aligned_slice(aligned_bytes::empty_aligned()).unwrap())
            } else {
                Some(Item::try_from_aligned_slice(&self.slice.data[..end][start..]).unwrap())
            }
        }
    }
}

impl<'a, Item: Cast + 'static + ?Sized> IntoIterator for &'a NonFixedWidthArray<Item> {
    type Item = &'a Item;
    type IntoIter = NonFixedWidthArrayIterator<'a, Item>;
    fn into_iter(self) -> Self::IntoIter {
        let (osz, lfo) = read_last_frame_offset(&self.data);
        NonFixedWidthArrayIterator {
            slice: self,
            next_start: 0,
            offset_idx: lfo,
            offset_size: osz,
        }
    }
}
impl<Item: Cast + 'static + ?Sized> core::ops::Index<usize> for NonFixedWidthArray<Item> {
    type Output = Item;
    fn index(&self, index: usize) -> &Self::Output {
        let (osz, lfo) = read_last_frame_offset(&self.data);
        let frame_offsets = &self.data.as_ref()[lfo..];
        let end = read_uint(frame_offsets, osz, index);
        let start = align_offset::<Item::AlignOf>(match index {
            0 => 0,
            x => read_uint(frame_offsets, osz, x - 1),
        });
        if start < self.data.len() && end < self.data.len() && start <= end {
            Item::try_from_aligned_slice(&self.data[..end][start..]).unwrap()
        } else {
            // Start or End Boundary of a Child Falls Outside the Container
            //
            // If the framing offsets (or calculations based on them) indicate
            // that any part of the byte sequence of a child value would fall
            // outside of the byte sequence of the parent then the child is given
            // the default value for its type.
            Item::try_from_aligned_slice(aligned_bytes::empty_aligned()).unwrap()
        }
    }
}

// 2.5.2 Maybes
//
// Maybes are encoded differently depending on if their element type is
// fixed-sized or not.
//
// The alignment of a maybe type is always equal to the alignment of its element
// type.

#[repr(transparent)]
#[derive(Debug, RefCast)]
pub struct MaybeFixedSize<T: Cast> {
    marker: PhantomData<T>,
    data: AlignedSlice<T::AlignOf>,
}
impl<T: Cast> MaybeFixedSize<T> {
    pub fn to_option(&self) -> Option<&T> {
        // 2.5.2.1 Maybe of a Fixed-Sized Element
        //
        // For the `Nothing` case, the serialised data is the empty byte
        // sequence.  For the `Just` case, the serialised data is exactly
        // equal to the serialised data of the child.  This is always
        // distinguishable from the `Nothing` case because all fixed-sized
        // values have a non-zero size.
        //
        // Wrong Size for Fixed Sized Maybe
        //
        // In the event that a maybe instance with a fixed element size
        // is not exactly equal to the size of that element, then the
        // value is taken to be `Nothing`.
        T::try_from_aligned_slice(&self.data).ok()
    }
}

impl<'a, T: Cast> From<&'a MaybeFixedSize<T>> for Option<&'a T> {
    fn from(m: &'a MaybeFixedSize<T>) -> Self {
        m.to_option()
    }
}

impl<T: Cast + PartialEq> PartialEq for MaybeFixedSize<T> {
    fn eq(&self, other: &Self) -> bool {
        self.to_option() == other.to_option()
    }
}

unsafe impl<T: Cast> AlignOf for MaybeFixedSize<T> {
    type AlignOf = T::AlignOf;
}
unsafe impl<T: Cast> AllBitPatternsValid for MaybeFixedSize<T> {}

impl<T: Cast + AlignOf> Cast for MaybeFixedSize<T> {
    fn default_ref() -> &'static Self {
        Self::ref_cast(empty_aligned())
    }
    fn try_from_aligned_slice(
        slice: &AlignedSlice<Self::AlignOf>,
    ) -> Result<&Self, casting::WrongSize> {
        Ok(Self::ref_cast(slice))
    }
    fn try_from_aligned_slice_mut(
        slice: &mut AlignedSlice<Self::AlignOf>,
    ) -> Result<&mut Self, casting::WrongSize> {
        Ok(Self::ref_cast_mut(slice))
    }
}

#[derive(Debug, RefCast)]
#[repr(transparent)]
pub struct MaybeNonFixedSize<T: Cast + ?Sized> {
    marker: PhantomData<T>,
    data: AlignedSlice<T::AlignOf>,
}
impl<T: Cast + ?Sized> MaybeNonFixedSize<T> {
    pub fn to_option(&self) -> Option<&T> {
        if self.data.is_empty() {
            // #### 2.5.2.2 Maybe of a Non-Fixed-Sized Element
            //
            // For the `Nothing` case, the serialised data is, again, the empty
            // byte sequence.
            None
        } else {
            // For the Just case, the serialised form is the serialised data of
            // the child element, followed by a single zero byte. This extra
            // byte ensures that the `Just` case is distinguishable from the
            // `Nothing` case even in the event that the child value has a size
            // of zero.
            Some(T::try_from_aligned_slice(&self.data[..self.data.len() - 1]).unwrap())
        }
    }
}

unsafe impl<T: Cast + ?Sized> AlignOf for MaybeNonFixedSize<T> {
    type AlignOf = T::AlignOf;
}
unsafe impl<T: Cast + ?Sized> AllBitPatternsValid for MaybeNonFixedSize<T> {}

impl<T: Cast + ?Sized> Cast for MaybeNonFixedSize<T> {
    fn default_ref() -> &'static Self {
        Self::ref_cast(empty_aligned())
    }
    fn try_from_aligned_slice(
        slice: &AlignedSlice<Self::AlignOf>,
    ) -> Result<&Self, casting::WrongSize> {
        Ok(Self::ref_cast(slice))
    }
    fn try_from_aligned_slice_mut(
        slice: &mut AlignedSlice<Self::AlignOf>,
    ) -> Result<&mut Self, casting::WrongSize> {
        Ok(Self::ref_cast_mut(slice))
    }
}

impl<'a, T: Cast + ?Sized> From<&'a MaybeNonFixedSize<T>> for Option<&'a T> {
    fn from(m: &'a MaybeNonFixedSize<T>) -> Self {
        m.to_option()
    }
}

impl<T: Cast + PartialEq + ?Sized> PartialEq for MaybeNonFixedSize<T> {
    fn eq(&self, other: &Self) -> bool {
        self.to_option() == other.to_option()
    }
}

/// Rust's built in bool doesn't have the same representation as GVariant's, so
/// we need our own type here.  Rust's must either be 0x00 (false) or 0x01
/// (true), while with GVariant any value in the range 0x01..=0xFF is true.
#[derive(Debug, RefCast)]
#[repr(transparent)]
pub struct Bool(u8);
impl Bool {
    pub fn to_bool(&self) -> bool {
        self.0 > 0
    }
}
unsafe impl AllBitPatternsValid for Bool {}
unsafe impl AlignOf for Bool {
    type AlignOf = aligned_bytes::A1;
}

pub fn nth_last_frame_offset(data: &[u8], osz: OffsetSize, n: usize) -> usize {
    let off = data.len() - (n + 1) * osz as usize;
    read_uint(&data[off..], osz, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aligned_bytes::{copy_to_align, AlignedSlice, AsAligned, A8};

    #[test]
    fn test_numbers() {
        let data = copy_to_align(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let aligned_slice: &AlignedSlice<A8> = data.as_ref();

        // If the size doesn't match exactly it should default to 0:
        assert_eq!(
            *i32::from_aligned_slice(&aligned_slice[..0].as_aligned()),
            0
        );
        assert_eq!(
            *i32::from_aligned_slice(&aligned_slice[..3].as_aligned()),
            0
        );
        assert_eq!(
            *i32::from_aligned_slice(&aligned_slice[..5].as_aligned()),
            0
        );
        assert_eq!(
            *i32::from_aligned_slice(&aligned_slice[..8].as_aligned()),
            0
        );

        // Common case (Little endian):
        assert_eq!(
            Bool::from_aligned_slice(&aligned_slice[..1].as_aligned()).to_bool(),
            true
        );
        assert_eq!(
            *u8::from_aligned_slice(&aligned_slice[..1].as_aligned()),
            0x01
        );
        assert_eq!(
            *i16::from_aligned_slice(&aligned_slice[..2].as_aligned()),
            0x0201
        );
        assert_eq!(
            *u16::from_aligned_slice(&aligned_slice[..2].as_aligned()),
            0x0201
        );
        assert_eq!(
            *i32::from_aligned_slice(&aligned_slice[..4].as_aligned()),
            0x04030201
        );
        assert_eq!(
            *u32::from_aligned_slice(&aligned_slice[..4].as_aligned()),
            0x04030201
        );
        assert_eq!(
            *i64::from_aligned_slice(&aligned_slice[..8]),
            0x0807060504030201
        );
        assert_eq!(
            *u64::from_aligned_slice(&aligned_slice[..8]),
            0x0807060504030201
        );
        assert_eq!(
            *f64::from_aligned_slice(&aligned_slice[..8]),
            f64::from_bits(0x0807060504030201)
        );
    }
    #[test]
    fn test_non_fixed_width_maybe() {
        assert_eq!(
            MaybeNonFixedSize::<Str>::from_aligned_slice(b"".as_aligned()).to_option(),
            None
        );
        assert_eq!(
            MaybeNonFixedSize::<Str>::from_aligned_slice(b"\0".as_aligned())
                .to_option()
                .unwrap()
                .to_bytes(),
            b""
        );
        assert_eq!(
            MaybeNonFixedSize::<Str>::from_aligned_slice(b"hello world\0\0".as_aligned())
                .to_option()
                .unwrap()
                .to_bytes(),
            b"hello world"
        );
    }

    #[test]
    fn test_non_fixed_width_array() {
        let a_s = NonFixedWidthArray::<Str>::from_aligned_slice(b"".as_aligned());
        assert_eq!(a_s.len(), 0);
        assert!(a_s.into_iter().collect::<Vec<_>>().is_empty());

        let a_s =
            NonFixedWidthArray::<Str>::from_aligned_slice(b"hello\0world\0\x06\x0c".as_aligned());
        assert_eq!(a_s.len(), 2);
        assert_eq!(
            a_s.into_iter().map(|x| x.to_bytes()).collect::<Vec<_>>(),
            &[b"hello", b"world"]
        );
        assert_eq!(a_s[0].to_bytes(), b"hello");
        assert_eq!(a_s[1].to_bytes(), b"world");
    }

    #[test]
    fn test_spec_examples() {
        assert_eq!(
            Str::from_aligned_slice(b"hello world\0".as_aligned()).to_bytes(),
            b"hello world"
        );
        assert_eq!(
            MaybeNonFixedSize::<Str>::from_aligned_slice(b"hello world\0\0".as_aligned())
                .to_option()
                .unwrap()
                .to_bytes(),
            b"hello world"
        );
        let aob = <[Bool]>::from_aligned_slice([1u8, 0, 0, 1, 1].as_aligned());
        assert_eq!(
            aob.iter().map(|x| x.to_bool()).collect::<Vec<_>>(),
            [true, false, false, true, true]
        );

        // String Array Example
        //
        // With type 'as':
        let v: Vec<_> = NonFixedWidthArray::<Str>::from_aligned_slice(
            b"i\0can\0has\0strings?\0\x02\x06\x0a\x13".as_aligned(),
        )
        .into_iter()
        .map(|x| x.to_bytes())
        .collect();
        assert_eq!(v, [b"i".as_ref(), b"can", b"has", b"strings?"]);

        // Array of Bytes Example
        //
        // With type 'ay':
        let aob = <[u8]>::from_aligned_slice([0x04u8, 0x05, 0x06, 0x07].as_aligned());
        assert_eq!(aob, &[0x04u8, 0x05, 0x06, 0x07]);

        // Array of Integers Example
        //
        // With type 'ai':
        let data = copy_to_align(b"\x04\0\0\0\x02\x01\0\0");
        let aoi = <[i32]>::from_aligned_slice(data.as_ref());
        assert_eq!(aoi, [4, 258]);

        // Dictionary Entry Example
        //
        // With type '{si}':
        //    'a sp 'k 'e  'y \0 -- --   02 02 00 00 06has a value of {'a key', 514}
    }

    #[test]
    fn test_gvariantstr() {
        assert_eq!(Str::from_aligned_slice(b"".as_aligned()).to_bytes(), b"");
        assert_eq!(Str::from_aligned_slice(b"\0".as_aligned()).to_bytes(), b"");
        assert_eq!(
            Str::from_aligned_slice(b"hello world\0".as_aligned()).to_bytes(),
            b"hello world"
        );
    }
}