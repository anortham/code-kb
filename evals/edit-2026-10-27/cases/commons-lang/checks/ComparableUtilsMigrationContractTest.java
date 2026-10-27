package org.apache.commons.lang3.compare;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;

import org.apache.commons.lang3.ObjectUtils;
import org.junit.jupiter.api.Test;

class ComparableUtilsMigrationContractTest {

    @Test
    void compareOrdersNullFirstByDefault() {
        assertEquals(0, ComparableUtils.<Integer>compare(null, null));
        assertEquals(-1, ComparableUtils.compare(null, 1));
        assertEquals(1, ComparableUtils.compare(1, null));
        assertEquals(-1, ComparableUtils.compare(1, 2));
        assertEquals(0, ComparableUtils.compare(2, 2));
        assertEquals(1, ComparableUtils.compare(3, 2));
    }

    @Test
    void compareCanOrderNullLast() {
        assertEquals(0, ComparableUtils.<Integer>compare(null, null, true));
        assertEquals(1, ComparableUtils.compare(null, 1, true));
        assertEquals(-1, ComparableUtils.compare(1, null, true));
        assertEquals(-1, ComparableUtils.compare(1, 2, true));
        assertEquals(1, ComparableUtils.compare(3, 2, true));
    }

    @Test
    void objectUtilsComparisonMethodsRemainCompatible() {
        assertEquals(ComparableUtils.<Integer>compare(null, null), ObjectUtils.<Integer>compare(null, null));
        assertEquals(ComparableUtils.compare(null, 1), ObjectUtils.compare(null, 1));
        assertEquals(ComparableUtils.compare(1, null, true), ObjectUtils.compare(1, null, true));
    }

    @Test
    void maxAndMinIgnoreNullValuesAndPreserveTheirExistingContracts() {
        assertNull(ComparableUtils.max());
        assertNull(ComparableUtils.min(new Integer[0]));
        assertNull(ComparableUtils.max(new Integer[] {null, null}));
        assertNull(ComparableUtils.min((Integer[]) null));
        assertEquals(3, ComparableUtils.max(null, 2, 3, 3));
        assertEquals(2, ComparableUtils.min(null, 2, 3, 2));
        assertEquals(3, ObjectUtils.max(null, 2, 3, 3));
        assertEquals(2, ObjectUtils.min(null, 2, 3, 2));

        String first = new String("equal");
        String second = new String("equal");
        assertSame(first, ComparableUtils.max(new String[] {first, second}));
        assertSame(first, ComparableUtils.min(new String[] {first, second}));
    }

    @Test
    void compareAcceptsValuesComparableToTheirBaseType() {
        assertEquals(-1, ComparableUtils.compare(new Derived(1), new Derived(2)));
        Derived maximum = ComparableUtils.max(new Derived[] {new Derived(1), new Derived(2)});
        assertEquals(0, ComparableUtils.compare(maximum, new Derived(2)));
    }

    private static class Base implements Comparable<Base> {
        private final int value;

        private Base(int value) {
            this.value = value;
        }

        @Override
        public int compareTo(Base other) {
            return Integer.compare(value, other.value);
        }
    }

    private static final class Derived extends Base {
        private Derived(int value) {
            super(value);
        }
    }
}
