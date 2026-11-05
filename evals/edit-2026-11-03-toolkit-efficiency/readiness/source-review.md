# K1 independent source review

Reviewed by the lead before confirmation exposure at 2026-11-04T01:42:06.047350+00:00.

Reference test names are not required of model patches. Raw readiness remains unchanged; accepted derivatives bind this review.

## flask-config

Reviewed defaults and permanent-session expiration, valid timedelta/None, non-permanent bypass, and invalid permanent type. Reference replaces the seeded zero default. New public request/cookie tests and hidden checks pass.

Case SHA-256: `00bb6dec8edcd93059ae3e1ce670331f7448e2a0dc8962706fc0e2d64478b5c4`. Raw readiness SHA-256: `efc990253615e63524212449cd00b06286168a9dc3795c876434f590243b9af7`.

## express-route

Reviewed validation before mutation, depth-default binding, recompilation after depth changes, zero-depth routes, and simple/disabled/custom parser preservation. New route tests and seven independent hidden checks pass.

Case SHA-256: `92bd3e7736b45473f4c2a904c224d3234e45bca8068604738f26a69cf573e3b5`. Raw readiness SHA-256: `17af1ac07d2d2a79f2ca29b11b7bb320d7223cf270eae9b4e79ad6a4d0304418`.

## minijinja-config

Reviewed optional CLI fuel handling: only an explicit Some value overrides environment/config, including zero. Positive behavior remains assigned unchanged. Help and new zero/positive CLI tests pass. Protected old help snapshot exclusion is explicit in request; new help coverage verifies the changed contract.

Case SHA-256: `3580ae7fd74760eec9ed0e4c0f07a274552949bab76600be5eeb25f72d088915`. Raw readiness SHA-256: `408321bf203ed6e5628fe6dac40f3f91999e75d048b10f1fcfa44bdbf1e38aab`.

## hcl-config

Reviewed full reversal of planted case-folding in shared object/map access and JSON key construction. New public tests cover valid exact-case inputs and wrong-case object/map/JSON traversals. Canonical frontend suites and hidden checks pass.

Case SHA-256: `37fdb1ef7267ae6d9165a9fb54623b71a428f753b5881c67c56dcf6c26540079`. Raw readiness SHA-256: `54061ef3d12c27032ea28b5d6e73b73419ac4465170a4592da478f8282ee7189`.

## commons-lang

Reviewed both ZoneId entrypoints delegating existing conversion/formatting with explicit non-null zones. DST, fixed offset, epoch preservation, unchanged Date, and null behavior are checked. Four separate public reference tests pass.

Case SHA-256: `f5bcc4a9f9f1c631d17a5934ffc91de72cf4157bdbf1b8b2cacf784cfe21960f`. Raw readiness SHA-256: `49ba8e01dd4841211e0da2d6d51c30911fb9cf33db9b76cf2a96d04fae737e5c`.

## nodatime

Reviewed both Try conversion wrappers, success/default-out paths and narrow range exceptions, existing To behavior, precision, Julian calendar, and Pure annotations. Existing purity is a public regression. Seven separate public reference tests pass.

Case SHA-256: `a55ab9b83e8b0a45c9ed8bcbaf79a6f3350c9cb89232a04662993a7bfd05ab43`. Raw readiness SHA-256: `d28d2a40d818c0a7a14134de73a2e7381104867cfa84669de04ed0c0eb657c76`.
