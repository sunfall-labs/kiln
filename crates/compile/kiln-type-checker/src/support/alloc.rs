//! Allocation support used by the imported checker.
//!
//! This follows Bun's allocator contract: allocations are individually
//! reclaimable, vectors can grow without leaking their previous buffers, and
//! a value may be dropped on a different thread from the one that allocated
//! it. Each arena owns a mimalloc heap and bulk-reclaims any remaining blocks
//! when the arena is dropped.
//!
//! The per-heap ownership, `mi_free` cross-thread behavior, and transfer
//! strategy are adapted from Bun's `src/bun_alloc` implementation
//! (MIT-licensed; pinned source commit `bbdc5a519e0a06d1b3133b564f91096b9ba10a31`).

use std::alloc::{AllocError, Allocator, Layout};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// Per-session mimalloc heap. Allocation and resize are restricted to the
/// creating thread; deallocation is safe from any thread. This mirrors the
/// source allocator's contract while allowing arenas to cross worker queues.
pub struct Arena {
    heap: NonNull<libmimalloc_sys::mi_heap_t>,
    owner: std::thread::ThreadId,
}

unsafe impl Send for Arena {}
unsafe impl Sync for Arena {}

impl Default for Arena {
    fn default() -> Self {
        Self::new()
    }
}

/// `mi_option_allow_thp` in mimalloc 3.
const MI_OPTION_ALLOW_THP: libmimalloc_sys::mi_option_t = 43;

impl Arena {
    pub fn new() -> Self {
        // Every check thread starts its own heap on fresh memory. With transparent huge pages,
        // mimalloc advises each new region huge and the first touch of a page zeroes 2 MiB, which
        // made a 16-thread load slower than a serial one. Only that advice is withdrawn; the
        // process's THP setting is untouched.
        static NO_HUGE_PAGE_ADVICE: std::sync::Once = std::sync::Once::new();
        NO_HUGE_PAGE_ADVICE.call_once(|| {
            // SAFETY: setting a mimalloc option has no preconditions.
            unsafe { libmimalloc_sys::mi_option_set(MI_OPTION_ALLOW_THP, 0) }
        });
        // SAFETY: mimalloc heap creation has no preconditions.
        let heap = unsafe { libmimalloc_sys::mi_heap_new() };
        Self {
            heap: NonNull::new(heap).expect("mimalloc heap allocation failed"),
            owner: std::thread::current().id(),
        }
    }

    #[inline]
    fn assert_owner(&self) {
        assert_eq!(
            self.owner,
            std::thread::current().id(),
            "Arena allocation/resizing must stay on its owner thread"
        );
    }

    #[inline]
    fn zero_size_ptr(layout: Layout) -> NonNull<u8> {
        // The Allocator contract requires a non-null pointer with the
        // requested alignment, even when the requested size is zero.
        unsafe { NonNull::new_unchecked(layout.align() as *mut u8) }
    }

    pub fn alloc<T>(&self, value: T) -> &mut T {
        Box::leak(Box::new_in(value, self))
    }

    pub fn alloc_slice_copy<T: Copy>(&self, values: &[T]) -> &mut [T] {
        self.alloc_slice_fill_iter(values.iter().copied())
    }

    pub fn alloc_slice_fill_copy<T: Copy>(&self, len: usize, value: T) -> &mut [T] {
        self.alloc_slice_fill_iter(std::iter::repeat(value).take(len))
    }

    pub fn alloc_slice_fill_iter<T>(&self, values: impl IntoIterator<Item = T>) -> &mut [T] {
        Box::leak(vec_from_iter_in(values, self).into_boxed_slice())
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        // Lifetimes prevent live ArenaBox/Vec allocations from outliving the heap.
        // SAFETY: this heap was created by mi_heap_new and is uniquely owned.
        unsafe { libmimalloc_sys::mi_heap_destroy(self.heap.as_ptr()) }
    }
}

unsafe impl Allocator for &Arena {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.assert_owner();
        if layout.size() == 0 {
            return Ok(NonNull::slice_from_raw_parts(
                Arena::zero_size_ptr(layout),
                0,
            ));
        }
        // SAFETY: valid heap and layout; null is converted to AllocError.
        let ptr = unsafe {
            libmimalloc_sys::mi_heap_malloc_aligned(
                self.heap.as_ptr(),
                layout.size(),
                layout.align(),
            )
        };
        let ptr = NonNull::new(ptr.cast::<u8>()).ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.assert_owner();
        if layout.size() == 0 {
            return Ok(NonNull::slice_from_raw_parts(
                Arena::zero_size_ptr(layout),
                0,
            ));
        }
        // SAFETY: valid heap and layout; null is converted to AllocError.
        let ptr = unsafe {
            libmimalloc_sys::mi_heap_zalloc_aligned(
                self.heap.as_ptr(),
                layout.size(),
                layout.align(),
            )
        };
        let ptr = NonNull::new(ptr.cast::<u8>()).ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() == 0 {
            return;
        }
        // SAFETY: mi_free is thread-safe and finds the owning heap from ptr.
        unsafe { libmimalloc_sys::mi_free(ptr.as_ptr().cast()) }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.assert_owner();
        if new_layout.size() == 0 {
            return Ok(NonNull::slice_from_raw_parts(
                Arena::zero_size_ptr(new_layout),
                0,
            ));
        }
        if old_layout.size() == 0 {
            return self.allocate(new_layout);
        }
        // SAFETY: ptr is this allocator's live block; mimalloc preserves it on failure.
        let out = unsafe {
            libmimalloc_sys::mi_heap_realloc_aligned(
                self.heap.as_ptr(),
                ptr.as_ptr().cast(),
                new_layout.size(),
                new_layout.align(),
            )
        };
        let out = NonNull::new(out.cast::<u8>()).ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(out, new_layout.size()))
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let out = unsafe { self.grow(ptr, old_layout, new_layout)? };
        // SAFETY: newly grown tail is initialized here; the old prefix remains intact.
        unsafe {
            std::ptr::write_bytes(
                out.as_ptr().cast::<u8>().add(old_layout.size()),
                0,
                new_layout.size() - old_layout.size(),
            );
        }
        Ok(out)
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.assert_owner();
        if new_layout.size() == 0 {
            if old_layout.size() != 0 {
                unsafe {
                    libmimalloc_sys::mi_free(ptr.as_ptr().cast());
                }
            }
            return Ok(NonNull::slice_from_raw_parts(
                Arena::zero_size_ptr(new_layout),
                0,
            ));
        }
        let out = unsafe {
            libmimalloc_sys::mi_heap_realloc_aligned(
                self.heap.as_ptr(),
                ptr.as_ptr().cast(),
                new_layout.size(),
                new_layout.align(),
            )
        };
        let out = NonNull::new(out.cast::<u8>()).ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(out, new_layout.size()))
    }
}

/// Boxed checker data allocated in an arena lifetime. Keeping only the data
/// pointer preserves Box's compact thin/fat pointer representation; the
/// arena lifetime is carried without storing its allocator handle.
pub struct ArenaBox<'a, T: ?Sized> {
    ptr: NonNull<T>,
    _arena: PhantomData<&'a Arena>,
    _value: PhantomData<T>,
}

const _: () = assert!(std::mem::size_of::<ArenaBox<'static, u64>>() == 8);
const _: () = assert!(std::mem::size_of::<ArenaBox<'static, [u64]>>() == 16);

impl<'a, T> ArenaBox<'a, T> {
    pub fn new_in(value: T, arena: &'a Arena) -> Self {
        Self::from_box(Box::new_in(value, arena))
    }
}

impl<'a, T: ?Sized> ArenaBox<'a, T> {
    pub fn from_box(value: Box<T, &'a Arena>) -> Self {
        let (ptr, _allocator) = Box::into_raw_with_allocator(value);
        // SAFETY: Box guarantees the pointer is non-null and properly aligned.
        Self {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
            _arena: PhantomData,
            _value: PhantomData,
        }
    }
}

impl<'a, T> ArenaBox<'a, [T]> {
    pub fn copy_from_slice_in(items: &[T], arena: &'a Arena) -> Self
    where
        T: Copy,
    {
        Self::from_box(vec_from_iter_in(items.iter().copied(), arena).into_boxed_slice())
    }

    pub fn from_iter_in(items: impl IntoIterator<Item = T>, arena: &'a Arena) -> Self {
        Self::from_box(vec_from_iter_in(items, arena).into_boxed_slice())
    }
}

impl<T> Default for ArenaBox<'_, [T]> {
    fn default() -> Self {
        // SAFETY: dangling is non-null and aligned for T; a zero-length slice
        // never dereferences the pointer, and Drop skips freeing zero bytes.
        let raw = std::ptr::slice_from_raw_parts_mut(NonNull::<T>::dangling().as_ptr(), 0);
        Self {
            ptr: unsafe { NonNull::new_unchecked(raw) },
            _arena: PhantomData,
            _value: PhantomData,
        }
    }
}

impl<'a, T> From<Vec<T, &'a Arena>> for ArenaBox<'a, [T]> {
    fn from(value: Vec<T, &'a Arena>) -> Self {
        Self::from_box(value.into_boxed_slice())
    }
}

impl<T: ?Sized> Deref for ArenaBox<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: ArenaBox owns a valid T until Drop.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for ArenaBox<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: `&mut self` provides unique access to the owned value.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: ?Sized> Drop for ArenaBox<'_, T> {
    fn drop(&mut self) {
        let allocated_bytes = std::mem::size_of_val(unsafe { self.ptr.as_ref() });
        // SAFETY: the pointer came from Box and this is its unique owner.
        unsafe {
            std::ptr::drop_in_place(self.ptr.as_ptr());
        }
        // Zero-sized values have an aligned dangling pointer and no allocation.
        if allocated_bytes != 0 {
            unsafe {
                libmimalloc_sys::mi_free(self.ptr.as_ptr().cast());
            }
        }
    }
}

// SAFETY: ownership and access are transferred only when T permits it; free
// uses mimalloc's thread-safe mi_free regardless of allocating arena/thread.
unsafe impl<T: ?Sized + Send> Send for ArenaBox<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for ArenaBox<'_, T> {}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for ArenaBox<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized + PartialEq> PartialEq for ArenaBox<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}
impl<T: ?Sized + Eq> Eq for ArenaBox<'_, T> {}
impl<T: ?Sized + std::hash::Hash> std::hash::Hash for ArenaBox<'_, T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&**self, state);
    }
}
impl<T: ?Sized> AsRef<T> for ArenaBox<'_, T> {
    fn as_ref(&self) -> &T {
        self
    }
}
impl<T: ?Sized> std::borrow::Borrow<T> for ArenaBox<'_, T> {
    fn borrow(&self) -> &T {
        self
    }
}
impl<'a, T> IntoIterator for &'a ArenaBox<'_, [T]> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> ArenaBox<'a, [T]> {
    pub fn empty() -> Self {
        Self::default()
    }
}

pub type ArenaVec<'a, T> = Vec<T, &'a Arena>;

pub trait ArenaVecExt<'a, T> {
    fn allocator(&self) -> &'a Arena;
}

impl<'a, T> ArenaVecExt<'a, T> for ArenaVec<'a, T> {
    fn allocator(&self) -> &'a Arena {
        // Vec exposes its allocator through the allocator_api feature.
        *Vec::allocator(self)
    }
}

pub fn vec_from_iter_in<'a, T, I>(items: I, arena: &'a Arena) -> ArenaVec<'a, T>
where
    I: IntoIterator<Item = T>,
{
    let iter = items.into_iter();
    let (lower, _) = iter.size_hint();
    let mut vec = Vec::with_capacity_in(lower, arena);
    vec.extend(iter);
    vec
}

/// Move a Vec's elements to storage owned by the destination arena, then
/// release the source buffer through its original allocator.
pub fn transfer_arena<'a, T>(vec: &mut ArenaVec<'a, T>, dst: &'a Arena) {
    let mut moved = Vec::with_capacity_in(vec.len(), dst);
    moved.append(vec);
    *vec = moved;
}

pub fn map_in<'a, K, V>(_arena: &'a Arena) -> std::collections::HashMap<K, V> {
    std::collections::HashMap::new()
}

pub fn set_in<'a, T>(_arena: &'a Arena) -> std::collections::HashSet<T> {
    std::collections::HashSet::new()
}
