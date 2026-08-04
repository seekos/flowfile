//! API-compatible Rust interface for Apple's C language extension of blocks.
//!
//! This local copy preserves the public API of `block` 0.1.6. Its opaque
//! Objective-C `Class` uses an inhabited FFI representation so modern Rust
//! compilers do not reject `_NSConcreteStackBlock` as an uninhabited static.

use std::marker::PhantomData;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::os::raw::{c_int, c_ulong, c_void};
use std::ptr;

#[repr(C)]
struct Class {
    _private: [u8; 0],
}

#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    link(name = "System", kind = "dylib")
)]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios")),
    link(name = "BlocksRuntime", kind = "dylib")
)]
unsafe extern "C" {
    static _NSConcreteStackBlock: Class;

    fn _Block_copy(block: *const c_void) -> *mut c_void;
    fn _Block_release(block: *const c_void);
}

/// Types that may be used as the arguments to an Objective-C block.
pub trait BlockArguments: Sized {
    /// Calls the given block with these arguments.
    ///
    /// # Safety
    ///
    /// `block` must point to a valid Objective-C block whose signature matches
    /// the argument and return types.
    unsafe fn call_block<R>(self, block: *mut Block<Self, R>) -> R;
}

macro_rules! block_args_impl {
    ($($argument:ident : $argument_type:ident),*) => {
        impl<$($argument_type),*> BlockArguments for ($($argument_type,)*) {
            unsafe fn call_block<R>(self, block: *mut Block<Self, R>) -> R {
                let invoke: unsafe extern "C" fn(
                    *mut Block<Self, R>
                    $(, $argument_type)*
                ) -> R = {
                    let base = block as *mut BlockBase<Self, R>;
                    unsafe { mem::transmute((*base).invoke) }
                };
                let ($($argument,)*) = self;
                unsafe { invoke(block $(, $argument)*) }
            }
        }
    };
}

block_args_impl!();
block_args_impl!(a: A);
block_args_impl!(a: A, b: B);
block_args_impl!(a: A, b: B, c: C);
block_args_impl!(a: A, b: B, c: C, d: D);
block_args_impl!(a: A, b: B, c: C, d: D, e: E);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K);
block_args_impl!(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K, l: L);

#[repr(C)]
struct BlockBase<A, R> {
    isa: *const Class,
    flags: c_int,
    reserved: c_int,
    invoke: unsafe extern "C" fn(*mut Block<A, R>, ...) -> R,
}

/// An Objective-C block that accepts arguments `A` and returns `R`.
#[repr(C)]
pub struct Block<A, R> {
    base: PhantomData<BlockBase<A, R>>,
}

impl<A: BlockArguments, R> Block<A, R> {
    /// Invokes this Objective-C block.
    ///
    /// # Safety
    ///
    /// The block must be valid and its captured state must be safe to access.
    pub unsafe fn call(&self, arguments: A) -> R {
        unsafe { arguments.call_block(self as *const _ as *mut _) }
    }
}

/// A reference-counted Objective-C block.
pub struct RcBlock<A, R> {
    ptr: *mut Block<A, R>,
}

impl<A, R> RcBlock<A, R> {
    /// Wraps a block pointer without copying it.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid block with a +1 reference count.
    pub unsafe fn new(ptr: *mut Block<A, R>) -> Self {
        Self { ptr }
    }

    /// Copies a block through the Objective-C Blocks runtime.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid block.
    pub unsafe fn copy(ptr: *mut Block<A, R>) -> Self {
        let ptr = unsafe { _Block_copy(ptr.cast()) }.cast();
        Self { ptr }
    }
}

impl<A, R> Clone for RcBlock<A, R> {
    fn clone(&self) -> Self {
        unsafe { Self::copy(self.ptr) }
    }
}

impl<A, R> Deref for RcBlock<A, R> {
    type Target = Block<A, R>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.ptr }
    }
}

impl<A, R> Drop for RcBlock<A, R> {
    fn drop(&mut self) {
        unsafe { _Block_release(self.ptr.cast()) };
    }
}

/// Types that may be converted into a [`ConcreteBlock`].
pub trait IntoConcreteBlock<A>: Sized
where
    A: BlockArguments,
{
    /// The return type of the resulting block.
    type Ret;

    /// Consumes the closure and creates a stack block.
    fn into_concrete_block(self) -> ConcreteBlock<A, Self::Ret, Self>;
}

macro_rules! concrete_block_impl {
    ($invoke:ident) => {
        concrete_block_impl!($invoke,);
    };
    ($invoke:ident, $($argument:ident : $argument_type:ident),*) => {
        impl<$($argument_type,)* R, X> IntoConcreteBlock<($($argument_type,)*)> for X
        where
            X: Fn($($argument_type,)*) -> R,
        {
            type Ret = R;

            fn into_concrete_block(self) -> ConcreteBlock<($($argument_type,)*), R, X> {
                unsafe extern "C" fn $invoke<$($argument_type,)* R, X>(
                    block_ptr: *mut ConcreteBlock<($($argument_type,)*), R, X>
                    $(, $argument: $argument_type)*
                ) -> R
                where
                    X: Fn($($argument_type,)*) -> R,
                {
                    let block = unsafe { &*block_ptr };
                    (block.closure)($($argument),*)
                }

                let invoke: unsafe extern "C" fn(
                    *mut ConcreteBlock<($($argument_type,)*), R, X>
                    $(, $argument_type)*
                ) -> R = $invoke;
                unsafe { ConcreteBlock::with_invoke(mem::transmute(invoke), self) }
            }
        }
    };
}

concrete_block_impl!(concrete_block_invoke_args0);
concrete_block_impl!(concrete_block_invoke_args1, a: A);
concrete_block_impl!(concrete_block_invoke_args2, a: A, b: B);
concrete_block_impl!(concrete_block_invoke_args3, a: A, b: B, c: C);
concrete_block_impl!(concrete_block_invoke_args4, a: A, b: B, c: C, d: D);
concrete_block_impl!(concrete_block_invoke_args5, a: A, b: B, c: C, d: D, e: E);
concrete_block_impl!(concrete_block_invoke_args6, a: A, b: B, c: C, d: D, e: E, f: F);
concrete_block_impl!(concrete_block_invoke_args7, a: A, b: B, c: C, d: D, e: E, f: F, g: G);
concrete_block_impl!(concrete_block_invoke_args8, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H);
concrete_block_impl!(concrete_block_invoke_args9, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I);
concrete_block_impl!(concrete_block_invoke_args10, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J);
concrete_block_impl!(concrete_block_invoke_args11, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K);
concrete_block_impl!(concrete_block_invoke_args12, a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I, j: J, k: K, l: L);

/// A stack-allocated Objective-C block with a Rust closure payload.
#[repr(C)]
pub struct ConcreteBlock<A, R, F> {
    base: BlockBase<A, R>,
    descriptor: Box<BlockDescriptor<ConcreteBlock<A, R, F>>>,
    closure: F,
}

impl<A, R, F> ConcreteBlock<A, R, F>
where
    A: BlockArguments,
    F: IntoConcreteBlock<A, Ret = R>,
{
    /// Creates a concrete block from a closure.
    pub fn new(closure: F) -> Self {
        closure.into_concrete_block()
    }
}

impl<A, R, F> ConcreteBlock<A, R, F> {
    unsafe fn with_invoke(
        invoke: unsafe extern "C" fn(*mut Self, ...) -> R,
        closure: F,
    ) -> Self {
        Self {
            base: BlockBase {
                isa: unsafe { &_NSConcreteStackBlock },
                flags: 1 << 25,
                reserved: 0,
                invoke: unsafe { mem::transmute(invoke) },
            },
            descriptor: Box::new(BlockDescriptor::new()),
            closure,
        }
    }
}

impl<A, R, F: 'static> ConcreteBlock<A, R, F> {
    /// Copies this stack block to the heap.
    pub fn copy(self) -> RcBlock<A, R> {
        unsafe {
            let mut block = self;
            let copied = RcBlock::copy(&mut *block);
            mem::forget(block);
            copied
        }
    }
}

impl<A, R, F: Clone> Clone for ConcreteBlock<A, R, F> {
    fn clone(&self) -> Self {
        unsafe { Self::with_invoke(mem::transmute(self.base.invoke), self.closure.clone()) }
    }
}

impl<A, R, F> Deref for ConcreteBlock<A, R, F> {
    type Target = Block<A, R>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(&self.base as *const _ as *const Block<A, R>) }
    }
}

impl<A, R, F> DerefMut for ConcreteBlock<A, R, F> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *(&mut self.base as *mut _ as *mut Block<A, R>) }
    }
}

unsafe extern "C" fn block_context_dispose<B>(block: &mut B) {
    unsafe { ptr::read(block) };
}

unsafe extern "C" fn block_context_copy<B>(_destination: &mut B, _source: &B) {}

#[repr(C)]
struct BlockDescriptor<B> {
    reserved: c_ulong,
    block_size: c_ulong,
    copy_helper: unsafe extern "C" fn(&mut B, &B),
    dispose_helper: unsafe extern "C" fn(&mut B),
}

impl<B> BlockDescriptor<B> {
    fn new() -> Self {
        Self {
            reserved: 0,
            block_size: mem::size_of::<B>() as c_ulong,
            copy_helper: block_context_copy::<B>,
            dispose_helper: block_context_dispose::<B>,
        }
    }
}
