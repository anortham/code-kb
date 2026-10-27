# Bounded receiver evidence in reference reports

## Context

The typed receiver repair reused one SQL proof for reference labels and impact queries. Embedding the full proof in both the judge predicate and evidence expression produced a 380 KB statement. SQLite preparation dominated execution and caused report-budget fallbacks on small fixtures under parallel load. Factoring expressions and report-local statements improved it, but the expanded query still caused deadline failures.

## Decision

Compute receiver owners and nullable target status separately for each existing reference-report chunk and target. Bind a keyed JSON status map into the compact judge. The chunk bounds temporary evidence; SQLite remains the source of owner and ancestry facts. Graph queries continue to use the same proof factories.

Use raw pending-row chains, caller language, and source positions for this proof. A JavaScript chain augmented for another heuristic is not interchangeable with the recorded pending call. Preserve synthetic target columns for removed symbols, since a missing current-index row does not prove that a caller is excluded. Unknown status stays unknown and never becomes a positive claim.

## Consequences

The judge can reuse computed evidence without repeatedly compiling nested owner and ancestry programs. Fixed prepared statements live only for one report. This adds a private adapter between shared proof queries and report classification; it changes no MCP/CLI schema.

Do not replace the report's claim/rival ordering, hydrate repository graphs, register SQL functions, write temporary tables, cache repository data globally, or increase deadlines to hide query expansion.

## Applies to

`queries.rs` receiver proof factories and `reference_report.rs` chunk judging.

## Future agents

Preserve reference/impact agreement, nullable evidence, absent targets, overload aggregation, mixed plain/nested chunks, and legacy-schema fallback. Measure SQL preparation separately from execution when changing this boundary. Generic declaration evidence must come from indexed metadata or bounded declaration recognition; missing facts must not turn a type parameter into a same-named project class.
