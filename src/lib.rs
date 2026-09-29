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

const IN_USE_SENTINEL: *mut () = 1 as _;

pub struct FfiCell<T: Sync> {
  ptr: AtomicPtr<T>,
}

impl<T: Sync> FfiCell<T> {
  pub const fn new() -> Self {
    Self { ptr: AtomicPtr::new(null_mut()) }
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

  #[track_caller]
  pub fn borrow(&self) -> impl DerefMut<Target = T> {
    self.try_borrow().unwrap_or_display_err()
  }

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
        };

        Err(BorrowError::Unavailable)
      },
    }
  }

  #[track_caller]
  pub fn reclaim(&self) {
    self.try_reclaim().unwrap_or_display_err()
  }

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
        IN_USE_SENTINEL.cast(),
        self.ptr.as_ptr(),
        Ordering::SeqCst,
        Ordering::SeqCst,
      )
      .expect("tried to return lent pointer, but another pointer was there");
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
