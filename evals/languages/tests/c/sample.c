int anchor_c(int value) { return value + 41; }
int caller_c(void) { return anchor_c(1); }
