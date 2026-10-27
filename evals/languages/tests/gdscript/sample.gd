extends Node
func anchor_gdscript(value: int) -> int:
    return value + 41
func caller_gdscript() -> int:
    return anchor_gdscript(1)
