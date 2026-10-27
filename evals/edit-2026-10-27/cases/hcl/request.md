Fix GoHCL's handling of omitted expression fields.

An `hcl.Expression` field inferred by `ImpliedBodySchema` must stay optional. When
`DecodeBody` sees that field omitted, it must assign a non-nil expression that
evaluates to null. Ordinary scalar attributes must remain required, and pointer
attributes must remain optional.
