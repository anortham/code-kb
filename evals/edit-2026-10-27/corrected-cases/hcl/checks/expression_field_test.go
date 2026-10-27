package gohcl

import (
	"testing"

	"github.com/hashicorp/hcl/v2"
)

func TestEditEvaluationExpressionFieldBehavior(t *testing.T) {
	type schemaInput struct {
		Expression hcl.Expression `hcl:"expression"`
		Required   string         `hcl:"required"`
		Optional   *string        `hcl:"optional"`
	}

	schema, partial := ImpliedBodySchema(schemaInput{})
	if partial {
		t.Fatal("schema unexpectedly accepts unrecognized attributes")
	}

	requiredByName := make(map[string]bool, len(schema.Attributes))
	for _, attribute := range schema.Attributes {
		requiredByName[attribute.Name] = attribute.Required
	}
	if len(requiredByName) != 3 {
		t.Fatalf("got %d attributes, want 3", len(requiredByName))
	}
	if requiredByName["expression"] {
		t.Error("hcl.Expression fields must remain optional")
	}
	if !requiredByName["required"] {
		t.Error("ordinary scalar attributes must remain required")
	}
	if requiredByName["optional"] {
		t.Error("pointer attributes must remain optional")
	}

	var decoded struct {
		Expression hcl.Expression `hcl:"expression"`
	}
	if diags := DecodeBody(hcl.EmptyBody(), nil, &decoded); diags.HasErrors() {
		t.Fatalf("DecodeBody rejected an omitted expression: %v", diags)
	}
	if decoded.Expression == nil {
		t.Fatal("an omitted expression must decode to a non-nil null expression")
	}

	value, diags := decoded.Expression.Value(nil)
	if diags.HasErrors() {
		t.Fatalf("evaluating the omitted expression returned diagnostics: %v", diags)
	}
	if !value.IsNull() {
		t.Fatal("omitted expression did not evaluate to null")
	}
}
