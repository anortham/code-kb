/*
 * Licensed to the Apache Software Foundation (ASF) under one or more
 * contributor license agreements.  See the NOTICE file distributed with
 * this work for additional information regarding copyright ownership.
 * The ASF licenses this file to You under the Apache License, Version 2.0
 * (the "License"); you may not use this file except in compliance with
 * the License.  You may obtain a copy of the License at
 *
 *      https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package org.apache.commons.lang3.time;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.time.Instant;
import java.time.ZoneId;
import java.util.Calendar;
import java.util.Date;

import org.junit.jupiter.api.Test;

class ZoneIdInteropContractTest {
    @Test
    void calendarUsesDaylightSavingRules() {
        ZoneId zone = ZoneId.of("America/New_York");
        Date date = Date.from(Instant.parse("2024-07-01T12:00:00Z"));

        long expectedMillis = date.getTime();
        Calendar calendar = DateUtils.toCalendarInZone(date, zone);

        assertEquals(zone, calendar.getTimeZone().toZoneId());
        assertEquals(8, calendar.get(Calendar.HOUR_OF_DAY));
        assertEquals(-4 * 60 * 60 * 1000, calendar.getTimeZone().getOffset(date.getTime()));
        assertEquals(expectedMillis, calendar.getTimeInMillis());
        assertEquals(expectedMillis, date.getTime());
    }

    @Test
    void calendarRejectsNullZoneId() {
        assertThrows(NullPointerException.class, () -> DateUtils.toCalendarInZone(new Date(0), null));
    }

    @Test
    void formatterUsesDaylightSavingRules() {
        Date date = Date.from(Instant.parse("2024-07-01T12:00:00Z"));

        assertEquals("2024-07-01 08:00 -0400",
                DateFormatUtils.formatInZone(date, "yyyy-MM-dd HH:mm Z", ZoneId.of("America/New_York")));
    }

    @Test
    void formatterRejectsNullZoneId() {
        assertThrows(NullPointerException.class, () -> DateFormatUtils.formatInZone(new Date(0), "yyyy", null));
    }
}
