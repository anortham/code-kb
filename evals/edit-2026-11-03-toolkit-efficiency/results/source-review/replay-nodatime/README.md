# NodaTime added public test replays

Fresh isolated public replays ran after the timed campaign reached 36/36 terminal trials. The helper used the frozen seed, attempt bundle, source pin, and offline inputs; no hidden overlay was installed.

| Attempt | Added class | Passed | Status |
|---|---|---:|---|
| `nodatime-r1-native` | `NodaTime.Test.TryToDateTimeOffsetTest` | 33/33 | passed |
| `nodatime-r1-current` | `NodaTime.Test.TryToDateTimeOffsetTest` | 26/26 | passed |
| `nodatime-r1-candidate` | `NodaTime.Test.BclDateTimeOffsetConversionTest` | 36/36 | passed |
| `nodatime-r2-candidate` | `NodaTime.Test.DateTimeOffsetConversionTest` | 27/27 | passed |
| `nodatime-r2-current` | `NodaTime.Test.TryToDateTimeOffsetTest` | 37/37 | passed |
| `nodatime-r2-native` | `NodaTime.Test.DateTimeOffsetConversionTest` | 31/31 | passed |

Each attempt directory contains its helper receipt and copied trusted stdout/stderr logs. `manifest.json` records exact argv, observed test names and counts, source and replay hashes, and copied log hashes. The r2 native class was verified from the completed attempt's added test blob because its provisional source note was not present when replay began.
