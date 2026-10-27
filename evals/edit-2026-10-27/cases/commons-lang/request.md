Move the null-safe comparable helpers marked for migration in `ObjectUtils` to `ComparableUtils`.

Add `ComparableUtils.compare(T, T)` and `ComparableUtils.compare(T, T, boolean nullGreater)`. The default comparison places `null` before every non-null value. The flag reverses only null ordering; two nulls compare equal, and two non-null values use `compareTo`.

Add varargs `ComparableUtils.max(T...)` and `ComparableUtils.min(T...)` with the existing `ObjectUtils` behavior: a null array, empty array, or all-null array returns null; null entries are ignored; equal values retain the first occurrence. Keep the existing two-value `ComparableUtils.max` and `min` behavior unchanged.

Keep the existing `ObjectUtils.compare`, `max`, and `min` signatures as compatibility methods that delegate to the corresponding `ComparableUtils` methods. Preserve the generic bound that accepts a value comparable to one of its supertypes.
