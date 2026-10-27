import unittest

import sqlglot
from sqlglot import exp


class TimestampBucketTests(unittest.TestCase):
    def test_bigquery_timestamp_bucket_preserves_canonical_argument_order(self):
        source = "SELECT TIMESTAMP_BUCKET(event_ts, INTERVAL 15 MINUTE, origin_ts) AS bucket"
        parsed = sqlglot.parse_one(source, read="bigquery")
        bucket = parsed.find(exp.DateBin)

        self.assertIsInstance(bucket, exp.DateBin, "TIMESTAMP_BUCKET should parse as DateBin")
        self.assertEqual(
            bucket.this.sql(dialect="bigquery"),
            "INTERVAL '15' MINUTE",
            "bucket width belongs in DateBin.this",
        )
        self.assertEqual(bucket.expression.sql(dialect="bigquery"), "event_ts")
        self.assertEqual(
            bucket.args["origin"].sql(dialect="bigquery"),
            "origin_ts",
        )

        generated = parsed.sql(dialect="bigquery")
        reparsed = sqlglot.parse_one(generated, read="bigquery")
        reparsed_bucket = reparsed.find(exp.DateBin)

        self.assertIsInstance(reparsed_bucket, exp.DateBin)
        self.assertEqual(reparsed_bucket.this.sql(dialect="bigquery"), "INTERVAL '15' MINUTE")
        self.assertEqual(reparsed_bucket.expression.sql(dialect="bigquery"), "event_ts")
        self.assertEqual(
            reparsed_bucket.args["origin"].sql(dialect="bigquery"),
            "origin_ts",
        )

    def test_bigquery_timestamp_bucket_keeps_omitted_origin_omitted(self):
        parsed = sqlglot.parse_one(
            "SELECT TIMESTAMP_BUCKET(event_ts, INTERVAL 15 MINUTE)",
            read="bigquery",
        )
        bucket = parsed.find(exp.DateBin)

        self.assertIsInstance(bucket, exp.DateBin)
        self.assertIsNone(bucket.args.get("origin"))

        generated = parsed.sql(dialect="bigquery")
        self.assertIn("TIMESTAMP_BUCKET(", generated)
        self.assertNotIn("TIMESTAMP_BUCKET(event_ts, INTERVAL 15 MINUTE, ", generated)

        reparsed = sqlglot.parse_one(generated, read="bigquery").find(exp.DateBin)
        self.assertIsNone(reparsed.args.get("origin"))


if __name__ == "__main__":
    unittest.main()
