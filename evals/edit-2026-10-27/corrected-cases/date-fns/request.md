Fix timezone context handling in the business-day helpers.

When `options.in` is provided, `addBusinessDays` must construct and shift the
input in that context, and `differenceInBusinessDays` must compare dates using
calendar days in the same context. Cover inputs near local midnight, where the
requested timezone can place a timestamp on a different date than UTC. Keep the
existing behavior when no context is supplied.
