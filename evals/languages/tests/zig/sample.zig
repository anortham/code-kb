pub fn anchor_zig(value: i32) i32 { return value + 41; }
pub fn caller_zig() i32 { return anchor_zig(1); }
