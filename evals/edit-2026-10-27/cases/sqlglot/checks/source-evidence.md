# Pinned source and readiness evidence

- Repository: https://github.com/tobymao/sqlglot.git
- Pin: `feb0e08aba28a29590596bb3f5761fbaeb0e98b9`
- Full-history clone: `target/edit-evaluation/sources/sqlglot`; clean at the pin. The root `LICENSE` is retained. Clone history is not included in trial exports.
- Production eligibility: 183 tracked Python runtime files and 85,099 physical lines under `sqlglot/`, excluding tests, docs, examples, dependencies, and generated output. The request requires coordinated parser and generator changes.
- Production paths: `sqlglot/parsers/bigquery.py` (pinned source mapping area at lines 302–305) and `sqlglot/generators/bigquery.py` (pinned transform area at lines 515–519).
- Upstream guidance: `README.md` documents `make unit`. The selected BigQuery test module imports `pytz`, listed in the upstream development extra in `setup.py`.
- Exposure screen: corpus/review files named by `corpus.json` contain no sqlglot, tobymao, or pinned repository matches.

## Prepared inputs

- Preparation downloaded the public wheel `pytz==2026.5` in sandbox `prepare` mode. Wheel: `pytz-2026.5-py2.py3-none-any.whl`, SHA-256 `e658af3757f9e26a9d25dd2aff38335acd92bc9104f890a894b2c1ba28311b03`.
- Canonical dependency input: `/tmp/code-kb-edit-python-rust-20261027/inputs/python-packages`, mounted as `/deps/python-packages`; tree SHA-256 `47cb32636d3296545e7e40015bf2a8189c6ce4cc9d647c62ae12714c4d2e92f3`.
- Runtime setup installs only from that mount into `/state/python-site`; tests set `PYTHONPATH=/state/python-site:/workspace/project` and run `/usr/bin/python3` 3.14.8.
- A sandbox import probe printed `/workspace/project/sqlglot/__init__.py`, confirming tests import the checked-out source rather than an installed subject package.
- No upstream setup, build, or test command ran on the host. Grade commands used network-disabled sandbox mode.

## Readiness results

- Integrated readiness record: `target/edit-evaluation/task3_python_rust/official-readiness/sqlglot/readiness.json`, created `2026-10-27T14:36:43Z`, status `ready`, source-tree SHA-256 `67f8d6f079a5fcd3c512ca367ec9c1744e78302b44795b025dc8e90827725b59`.
- Baseline BigQuery regressions: 64 tests passed in 0.87 s.
- Seeded hidden check: exit 1 in 0.31 s. Its diagnostic is `bucket width belongs in DateBin.this`; the parser creates `DateBin` with the timestamp in `this`.
- Reference BigQuery regressions: 64 tests passed in 0.87 s. Reference hidden check: 2 tests passed in 0.31 s, covering canonical argument order, both arities, and parse-to-generate round trips.
- Reference patch changed exactly `sqlglot/parsers/bigquery.py` and `sqlglot/generators/bigquery.py`.
- Retained command logs are under `target/edit-evaluation/task3_python_rust/official-readiness/sqlglot/evidence/`.
