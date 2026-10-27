Add BigQuery TIMESTAMP_BUCKET support to SQLGlot.

Parse BigQuery's (timestamp, bucket_width[, origin]) arguments into the canonical DateBin expression, where the width is this, the timestamp is expression, and the optional origin is origin. Generate canonical DateBin expressions back to BigQuery with the same argument order. Preserve both the two-argument and three-argument forms.

Add regression coverage for argument order, optional origin, and BigQuery parse-to-generate round trips.
