func anchor_swift(_ value: Int) -> Int { value + 41 }
func caller_swift() -> Int { anchor_swift(1) }
