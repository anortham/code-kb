class Sample {
    static int anchor_java(int value) { return value + 41; }
    static int caller_java() { return anchor_java(1); }
}
