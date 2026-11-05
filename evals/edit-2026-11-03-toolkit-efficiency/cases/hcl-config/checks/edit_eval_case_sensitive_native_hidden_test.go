package hclsyntax_test

import (
	"strings"
	"testing"

	"github.com/hashicorp/hcl/v2"
	"github.com/hashicorp/hcl/v2/hclsyntax"
	"github.com/zclconf/go-cty/cty"
)

func TestEditEvalCaseSensitiveNativeTraversal(t *testing.T) {
	ctx := &hcl.EvalContext{Variables: map[string]cty.Value{
		"local": cty.ObjectVal(map[string]cty.Value{
			"settings": cty.ObjectVal(map[string]cty.Value{"Region": cty.StringVal("north")}),
			"tags":     cty.MapVal(map[string]cty.Value{"Release": cty.StringVal("stable")}),
		}),
	}}
	exact, diags := hclsyntax.ParseExpression([]byte("local.settings.Region"), "test.hcl", hcl.InitialPos)
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	value, diags := exact.Value(ctx)
	if diags.HasErrors() || value.AsString() != "north" {
		t.Fatalf("exact-case access failed: %v %s", value, diags.Error())
	}
	wrong, diags := hclsyntax.ParseExpression([]byte("local.settings.region"), "test.hcl", hcl.InitialPos)
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	_, diags = wrong.Value(ctx)
	if !diags.HasErrors() || !strings.Contains(diags.Error(), "Unsupported attribute") {
		t.Fatalf("case sensitivity must be preserved in native traversal; diagnostics: %s", diags.Error())
	}
	wrongMapKey, diags := hclsyntax.ParseExpression([]byte("local.tags.release"), "test.hcl", hcl.InitialPos)
	if diags.HasErrors() {
		t.Fatal(diags.Error())
	}
	_, diags = wrongMapKey.Value(ctx)
	if !diags.HasErrors() || !strings.Contains(diags.Error(), "Missing map element") {
		t.Fatalf("map keys must remain case-sensitive; diagnostics: %s", diags.Error())
	}
}
