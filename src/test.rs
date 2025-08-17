use super::*;

#[test]
fn test() {
  let cell = FfiCell::<i32>::default();
  assert!(
    cell.ptr.load(Ordering::SeqCst).is_null(),
    "new cell should have null pointer"
  );
  assert!(
    !cell.in_use.load(Ordering::SeqCst),
    "new cell should not be in use"
  );

  let mut value = 42;
  let value_ptr: *const i32 = &value;

  unsafe {
    cell.lend(&mut value);
  }
  let ptr: *const _ = cell.ptr.load(Ordering::SeqCst);
  assert!(!ptr.is_null(), "after loan, pointer should not be null");
  assert_eq!(ptr, value_ptr, "value in cell should match lent value");
  assert!(
    !cell.in_use.load(Ordering::SeqCst),
    "cell should not be in use until borrowed"
  );

  let num = cell.borrow();
  let num_ptr: *const i32 = &*num;
  assert!(
    cell.in_use.load(Ordering::SeqCst),
    "cell should be in use after borrowed"
  );
  assert_eq!(
    num_ptr, value_ptr,
    "borrowed value should match stored value"
  );
  assert!(
    cell.ptr.load(Ordering::SeqCst).is_null(),
    "cell should have null pointer while guard exists"
  );

  drop(num);
  let ptr: *const _ = cell.ptr.load(Ordering::SeqCst);
  assert!(
    !ptr.is_null(),
    "cell should not have null pointer after guard is dropped"
  );
  assert_eq!(
    ptr, value_ptr,
    "cell's pointer after guard is dropped should match the original pointer"
  );

  cell.reclaim();
  assert!(
    cell.ptr.load(Ordering::SeqCst).is_null(),
    "cell should have null pointer after value is reclaimed"
  );

  cell.run(&mut value, || {
    assert!(
      !cell.ptr.load(Ordering::SeqCst).is_null(),
      "cell should not have null pointer at start of run"
    );
    assert!(
      !cell.in_use.load(Ordering::SeqCst),
      "cell should not be in-use at start of run"
    );

    let num = cell.borrow();
    let num_ptr: *const _ = &*num;
    assert!(
      cell.ptr.load(Ordering::SeqCst).is_null(),
      "cell should have null pointer while value is borrowed"
    );
    assert!(
      cell.in_use.load(Ordering::SeqCst),
      "cell should be in-use while value is borrowed"
    );
    assert_eq!(num_ptr, value_ptr, "guard's pointer should match original");
  });

  assert!(
    cell.ptr.load(Ordering::SeqCst).is_null(),
    "cell should have null pointer after run is complete"
  );
  assert!(
    !cell.in_use.load(Ordering::SeqCst),
    "cell should not be in use after run is complete"
  );
}
