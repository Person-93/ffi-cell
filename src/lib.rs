#![doc = include_str!("../README.md")]
#![no_std]

use core::{
  fmt::Display,
  marker::PhantomData,
  ops::{Deref, DerefMut},
  ptr::{NonNull, null_mut},
  sync::atomic::{AtomicPtr, Ordering},
};

use derive_more::{Display, Error, From};

#[cfg(test)]
mod test;

#[expect(clippy::as_conversions, reason = "sentinel value for ptr")]
const IN_USE_SENTINEL: *mut () = 1 as _;

/// Holds data that can be lent across FFI boundaries. See crate level docs for
/// details.
pub struct FfiCell<T: Sync> {
  ptr: AtomicPtr<T>,
}

impl<T: Sync> FfiCell<T> {
  /// Create a new [`FfiCell`]
  pub const fn new() -> Self {
    Self { ptr: AtomicPtr::new(null_mut()) }
  }

  /// Run the function with `object` available to be borrowed.
  ///
  /// See [`Self::try_run`] for the non-panicking version.
  ///
  /// # Panic
  /// Panics if this cell is already borrowing an object.
  #[track_caller]
  pub fn run<R>(&self, object: &mut T, f: impl FnOnce() -> R) -> R {
    self.try_run(object, f).unwrap_or_display_err()
  }

  /// Run the function with `object` available to be borrowed.
  ///
  /// See [`Self::run`] for the panicking version.
  ///
  /// # Error
  /// Returns and error if this cell is already borrowing an object.
  pub fn try_run<R>(
    &self,
    object: &mut T,
    f: impl FnOnce() -> R,
  ) -> Result<R, Error> {
    // SAFETY: reclaim is called by the ScopeGuard
    unsafe { self.try_lend(object)? };
    let _reclaim = ScopeGuard::new(|| self.reclaim());
    Ok(f())
  }

  /// Lend an object to the cell.
  ///
  /// See [`Self::run`] for the safe API.
  ///
  /// See [`Self::try_lend`] for the non-panicking version.
  ///
  /// # Panic
  /// Panics if this cell is already borrowing an object.
  ///
  /// # Safety
  /// The object pointed to in the params cannot be referenced until
  /// `reclaim` is called without panicking or `try_reclaim` is called and
  /// returns `Ok`.
  #[track_caller]
  pub unsafe fn lend(&self, ptr: &mut T) {
    // SAFETY: caller upholds guarantees
    unsafe { self.try_lend(ptr) }.unwrap_or_display_err();
  }

  /// Lend an object to the cell.
  ///
  /// See [`Self::try_run`] for the safe API.
  ///
  /// See [`Self::lend`] for the panicking version.
  ///
  /// # Error
  /// Panics if this cell is already borrowing an object.
  ///
  /// # Safety
  /// The object pointed to in the params cannot be referenced until
  /// `reclaim` is called without panicking or `try_reclaim` is called and
  /// returns `Ok`.
  pub unsafe fn try_lend(&self, ptr: &mut T) -> Result<(), LendError> {
    loop {
      return match self.ptr.compare_exchange(
        null_mut(),
        ptr,
        Ordering::SeqCst,
        Ordering::SeqCst,
      ) {
        Ok(_) => Ok(()),
        Err(ptr) => match ptr_from_raw(ptr) {
          Ok(_) => Err(LendError::AlreadyHasLoan),
          Err(RawPtrErr::InUse) => Err(LendError::AlreadyLent),
          Err(RawPtrErr::Null) => continue,
        },
      };
    }
  }

  /// Re-borrow the object that the cell is currently borrowing.
  ///
  /// See [`Self::try_borrow`] for the non-panicking version.
  ///
  /// # Panic
  /// Panics if the cell is not currently borrowing anything or if its borrowed
  /// object is currently reborrowed.
  #[track_caller]
  pub fn borrow(&self) -> impl DerefMut<Target = T> {
    self.try_borrow().unwrap_or_display_err()
  }

  /// Re-borrow the object that the cell is currently borrowing.
  ///
  /// See [`Self::borrow`] for the panicking version.
  ///
  /// # Error
  /// Returns an error if the cell is not currently borrowing anything or if
  /// its borrowed object is currently reborrowed.
  pub fn try_borrow(&self) -> Result<impl DerefMut<Target = T>, BorrowError> {
    let ptr = self.ptr.swap(IN_USE_SENTINEL.cast(), Ordering::SeqCst);
    match ptr_from_raw(ptr) {
      Ok(ptr) => Ok(FfiGuard {
        ptr,
        cell: self,
        _marker: PhantomData,
      }),
      Err(RawPtrErr::InUse) => Err(BorrowError::AlreadyBorrowed),
      Err(RawPtrErr::Null) => {
        // put the null ptr back in
        if let Err(err) = self.ptr.compare_exchange(
          IN_USE_SENTINEL.cast(),
          null_mut(),
          Ordering::SeqCst,
          Ordering::SeqCst,
        ) {
          unreachable!("unexpected pointer: {err:p}")
        }

        Err(BorrowError::Unavailable)
      },
    }
  }

  /// Reclaim the object that was lent to this cell.
  ///
  /// This function should only be called when using the unsafe API.
  ///
  /// See [`Self::try_reclaim`] for the non-panicking version.
  ///
  /// # Panic
  /// Panics if the borrowed object is currently re-borrowed.
  #[track_caller]
  pub fn reclaim(&self) {
    self.try_reclaim().unwrap_or_display_err();
  }

  /// Reclaim the object that was lent to this cell.
  ///
  /// This function should only be called when using the unsafe API.
  ///
  /// See [`Self::reclaim`] for the panicking version.
  ///
  /// # Errors
  /// Returns an error if the borrowed object is currently re-borrowed.
  pub fn try_reclaim(&self) -> Result<(), ReclaimError> {
    let ptr = self.ptr.swap(null_mut(), Ordering::SeqCst);

    match ptr_from_raw(ptr) {
      Ok(_) => Ok(()),
      Err(RawPtrErr::InUse) => Err(ReclaimError::InUse),
      Err(RawPtrErr::Null) => unreachable!("missing pointer when not in use"),
    }
  }
}

fn ptr_from_raw<T>(ptr: *mut T) -> Result<NonNull<T>, RawPtrErr> {
  match NonNull::new(ptr) {
    Some(ptr) if ptr.as_ptr().cast() == IN_USE_SENTINEL => {
      Err(RawPtrErr::InUse)
    },
    Some(ptr) => Ok(ptr),
    None => Err(RawPtrErr::Null),
  }
}

enum RawPtrErr {
  Null,
  InUse,
}

impl<T: Sync> Default for FfiCell<T> {
  fn default() -> Self {
    Self::new()
  }
}

struct FfiGuard<'g, T: Sync> {
  ptr: NonNull<T>,
  cell: &'g FfiCell<T>,
  _marker: PhantomData<&'g ()>,
}

impl<T: Sync> Deref for FfiGuard<'_, T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    // SAFETY: this guard has the only pointer
    unsafe { self.ptr.as_ref() }
  }
}

impl<T: Sync> DerefMut for FfiGuard<'_, T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    // SAFETY: this guard has the only pointer
    unsafe { self.ptr.as_mut() }
  }
}

impl<T: Sync> Drop for FfiGuard<'_, T> {
  fn drop(&mut self) {
    self
      .cell
      .ptr
      .compare_exchange(
        IN_USE_SENTINEL.cast(),
        self.ptr.as_ptr(),
        Ordering::SeqCst,
        Ordering::SeqCst,
      )
      .expect("tried to return lent pointer, but another pointer was there");
  }
}

/// All errors that can occur in this library. Returned by [`FfiCell::try_run`].
#[non_exhaustive]
#[derive(Debug, Display, Error, From)]
pub enum Error {
  /// An error occurred trying to lend an object to a cell
  LendError(LendError),

  /// An error occurred trying to re-borrow an object from a cell
  BorrowError(BorrowError),
}

/// Errors that can occur trying to lend an object to a cell
#[non_exhaustive]
#[derive(Debug, Display, Error)]
#[display("cannot lend value to ffi-cell because {_variant}")]
pub enum LendError {
  /// The cell is already borrowing an object and it's re-lending it
  #[display("it currently has one and it is already lent out")]
  AlreadyLent,

  /// The cell is already borrowing an object
  #[display("it already has one")]
  AlreadyHasLoan,
}

/// Errors that can occur trying to borrow an object from a cell
#[non_exhaustive]
#[derive(Debug, Display, Error)]
#[display("cannot borrow value from ffi-cell because {_variant}")]
pub enum BorrowError {
  /// The cell is not currently borrowing an object
  #[display("the cell does not have a value")]
  Unavailable,

  /// The cell's object is already re-borrowed
  #[display("the cell's value is already lent out")]
  AlreadyBorrowed,
}

/// Errors that can occur trying to reclaim an object from a cell
#[non_exhaustive]
#[derive(Debug, Display, Error)]
#[display("cannot reclaim value from ffi-cell because {_variant}")]
pub enum ReclaimError {
  /// The object is currently re-borrowed
  #[display("it is currently in use")]
  InUse,
}

struct ScopeGuard<F: FnMut()>(F);

impl<F: FnMut()> ScopeGuard<F> {
  fn new(f: F) -> Self {
    Self(f)
  }
}

impl<F: FnMut()> Drop for ScopeGuard<F> {
  fn drop(&mut self) {
    (self.0)();
  }
}

trait ResultExt<T> {
  #[track_caller]
  fn unwrap_or_display_err(self) -> T;
}

impl<T, E: Display> ResultExt<T> for Result<T, E> {
  fn unwrap_or_display_err(self) -> T {
    match self {
      Ok(val) => val,
      Err(err) => panic!("{err}"),
    }
  }
}
