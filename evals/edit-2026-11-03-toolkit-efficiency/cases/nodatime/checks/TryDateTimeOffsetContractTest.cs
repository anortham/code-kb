// Copyright 2026 The Noda Time Authors. All rights reserved.
// Use of this source code is governed by the Apache License 2.0,
// as found in the LICENSE.txt file.

using System;
using NodaTime;
using NUnit.Framework;

namespace NodaTime.Test;

[TestFixture]
public sealed class TryDateTimeOffsetContractTest
{
    [Test]
    public void InstantConvertsInRangeAndTruncatesTowardStartOfTime()
    {
        var instant = Instant.FromUtc(2024, 7, 1, 12, 0).PlusNanoseconds(NodaConstants.NanosecondsPerSecond - 1);

        Assert.That(instant.TryToDateTimeOffset(out var result), Is.True);
        Assert.That(result, Is.EqualTo(new DateTimeOffset(2024, 7, 1, 12, 0, 0, TimeSpan.Zero).AddTicks(NodaConstants.TicksPerSecond - 1)));
    }

    [Test]
    public void InstantReturnsFalseForValuesBeforeDateTimeOffsetRange()
    {
        var instant = Instant.FromUtc(1, 1, 1, 0, 0).PlusNanoseconds(-1);

        Assert.That(instant.TryToDateTimeOffset(out var result), Is.False);
        Assert.That(result, Is.EqualTo(default(DateTimeOffset)));
    }

    [Test]
    public void InstantConvertsAtUpperDateTimeOffsetBoundary()
    {
        Assert.That(Instant.MaxValue.TryToDateTimeOffset(out var result), Is.True);
        Assert.That(result, Is.EqualTo(new DateTimeOffset(DateTime.MaxValue.Ticks, TimeSpan.Zero)));
    }

    [Test]
    public void OffsetDateTimeReturnsFalseForOutOfRangeLocalDate()
    {
        var value = new LocalDate(1, 1, 1).PlusDays(-1).AtMidnight().WithOffset(Offset.Zero);

        Assert.That(value.TryToDateTimeOffset(out var result), Is.False);
        Assert.That(result, Is.EqualTo(default(DateTimeOffset)));
    }

    [Test]
    public void OffsetDateTimeReturnsFalseForUnsupportedOffset()
    {
        var value = new LocalDateTime(2024, 7, 1, 12, 0).WithOffset(Offset.FromHours(-15));

        Assert.That(value.TryToDateTimeOffset(out var result), Is.False);
        Assert.That(result, Is.EqualTo(default(DateTimeOffset)));
    }
}
