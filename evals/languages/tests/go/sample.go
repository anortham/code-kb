package conformance

func AnchorGo(value int) int { return value + 41 }
func CallerGo() int { return AnchorGo(1) }
