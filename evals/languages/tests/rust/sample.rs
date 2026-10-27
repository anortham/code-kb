pub fn anchor_rust(value: i32) -> i32 { value + 41 }
pub fn caller_rust() -> i32 { anchor_rust(1) }
