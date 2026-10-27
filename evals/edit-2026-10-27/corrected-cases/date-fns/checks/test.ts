import { tz } from "@date-fns/tz";
import { describe, expect, it } from "vitest";
import { addBusinessDays } from "../addBusinessDays/index.ts";
import { differenceInBusinessDays } from "../differenceInBusinessDays/index.ts";

describe("business-day context", () => {
  it("uses the supplied context while shifting", () => {
    const result = addBusinessDays("2024-08-20T15:00:00Z", 3, {
      in: tz("Asia/Singapore"),
    });

    expect(result.toISOString()).toBe("2024-08-23T23:00:00.000+08:00");
  });

  it("uses the supplied context while counting", () => {
    const result = differenceInBusinessDays(
      "2024-04-10T02:00:00Z",
      "2024-04-08T06:00:00Z",
      { in: tz("America/New_York") },
    );

    expect(result).toBe(1);
  });
});
