import unittest

import sqlglot
from sqlglot import exp


class BigQueryTimestampBucketPublicTests(unittest.TestCase):
    def test_timestamp_bucket_parses_width_timestamp_and_origin_into_date_bin(self):
        parsed = sqlglot.parse_one(
            "SELECT TIMESTAMP_BUCKET(event_ts, INTERVAL 15 MINUTE, origin_ts)",
            read="bigquery",
        )
        bucket = parsed.find(exp.DateBin)

        self.assertIsInstance(bucket, exp.DateBin)
        self.assertEqual(bucket.this.sql(dialect="bigquery"), "INTERVAL '15' MINUTE")
        self.assertEqual(bucket.expression.name, "event_ts")
        self.assertEqual(bucket.args["origin"].name, "origin_ts")

    def test_timestamp_bucket_generator_preserves_origin_arity(self):
        queries = (
            (
                "SELECT TIMESTAMP_BUCKET(event_ts, INTERVAL 15 MINUTE)",
                None,
            ),
            (
                "SELECT TIMESTAMP_BUCKET(event_ts, INTERVAL 15 MINUTE, origin_ts)",
                "origin_ts",
            ),
        )

        for source, expected_origin in queries:
            with self.subTest(expected_origin=expected_origin):
                parsed = sqlglot.parse_one(source, read="bigquery")
                generated = parsed.sql(dialect="bigquery")
                reparsed_bucket = sqlglot.parse_one(generated, read="bigquery").find(exp.DateBin)

                self.assertIsInstance(reparsed_bucket, exp.DateBin)
                self.assertEqual(reparsed_bucket.this.sql(dialect="bigquery"), "INTERVAL '15' MINUTE")
                self.assertEqual(reparsed_bucket.expression.name, "event_ts")
                origin = reparsed_bucket.args.get("origin")
                self.assertEqual(origin.name if origin is not None else None, expected_origin)


if __name__ == "__main__":
    unittest.main()
