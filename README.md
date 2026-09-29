# Ffi Cell

When calling C functions that take function-pointers as an argument, you often
need a place to store non-static data that your rust-callbacks can access.

This library offers a safe way to lend non-static data so that it can be
reborrowed within a closure.

```rust
# use ffi_cell::FfiCell;
#
let mut n = 42; // non-static data
DATA.run(&mut n, || {
  // the data can be borrowed within this closure
  fn_that_takes_callback(callback);
});

static DATA: FfiCell<i32> = FfiCell::new();

extern "C" fn callback() {
  *DATA.borrow() += 1;
}
# fn fn_that_takes_callback(f: extern "C" fn()) {};
```
