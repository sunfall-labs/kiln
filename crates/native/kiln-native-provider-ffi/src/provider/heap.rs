use core::alloc::{GlobalAlloc, Layout};

unsafe extern "C" {
    fn malloc(size: usize) -> *mut core::ffi::c_void;
    fn free(pointer: *mut core::ffi::c_void);
    fn posix_memalign(
        out: *mut *mut core::ffi::c_void,
        alignment: usize,
        size: usize,
    ) -> core::ffi::c_int;
}

/// What `malloc` already guarantees on every target this compiles for.
const MALLOC_ALIGNMENT: usize = 16;

struct CHeap;

// SAFETY: every allocation is returned by `malloc` or `posix_memalign` and freed by `free`,
// which is the contract both halves of libc's allocator keep.
unsafe impl GlobalAlloc for CHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MALLOC_ALIGNMENT {
            // SAFETY: a nonzero size is guaranteed by the `Layout` contract.
            return unsafe { malloc(layout.size()) }.cast::<u8>();
        }
        let mut out = core::ptr::null_mut();
        // SAFETY: `out` is a valid slot for the returned pointer.
        let status = unsafe { posix_memalign(&raw mut out, layout.align(), layout.size()) };
        if status == 0 {
            out.cast::<u8>()
        } else {
            core::ptr::null_mut()
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        // SAFETY: the pointer came from `alloc` above and is freed exactly once.
        unsafe { free(pointer.cast::<core::ffi::c_void>()) };
    }
}

#[global_allocator]
static HEAP: CHeap = CHeap;
