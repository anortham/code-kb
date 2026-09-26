# Built-in search: bounded owner/member expansion

**Decision: keep.** A modest improvement: a few Miller (C#) gains, no gold-rank regressions,
exact-identifier results unchanged. Cost: about 1–3 ms warm p50 and about 4.5 MiB more
retained RSS on the code-kb corpus. No top-1 gain; this does not show broad search improvement.

## Change

After the first rerank, search takes at most three code types among the top 20 rows and reads
at most 40 direct members of each in source order, with the ordinary kind, path, test, and
document filters. A member must match a query term in its own name, signature, or docstring.
Terms it otherwise misses may get 0.5 credit from the owner's name and first 400 bytes of
documentation. One final rerank follows. No recursion, schema, dependency, or flag.

## Results

66 synthetic queries over frozen code-kb, Julie, and Miller snapshots: 48 known cases plus
18 fresh holdout cases authored without viewing their rankings.

| Cohort | Queries | Limit | Top 1 | Top 5 | MRR | Recall |
|---|---:|---:|---:|---:|---:|---:|
| Original development | 12 | 20 | 5 → 5 | 7 → 7 | 0.4792 → 0.4861 | 8 → 8 |
| Original evaluation | 36 | 20 | 13 → 13 | 18 → 18 | 0.4240 → 0.4279 | 23 → 24 |
| Fresh holdout | 18 | 20 | 8 → 8 | 9 → 10 | 0.5046 → 0.5152 | 15 → 15 |
| Original evaluation | 36 | 50 | 13 → 13 | 17 → 17 | 0.4215 → 0.4265 | 24 → 26 |
| Fresh holdout | 18 | 50 | 8 → 8 | 9 → 10 | 0.5030 → 0.5136 | 15 → 15 |

Paired 95% bootstrap intervals on the MRR gains all touch zero.

| Case | Accepted symbol | Rank at 20 | Rank at 50 |
|---|---|---:|---:|
| `miller-concept-01` | `WorkspaceRootSafety.RejectSensitiveRoot` | 12 → 6 | 14 → 8 |
| `miller-concept-04` | `EditPlanner.ReplaceSymbolBody` | miss → 7 | miss → 7 |
| `miller-concept-06` | `ContentFileClassifier.IsDocsLike` | miss → miss | miss → 27 |
| `holdout-miller-concept-endpoint-placeholders` | `RouteNormalizer.FromEndpoint` | 7 → 3 | 7 → 3 |

## Limits

Later members and unselected owners are still missed. A `kind=method` search has no owner
types in its pool and skips the expansion. Owner credit reads capped metadata only, so a
query with no lexical overlap is not repaired. All gains are in class-heavy C#; expect less elsewhere.

## Rejected experiments

Measured on the same cases and not shipped: the hosted Jev reranker (helped conceptual
queries, but needs a network call per search), local classifier rerankers (no gain over the
built-in ranking), possessive-query normalization (lost a default-limit top-5 answer), and
identifier-word FTS indexing (no recall gain, ten rank losses). None fixes the larger problem,
candidate recall: 12 of 36 evaluation targets never enter the pool. The harnesses, raw results,
and write-ups are on the `research/search-experiments` branch.
