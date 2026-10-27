using NUnit.Framework;

namespace NodaTime.Test;

[TestFixture]
public sealed class CalendarNeutralComparisonContractTest
{
    [Test]
    public void LocalDateComparesPhysicalDayAcrossCalendars()
    {
        LocalDate iso = new LocalDate(2011, 1, 2);
        LocalDate sameDayInJulian = iso.WithCalendar(CalendarSystem.Julian);
        LocalDate previousDayInJulian = iso.PlusDays(-1).WithCalendar(CalendarSystem.Julian);

        Assert.AreEqual(0, iso.CompareToIgnoringCalendar(sameDayInJulian));
        Assert.Greater(iso.CompareToIgnoringCalendar(previousDayInJulian), 0);
        Assert.Throws<System.ArgumentException>(() => iso.CompareTo(sameDayInJulian));
    }

    [Test]
    public void LocalDateTimeComparesPhysicalDayThenTimeAcrossCalendars()
    {
        LocalDateTime iso = new LocalDateTime(2011, 1, 2, 10, 30);
        LocalDateTime sameTimeInJulian = iso.WithCalendar(CalendarSystem.Julian);
        LocalDateTime laterTimeInJulian = new LocalDateTime(2011, 1, 2, 11, 0).WithCalendar(CalendarSystem.Julian);
        LocalDateTime nextDayInJulian = new LocalDateTime(2011, 1, 3, 10, 30).WithCalendar(CalendarSystem.Julian);

        Assert.AreEqual(0, iso.CompareToIgnoringCalendar(sameTimeInJulian));
        Assert.Less(iso.CompareToIgnoringCalendar(laterTimeInJulian), 0);
        Assert.Less(iso.CompareToIgnoringCalendar(nextDayInJulian), 0);
        Assert.Throws<System.ArgumentException>(() => iso.CompareTo(sameTimeInJulian));
    }
}
