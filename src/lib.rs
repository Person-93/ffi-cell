use std::{
  fmt::Display,
  marker::PhantomData,
  ops::{Deref, DerefMut},
  ptr::{NonNull, null_mut},
  sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use derive_more::{Display, Error, From};

#[cfg(test)]
mod test;

pub struct FfiCell<T: Sync> {
  ptr: AtomicPtr<T>,
  in_use: AtomicBool,
}

impl<T: Sync> FfiCell<T> {
  pub const fn new() -> Self {
    Self {
      ptr: AtomicPtr::new(null_mut()),
      in_use: AtomicBool::new(false),
    }
  }

  #[track_caller]
  pub fn run<R>(&self, object: &mut T, f: impl FnOnce() -> R) -> R {
    self.try_run(object, f).unwrap_or_display_err()
  }

  pub fn try_run<R>(
    &self,
    object: &mut T,
    f: impl FnOnce() -> R,
  ) -> Result<R, Error> {
    unsafe {
      self.try_lend(object)?;
    }
    let _reclaim = ScopeGuard::new(|| self.reclaim());
    Ok(f())
  }

  /// # Safety
  /// The object pointed to in the params cannot be referenced until
  /// `reclaim` is called without panicking or `try_reclaim` is called and
  /// returns `Ok`.
  #[track_caller]
  pub unsafe fn lend(&self, ptr: &mut T) {
    unsafe { self.try_lend(ptr).unwrap_or_display_err() }
  }

  /// # Safety
  /// The object pointed to in the params cannot be referenced until
  /// `reclaim` is called without panicking or `try_reclaim` is called and
  /// returns `Ok`.
  pub unsafe fn try_lend(&self, ptr: &mut T) -> Result<(), LendError> {
    // This check does not satisfy the safety requirement.
    // It is here to provide a better error message.
    if self.in_use.load(Ordering::SeqCst) {
      return Err(LendError::AlreadyLent);
    }

    match self.ptr.compare_exchange(
      null_mut(),
      ptr,
      Ordering::SeqCst,
      Ordering::SeqCst,
    ) {
      Ok(_) => Ok(()),
      Err(_) => Err(LendError::AlreadyHasLoan),
    }
  }

  #[track_caller]
  pub fn borrow(&self) -> impl DerefMut<Target = T> {
    self.try_borrow().unwrap_or_display_err()
  }

  pub fn try_borrow(&self) -> Result<impl DerefMut<Target = T>, BorrowError> {
    if self.in_use.swap(true, Ordering::SeqCst) {
      Err(BorrowError::AlreadyBorrowed)
    } else {
      let ptr = self.ptr.swap(null_mut(), Ordering::SeqCst);
      match NonNull::new(ptr) {
        Some(ptr) => Ok(FfiGuard {
          ptr,
          cell: self,
          _marker: PhantomData,
        }),
        None => Err(BorrowError::Unavailable),
      }
    }
  }

  #[track_caller]
  pub fn reclaim(&self) {
    self.try_reclaim().unwrap_or_display_err()
  }

  pub fn try_reclaim(&self) -> Result<(), ReclaimError> {
    if self.in_use.load(Ordering::SeqCst) {
      Err(ReclaimError::InUse)
    } else if self.ptr.swap(null_mut(), Ordering::SeqCst).is_null() {
      unreachable!("missing pointer when not in use")
    } else {
      Ok(())
    }
  }
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

impl<'g, T: Sync> Deref for FfiGuard<'g, T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    unsafe { self.ptr.as_ref() }
  }
}

impl<'g, T: Sync> DerefMut for FfiGuard<'g, T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    unsafe { self.ptr.as_mut() }
  }
}

impl<'g, T: Sync> Drop for FfiGuard<'g, T> {
  fn drop(&mut self) {
    self
      .cell
      .ptr
      .compare_exchange(
        null_mut(),
        self.ptr.as_ptr(),
        Ordering::SeqCst,
        Ordering::SeqCst,
      )
      .expect("tried to return lent pointer, but another pointer was there");
    let was_in_use = self.cell.in_use.swap(false, Ordering::SeqCst);
    assert!(was_in_use, "object was not in use when it was returned");
  }
}

#[non_exhaustive]
#[derive(Debug, Display, Error, From)]
pub enum Error {
  LendError(LendError),
  BorrowError(BorrowError),
}

#[non_exhaustive]
#[derive(Debug, Display, Error)]
#[display("cannot lend value to ffi-cell because {_variant}")]
pub enum LendError {
  #[display("it currently has one and it is already lent out")]
  AlreadyLent,
  #[display("it already has one")]
  AlreadyHasLoan,
}

#[non_exhaustive]
#[derive(Debug, Display, Error)]
#[display("cannot borrow value from ffi-cell because {_variant}")]
pub enum BorrowError {
  #[display("the cell does not have a value")]
  Unavailable,
  #[display("the cell's value is already lent out")]
  AlreadyBorrowed,
}

#[non_exhaustive]
#[derive(Debug, Display, Error)]
#[display("cannot reclaim value from ffi-cell because {_variant}")]
pub enum ReclaimError {
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
    (self.0)()
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
