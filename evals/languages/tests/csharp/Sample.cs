public static class Sample {
    public static int AnchorCsharp(int value) { return value + 41; }
    public static int CallerCsharp() { return AnchorCsharp(1); }
}
