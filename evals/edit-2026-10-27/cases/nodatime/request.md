Add `CompareToIgnoringCalendar` to both `LocalDate` and `LocalDateTime`.

For `LocalDate`, compare the physical day on the Unix-epoch timeline. For `LocalDateTime`, compare the physical day first and then the time of day. Each method returns a negative, zero, or positive value according to that ordering and allows values in different calendar systems to be compared.

Keep `CompareTo`, equality, and relational operators calendar-sensitive. Do not convert through `DateTime` or change either value's calendar.
