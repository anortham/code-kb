package hclsyntax_test

import (
	"strings"
	"testing"

	"github.com/hashicorp/hcl/v2"
	"github.com/hashicorp/hcl/v2/hclsyntax"
	"github.com/zclconf/go-cty/cty"
)

func TestEditEvalPublicNativeRegion(t *testing.T) {
	ctx := &hcl.EvalContext{Variables: map[string]cty.Value{
		"local": cty.ObjectVal(map[string]cty.Value{
			"settings": cty.ObjectVal(map[string]cty.Value{"Region": cty.StringVal("north")}),
			"tags":     cty.MapVal(map[string]cty.Value{"Release": cty.StringVal("stable")}),
		}),
	}}

	for _, test := range []struct {
		expression string
		want       string
	}{
		{"local.settings.Region", "north"},
		{"local.tags.Release", "stable"},
	} {
		expr, diags := hclsyntax.ParseExpression([]byte(test.expression), "test.hcl", hcl.InitialPos)
		if diags.HasErrors() {
			t.Fatalf("parse %q: %s", test.expression, diags.Error())
		}
		got, diags := expr.Value(ctx)
		if diags.HasErrors() {
			t.Fatalf("evaluate %q: %s", test.expression, diags.Error())
		}
		if got.AsString() != test.want {
			t.Errorf("%q = %q, want %q", test.expression, got.AsString(), test.want)
		}
	}

	for _, test := range []struct {
		expression string
		diagnostic string
	}{
		{"local.settings.region", "Unsupported attribute"},
		{"local.tags.release", "Missing map element"},
	} {
		expr, diags := hclsyntax.ParseExpression([]byte(test.expression), "test.hcl", hcl.InitialPos)
		if diags.HasErrors() {
			t.Fatalf("parse %q: %s", test.expression, diags.Error())
		}
		_, diags = expr.Value(ctx)
		if !diags.HasErrors() || !strings.Contains(diags.Error(), test.diagnostic) {
			t.Errorf("%q diagnostics = %v, want %q", test.expression, diags, test.diagnostic)
		}
	}
}
