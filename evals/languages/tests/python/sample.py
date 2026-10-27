def anchor_python(value: int) -> int:
    return value + 41

def caller_python() -> int:
    return anchor_python(1)
